"""ASR 文本 → LLM 滑动窗口校对管道（设计文档 docs/文件转写-多引擎与LLM校对-设计.md §4）。

策略参考开源调研结论（VideoCaptioner 的滑窗 + 验证设计，GPL-3.0 只借鉴
策略不抄代码，见设计文档附录B），提示词为自有文案。

核心约定：

- 窗口双上限：每窗 ≤ batch_segments 段 **且** ≤ batch_chars 字符，切分只发生在
  段边界（豆包整场单 segment 时靠字符上限切窗）；可选携带前文尾部
  context_chars 字作为「前文参考」（不参与输出键集合）；
- 每窗独立执行「LLM → 验证 → 反馈 → 重试」循环，最多 MAX_STEPS 次：
  验证一是键集合完全相等（不吞段/不并段），验证二是逐条相似度阈值
  （difflib：常规 ≥ 0.7，≤ 20 字短文本 ≥ 0.3——按字数近似"10 词"）；
- 验证耗尽的窗口**保留原文**并计入 fallback_batches，流程继续；
- LLM 网关整体不可用（全部窗口都因 LLMError 失败）→ applied=False 降级，
  ASR 原文照常交付，不把任务置 failed；
- 校对不占 ASR 单并发闸门；窗口并发由 TRANSCRIBE_FILE_PROOF_CONCURRENCY 控制；
- LLM 调用统一走 app/llm v2 网关（超时/重试由网关注入），temperature=0.2。
"""

from __future__ import annotations

import asyncio
import difflib
import json
import re
from dataclasses import dataclass, field
from typing import Callable, Optional

from app.config import settings
from app.utils.logging import get_logger

logger = get_logger(__name__)

# 提示词版本：任何措辞调整都必须递增，使校对缓存变体自然失效（设计文档 §5.1）
PROMPT_VERSION = 1

# 单窗「LLM → 验证 → 反馈 → 重试」的最大步数
MAX_STEPS = 3
# 相似度阈值：常规文本 / 短文本（≤ SHORT_TEXT_CHARS 字，近似"10 词"）
SIMILARITY_DEFAULT = 0.7
SIMILARITY_SHORT = 0.3
SHORT_TEXT_CHARS = 20

SYSTEM_PROMPT = """你是语音转写（ASR）文本的校对员。用户给你一个 JSON 对象：键是段落编号，值是识别原文。
你的任务是逐条纠错，规则：
1. 只修正识别错误：错别字、同音/近音字、标点缺失或错用、明显的专有名词错误；
2. 删除口头语气词（呃、嗯、啊）与拟声词（*笑声*、*咳嗽*）；
3. 严禁：改写句式、替换同义词、翻译、总结、合并或拆分段落；
4. 前文参考（如有）仅用于判断术语与人称，不是待校对内容；
5. 输出键集合必须与输入完全一致，值与键一一对应；
6. 只输出 JSON 对象本体：不要解释、不要 markdown 代码围栏。"""


@dataclass
class ProofreadOptions:
    """窗口参数（全部有默认值，请求级可覆盖）。"""

    batch_segments: int = 40
    batch_chars: int = 4000
    context_chars: int = 200

    @classmethod
    def from_request(cls, payload: dict | None) -> "ProofreadOptions":
        """从请求体 proofread_options 构造，非法值回落默认（clamp 到安全区间）。"""
        base = cls()
        if not isinstance(payload, dict):
            return base
        return cls(
            batch_segments=_clamp(payload.get("batch_segments"), 1, 200, base.batch_segments),
            batch_chars=_clamp(payload.get("batch_chars"), 200, 20000, base.batch_chars),
            context_chars=_clamp(payload.get("context_chars"), 0, 2000, base.context_chars),
        )


@dataclass
class ProofreadResult:
    """一次校对管道的归一结果（响应 proofread 字段的数据源）。"""

    applied: bool
    text: str                                  # 校对后全文（降级时 = 原文拼接）
    segments: list[dict] = field(default_factory=list)  # 校对后 segments（时间戳不动）
    model_id: Optional[str] = None
    model_label: str = ""
    windows: int = 0
    requests: int = 0
    fallback_batches: list[int] = field(default_factory=list)
    error: str = ""        # applied=False 且非结构性跳过时的原因
    reason: str = ""       # 结构性跳过原因（如 asr_empty）
    prompt_version: int = PROMPT_VERSION

    def meta(self) -> dict:
        """响应 proofread 字段（不含正文本身）。"""
        payload = {
            "applied": self.applied,
            "llm_model": self.model_label or (self.model_id or "default"),
            "windows": self.windows,
            "requests": self.requests,
            "fallback_batches": self.fallback_batches,
            "prompt_version": self.prompt_version,
        }
        if self.error:
            payload["error"] = self.error
        if self.reason:
            payload["reason"] = self.reason
        return payload


def _clamp(value, low: int, high: int, default: int) -> int:
    try:
        number = int(value)
    except (TypeError, ValueError):
        return default
    return max(low, min(high, number))


# ------------------------------------------------------------------ 模型解析

def resolve_proofread_model(model_id: Optional[str]):
    """解析可调用的模型三元组（entry, profile, 明文密钥）。

    独立成函数是给请求期的前置校验用：未配置任何模型抛 LLMError
    （code=model_not_found），服务层映射 409。
    """
    from app.llm.store import get_llm_store

    return get_llm_store().resolve_runtime(model_id)


def model_label(entry) -> str:
    """模型条目的展示标签（档案名 · 模型名）。"""
    return str(getattr(entry, "label", "") or getattr(entry, "model", "") or "")


# ------------------------------------------------------------------ 窗口切分（纯函数，可直接单测）

def split_windows(texts: list[str], options: ProofreadOptions) -> list[list[int]]:
    """把段索引切成窗口序列：双上限（段数 / 字符数），只切在段边界。

    单段超过 batch_chars 时独立成窗（豆包整场单 segment 的长文本场景）。
    """
    windows: list[list[int]] = []
    current: list[int] = []
    current_chars = 0

    for index, text in enumerate(texts):
        length = len(str(text or ""))
        if current and (
            len(current) >= options.batch_segments
            or current_chars + length > options.batch_chars
        ):
            windows.append(current)
            current, current_chars = [], 0
        current.append(index)
        current_chars += length

    if current:
        windows.append(current)
    return windows


def _clean_for_similarity(text: str) -> str:
    return re.sub(r"\s+", " ", str(text or "")).strip()


def similarity(left: str, right: str) -> float:
    """归一相似度（0~1）：空白折叠后逐字符比对。"""
    return difflib.SequenceMatcher(
        None, _clean_for_similarity(left), _clean_for_similarity(right)
    ).ratio()


def _short_text(text: str) -> bool:
    return len(_clean_for_similarity(text)) <= SHORT_TEXT_CHARS


def validate_window(original: dict[str, str], corrected: dict) -> tuple[bool, str]:
    """验证一窗结果：键集合完全相等 + 逐条相似度不越阈。

    返回 (是否通过, 反馈文案)；反馈文案直接喂回 LLM 参与重试。
    """
    expected = set(original.keys())
    actual = set(corrected.keys()) if isinstance(corrected, dict) else set()
    if expected != actual:
        missing = sorted(expected - actual)
        extra = sorted(actual - expected)
        parts = []
        if missing:
            parts.append(f"缺少键：{missing}")
        if extra:
            parts.append(f"多出键：{extra}")
        return False, "；".join(parts) + f"。必须返回包含全部 {len(expected)} 个键的完整 JSON 对象。"

    for key in sorted(expected, key=int):
        raw = str(original[key])
        fixed = str(corrected[key])
        threshold = SIMILARITY_SHORT if _short_text(raw) else SIMILARITY_DEFAULT
        if similarity(raw, fixed) < threshold:
            return False, f"键 {key} 改动过大（相似度低于 {threshold}）：请只纠错，不要改写句式或替换同义词。"
    return True, ""


# ------------------------------------------------------------------ LLM 输出解析（纯函数）

def parse_llm_json(content: str) -> dict | None:
    """宽容解析 LLM 输出：剥 markdown 围栏 / 前后缀噪音 / 尾随逗号。"""
    text = str(content or "").strip()
    if not text:
        return None

    fence = re.search(r"```(?:json)?\s*(.+?)```", text, re.S)
    if fence:
        text = fence.group(1).strip()

    start, end = text.find("{"), text.rfind("}")
    if start < 0 or end <= start:
        return None
    text = text[start : end + 1]
    text = re.sub(r",\s*([}\]])", r"\1", text)  # 尾随逗号

    try:
        parsed = json.loads(text)
    except ValueError:
        return None
    return parsed if isinstance(parsed, dict) else None


def build_user_prompt(original: dict[str, str], context_tail: str) -> str:
    """构造单窗的用户消息：可选前文参考 + 待校对 JSON。"""
    parts = []
    if context_tail:
        parts.append(f"前文参考（仅供理解语境，不是待校对内容）：\n{context_tail}")
    parts.append("待校对：\n" + json.dumps(original, ensure_ascii=False))
    return "\n\n".join(parts)


# ------------------------------------------------------------------ 管道

class TranscribeProofreader:
    """对 ASR segments 执行滑窗校对。"""

    def __init__(
        self,
        model_id: Optional[str],
        options: ProofreadOptions | None = None,
        on_progress: Optional[Callable[[int, int], None]] = None,
    ):
        self.model_id = model_id
        self.options = options or ProofreadOptions()
        self.on_progress = on_progress

    async def run(self, segments: list[dict]) -> ProofreadResult:
        """segments 为 ASR 响应的 segments 列表（[{start,end,text}]，时间戳原样保留）。"""
        entry, profile, plain_key = resolve_proofread_model(self.model_id)
        label = model_label(entry)
        texts = [str(seg.get("text") or "") for seg in segments]
        joined_raw = " ".join(part for part in texts if part)

        if not joined_raw.strip():
            return ProofreadResult(applied=False, text="", segments=segments,
                                   model_id=self.model_id, model_label=label,
                                   reason="asr_empty")

        windows = split_windows(texts, self.options)
        semaphore = asyncio.Semaphore(max(int(getattr(settings, "transcribe_file_proof_concurrency", 2)), 1))
        done_counter = 0
        lock = asyncio.Lock()

        def _report_progress() -> None:
            nonlocal done_counter
            if self.on_progress:
                try:
                    self.on_progress(done_counter, len(windows))
                except Exception:  # 进度回调不允许影响主流程
                    pass

        # 前文参考：按窗口起点从原文拼接里截取前 context_chars 字（设计 §4.2）
        cumulative_chars: list[int] = []
        total = 0
        for text in texts:
            total += len(text) + 1
            cumulative_chars.append(total)

        async def _run_window(window_index: int, indices: list[int]) -> tuple[dict[int, str] | None, int, str]:
            """返回 (校正映射 or None=保留原文, LLM 请求数, 致命错误文案)。"""
            original = {str(i): texts[i] for i in indices}
            if self.options.context_chars > 0 and indices[0] > 0:
                cut = max(cumulative_chars[indices[0] - 1] - self.options.context_chars, 0)
                context_tail = joined_raw[cut : cumulative_chars[indices[0] - 1]]
            else:
                context_tail = ""

            messages = [
                {"role": "system", "content": SYSTEM_PROMPT},
                {"role": "user", "content": build_user_prompt(original, context_tail)},
            ]
            requests = 0
            try:
                from app.llm.gateway import get_llm_gateway

                for _step in range(MAX_STEPS):
                    chat = await get_llm_gateway().chat(
                        model_string=entry.model,
                        messages=messages,
                        api_key=plain_key,
                        api_base=profile.api_base,
                        extra_headers=profile.extra_headers or None,
                        temperature=0.2,
                        max_tokens=entry.params.max_tokens,
                        timeout_s=entry.params.timeout_s,
                        retries=entry.params.retries,
                    )
                    requests += 1
                    corrected = parse_llm_json(chat.text)
                    ok, feedback = (False, "输出不是 JSON 对象") if corrected is None else validate_window(original, corrected)
                    if ok:
                        return {int(k): str(v) for k, v in corrected.items()}, requests, ""
                    messages.append({"role": "assistant", "content": chat.text})
                    messages.append({
                        "role": "user",
                        "content": f"校验失败：{feedback}\n请修正后只输出完整的 JSON 对象本体。",
                    })
            except Exception as error:  # LLMError 等：网关侧已分级脱敏
                return None, requests, str(error)
            return None, requests, ""  # 验证耗尽 → 保留原文

        async def _guarded(window_index: int, indices: list[int]):
            nonlocal done_counter
            async with semaphore:
                outcome = await _run_window(window_index, indices)
            async with lock:
                done_counter += 1
                _report_progress()
            return outcome

        results = await asyncio.gather(*[
            _guarded(index, indices) for index, indices in enumerate(windows)
        ])

        corrected_map: dict[int, str] = {}
        fallback_batches: list[int] = []
        fatal_errors: list[str] = []
        total_requests = 0
        for window_index, (correction, requests, fatal) in enumerate(results):
            total_requests += requests
            if fatal:
                fatal_errors.append(fatal)
                fallback_batches.append(window_index)
            elif correction is None:
                fallback_batches.append(window_index)
            else:
                corrected_map.update(correction)

        # 全部窗口都因网关错误失败 → 整体降级（设计 §4.5：ASR 结果仍然交付）
        if fatal_errors and len(fatal_errors) == len(windows):
            logger.warning("LLM 校对整体不可用，降级交付 ASR 原文", error=fatal_errors[0])
            return ProofreadResult(
                applied=False, text=joined_raw, segments=segments,
                model_id=self.model_id, model_label=label,
                windows=len(windows), requests=total_requests,
                error=fatal_errors[0],
            )

        new_segments = []
        parts: list[str] = []
        for index, seg in enumerate(segments):
            text = corrected_map.get(index, texts[index])
            new_segments.append({**seg, "text": text})
            if text:
                parts.append(text)

        if fallback_batches:
            logger.warning("部分校对窗口保留原文", windows=fallback_batches)

        return ProofreadResult(
            applied=True, text=" ".join(parts), segments=new_segments,
            model_id=self.model_id, model_label=label,
            windows=len(windows), requests=total_requests,
            fallback_batches=fallback_batches,
        )
