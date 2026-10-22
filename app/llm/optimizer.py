"""文本优化器：把语音识别结果转成书面语（ATT 唯一的 LLM 热路径）。

v2 变更（docs/LLM配置模块重设计.md §9）：
- 调用统一走 LiteLLMGateway（超时/重试/错误分级/脱敏都在网关层强制注入）；
- 默认模型来自 LLMStore 的 default_model_id（可在设置页一键切换）；
- **移除 .env DeepSeek 直连兜底**：v1 的「配置失败 → 内置 key 重试」双真相源路径
  整体删除。调用失败直接抛 LLMError（已分级、已脱敏），由调用方按既有失败路径
  复位记录状态并提示用户——失败可见，绝不静默降级。
"""
from __future__ import annotations

import time
from dataclasses import dataclass
from typing import AsyncIterator, Optional

from app.llm.errors import LLMError
from app.utils.logging import get_logger

logger = get_logger(__name__)


@dataclass
class OptimizationResult:
    """一次优化的结果。失败不返回结果——直接抛 LLMError，让失败可见。"""

    original_text: str
    optimized_text: str
    processing_time: float
    tokens_used: int = 0
    model_string: str = ""
    # finish_reason=length：输出达到 max_tokens 被截断（思考 token 也计入）。
    # 截断必须让调用方/用户看见，不能静默丢尾巴。
    truncated: bool = False


@dataclass
class OptimizationStreamEvent:
    """流式优化事件：delta 携带增量文本，done 携带最终结果。"""

    type: str
    text: str = ""
    result: Optional[OptimizationResult] = None


def _get_prompt_manager():
    """Lazy import to avoid circular dependency."""
    from app.prompts import prompt_manager

    return prompt_manager


def _resolve_prompt(text: str, preset_id: Optional[str], style: str, default_prompt: str) -> str:
    """从预设解析提示词；预设缺失时回退到内置默认模板（安全兜底）。"""
    from app.config import user_settings

    pm = _get_prompt_manager()
    resolved_preset_id = preset_id or user_settings.get("default_preset_id")
    preset = pm.get_by_id(resolved_preset_id) if resolved_preset_id else None
    if preset is None:
        preset = pm.get_default()

    if preset:
        rendered = pm.render_prompt(preset.id, text)
        if rendered:
            return rendered

    return default_prompt.format(text=text)


class TextOptimizer:
    """基于 LLM 的文本优化服务。

    构造必须是轻量的（main.py 启动期实例化）：所有重资源（litellm、密钥解密）
    都延迟到首次 optimize() 调用时发生。
    """

    # 内置兜底提示词：预设（prompts/*.txt）缺失或渲染失败时使用
    DEFAULT_PROMPT = """你是一位语音转文字大师，擅长将语音识别后的文字转化为准确、流畅的书面文字。

## 核心原则

在保持原意的前提下，进行适度优化。既要修正错误，又要保留原文的细节和语气。

## 处理任务

1. **修正语音识别错误**
   - 同音字错误：如 "工做" → "工作"、"问提" → "问题"
   - 近音字错误：如 "以经" → "已经"
   - 根据上下文推断正确词汇

2. **优化标点符号**
   - 根据语义添加适当的标点
   - 区分句号、问号、感叹号的使用场景

3. **精简口语表达**
   - 去除无意义的填充词：嗯、啊、呃
   - 去除明显的重复表达
   - 适当调整不通顺的语句

## 注意事项

- ✅ 保持原文的核心意思和所有细节
- ✅ 保留原文的语气和风格
- ❌ 不添加原文没有的信息
- ❌ 不做过度润色或改写

## 示例

原文：我希望在这个音频时长后面增加一个时间,它的格式是多少多少秒,这样我便可以看到它到底花费了多长的时间。
优化：我希望在音频时长后面增加一个时间，格式是"多少多少秒"，这样我就能看到它到底花费了多长时间。
（修正标点，适度精简，但保留所有关键信息）

原文：嗯嗯，我觉得吧，这个方案其实还可以。
优化：我觉得这个方案还可以。
（去除填充词，保留核心意思）

---

原文：
{text}

优化后的文字："""

    # 兼容旧调用（style 参数）：全部指向默认提示词，风格由预设承担
    PROMPTS = {
        "default": DEFAULT_PROMPT,
        "formal": DEFAULT_PROMPT,
        "meeting": DEFAULT_PROMPT,
        "novel": DEFAULT_PROMPT,
        "command": DEFAULT_PROMPT,
    }

    async def optimize(
        self,
        text: str,
        style: str = "default",
        preset_id: Optional[str] = None,
        model_id: Optional[str] = None,
    ) -> OptimizationResult:
        """优化文本。

        Args:
            text: 原始文本
            style: 风格（已废弃，保留参数兼容旧调用方）
            preset_id: 提示词预设 ID（缺省用用户设置的默认预设）
            model_id: 指定模型条目 ID（缺省用 store 的默认优化模型）

        Raises:
            LLMError: 已分级、已脱敏的调用失败（含「未配置任何模型」）
        """
        # 延迟导入：避免 app.llm.store ↔ 配置体系的启动期耦合，也让本模块可独立单测
        from app.llm.gateway import get_llm_gateway
        from app.llm.store import get_llm_store

        if not text.strip():
            return OptimizationResult(
                original_text=text,
                optimized_text=text,
                processing_time=0,
            )

        try:
            prompt = _resolve_prompt(text, preset_id, style, self.DEFAULT_PROMPT)
        except Exception as exc:
            logger.warning("提示词预设解析失败，使用内置默认模板", error=str(exc))
            prompt = self.DEFAULT_PROMPT.format(text=text)

        entry, profile, plain_key = get_llm_store().resolve_runtime(model_id)

        logger.info(
            "开始文本优化",
            model=entry.model,
            profile_id=profile.id,
            preset_id=preset_id,
            text_length=len(text),
        )

        start_time = time.perf_counter()
        chat_result = await get_llm_gateway().chat(
            model_string=entry.model,
            messages=[{"role": "user", "content": prompt}],
            api_key=plain_key,
            api_base=profile.api_base,
            extra_headers=profile.extra_headers or None,
            temperature=entry.params.temperature,
            max_tokens=entry.params.max_tokens,
            timeout_s=entry.params.timeout_s,
            retries=entry.params.retries,
        )
        processing_time = time.perf_counter() - start_time

        logger.info(
            "文本优化完成",
            model=chat_result.model_string,
            original_length=len(text),
            optimized_length=len(chat_result.text),
            processing_time=f"{processing_time:.2f}s",
            tokens_used=chat_result.total_tokens,
            prompt_tokens=chat_result.prompt_tokens,
            completion_tokens=chat_result.completion_tokens,
            reasoning_tokens=chat_result.reasoning_tokens,
        )

        if chat_result.truncated:
            logger.warning(
                "优化结果被截断（输出达到 max_tokens 上限），文本可能不完整",
                model=chat_result.model_string,
                max_tokens=entry.params.max_tokens,
            )

        return OptimizationResult(
            original_text=text,
            optimized_text=chat_result.text,
            processing_time=processing_time,
            tokens_used=chat_result.total_tokens,
            model_string=chat_result.model_string,
            truncated=chat_result.truncated,
        )

    async def optimize_stream(
        self,
        text: str,
        style: str = "default",
        preset_id: Optional[str] = None,
        model_id: Optional[str] = None,
    ) -> AsyncIterator[OptimizationStreamEvent]:
        """流式优化：逐块产出 delta，最后产出 done。

        与 optimize() 使用同一套提示词/模型解析；失败直接抛 LLMError，
        由调用方决定「回退非流式」或「展示错误并复位状态」。
        """
        from app.llm.gateway import get_llm_gateway
        from app.llm.store import get_llm_store

        if not text.strip():
            yield OptimizationStreamEvent(
                type="done",
                result=OptimizationResult(
                    original_text=text,
                    optimized_text=text,
                    processing_time=0,
                ),
            )
            return

        try:
            prompt = _resolve_prompt(text, preset_id, style, self.DEFAULT_PROMPT)
        except Exception as exc:
            logger.warning("提示词预设解析失败，使用内置默认模板", error=str(exc))
            prompt = self.DEFAULT_PROMPT.format(text=text)

        entry, profile, plain_key = get_llm_store().resolve_runtime(model_id)

        logger.info(
            "开始流式文本优化",
            model=entry.model,
            profile_id=profile.id,
            preset_id=preset_id,
            text_length=len(text),
        )

        start_time = time.perf_counter()
        first_token_ms: Optional[int] = None
        parts: list[str] = []
        finish_reason = ""
        prompt_tokens = 0
        completion_tokens = 0
        total_tokens = 0
        reasoning_tokens = 0

        async for chunk in get_llm_gateway().chat_stream(
            model_string=entry.model,
            messages=[{"role": "user", "content": prompt}],
            api_key=plain_key,
            api_base=profile.api_base,
            extra_headers=profile.extra_headers or None,
            temperature=entry.params.temperature,
            max_tokens=entry.params.max_tokens,
            timeout_s=entry.params.timeout_s,
            retries=entry.params.retries,
        ):
            if chunk.text:
                if first_token_ms is None:
                    first_token_ms = int((time.perf_counter() - start_time) * 1000)
                parts.append(chunk.text)
                yield OptimizationStreamEvent(type="delta", text=chunk.text)

            if chunk.finish_reason:
                finish_reason = chunk.finish_reason
            if chunk.prompt_tokens:
                prompt_tokens = chunk.prompt_tokens
            if chunk.completion_tokens:
                completion_tokens = chunk.completion_tokens
            if chunk.total_tokens:
                total_tokens = chunk.total_tokens
            if chunk.reasoning_tokens:
                reasoning_tokens = chunk.reasoning_tokens

        processing_time = time.perf_counter() - start_time
        optimized_text = "".join(parts)
        result = OptimizationResult(
            original_text=text,
            optimized_text=optimized_text,
            processing_time=processing_time,
            tokens_used=total_tokens or (prompt_tokens + completion_tokens),
            model_string=entry.model,
            truncated=finish_reason == "length",
        )

        logger.info(
            "流式文本优化完成",
            model=entry.model,
            original_length=len(text),
            optimized_length=len(optimized_text),
            processing_time=f"{processing_time:.2f}s",
            first_token_ms=first_token_ms,
            prompt_tokens=prompt_tokens,
            completion_tokens=completion_tokens,
            total_tokens=total_tokens,
            reasoning_tokens=reasoning_tokens,
            finish_reason=finish_reason or "stop",
        )

        if result.truncated:
            logger.warning(
                "流式优化结果被截断（输出达到 max_tokens 上限），文本可能不完整",
                model=entry.model,
                max_tokens=entry.params.max_tokens,
            )

        yield OptimizationStreamEvent(type="done", result=result)
