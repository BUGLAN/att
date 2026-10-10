"""AI 动作编排服务。"""
import json
from typing import Optional

from app.ai_actions.models import AIActionResult, AIActionType
from app.llm.gateway import LLMGateway
from app.utils.logging import get_logger
from app.wordbook.manager import WordbookManager
from app.wordbook.models import WordbookEntryType

logger = get_logger(__name__)


class LLMNotConfiguredError(RuntimeError):
    """默认 LLM provider 不可用（未配置 / 缺少 API Key）。

    继承 ``RuntimeError``，保证既有的 ``pytest.raises(RuntimeError)`` 断言不受影响；
    路由层据此返回可读的中文 502 提示，而不是 500 裸抛。
    """


ACTION_PROMPTS = {
    AIActionType.TRANSLATE: """你是桌面端 AI 划词助手。请把输入文本翻译为简体中文，并返回 JSON。

要求：
1. 识别语言，字段名使用 language。
2. 按内容粒度输出 entry_type，只能是 word、phrase、sentence。
3. translation 输出主翻译。
4. explanations 输出 1 到 3 条简短解释。
5. examples 输出 0 到 2 条例句。
6. 只返回 JSON，不要附加解释文本。

原文：
{text}
""",
    AIActionType.EXPLAIN: """你是桌面端 AI 划词助手。请解释输入文本，并返回 JSON。

字段：
- answer: 简洁解释
- explanations: 补充要点列表
- examples: 可选例句

原文：
{text}
""",
    AIActionType.SUMMARIZE: """你是桌面端 AI 划词助手。请总结输入文本，并返回 JSON。

字段：
- answer: 总结结果
- explanations: 要点列表

原文：
{text}
""",
    AIActionType.REWRITE: """你是桌面端 AI 划词助手。请改写输入文本，并返回 JSON。

字段：
- answer: 改写结果
- explanations: 说明改写策略

原文：
{text}
""",
    AIActionType.CHAT: """你是桌面端 AI 划词助手。请围绕输入文本回答问题，并返回 JSON。

字段：
- answer: 直接回答
- explanations: 如有需要可给出要点列表

原文：
{text}
""",
}


ENTRY_TYPE_MAP = {
    "word": WordbookEntryType.WORD,
    "phrase": WordbookEntryType.PHRASE,
    "sentence": WordbookEntryType.SENTENCE,
}

# 多轮对话在单条 prompt 里的角色标签（provider 只接受「文本 + 模板」两参数，
# 因此把历史轮次拼接成带角色前缀的文本，而不是另起一套 messages provider 体系）。
CHAT_ROLE_LABELS = {
    "user": "用户",
    "assistant": "助手",
}

CHAT_SUPPORTED_ROLES = tuple(CHAT_ROLE_LABELS.keys())


class AIActionService:
    """负责执行划词后的 AI 动作。"""

    def __init__(self, llm_gateway: LLMGateway, wordbook_manager: WordbookManager):
        self.llm_gateway = llm_gateway
        self.wordbook_manager = wordbook_manager

    def _require_default_provider(self):
        provider = self.llm_gateway.get_default_provider()
        if not provider:
            raise LLMNotConfiguredError("未配置默认 LLM provider")
        return provider

    def _parse_json_output(self, raw_output: str) -> dict:
        text = (raw_output or "").strip()
        if not text:
            return {}

        try:
            return json.loads(text)
        except json.JSONDecodeError:
            logger.warning("AI 动作输出不是合法 JSON，回退为 answer 文本", raw_output=text)
            return {"answer": text}

    def normalize_chat_messages(self, messages) -> list[dict]:
        """校验并归一化多轮对话消息（支持 pydantic 模型或 dict）。"""

        normalized: list[dict] = []

        for index, message in enumerate(messages or []):
            if isinstance(message, dict):
                role = message.get("role")
                content = message.get("content")
            else:
                role = getattr(message, "role", None)
                content = getattr(message, "content", None)

            resolved_role = str(role or "").strip().lower()
            resolved_content = str(content or "").strip()

            if resolved_role not in CHAT_SUPPORTED_ROLES:
                raise ValueError(f"第 {index + 1} 条消息的 role 必须是 user 或 assistant")
            if not resolved_content:
                raise ValueError(f"第 {index + 1} 条消息的 content 不能为空")

            normalized.append({"role": resolved_role, "content": resolved_content})

        if not normalized:
            raise ValueError("messages 不能为空，至少需要一条用户消息")

        return normalized

    def build_chat_transcript(self, messages) -> str:
        """把多轮消息拼接成单条 prompt 文本，保留完整的上下文顺序。"""

        return "\n".join(
            f"{CHAT_ROLE_LABELS[message['role']]}：{message['content']}"
            for message in self.normalize_chat_messages(messages)
        )

    async def chat(self, messages, metadata: Optional[dict] = None) -> dict:
        """执行多轮对话，复用默认 LLM provider 与既有 CHAT prompt。"""

        normalized = self.normalize_chat_messages(messages)
        provider = self._require_default_provider()
        transcript = self.build_chat_transcript(normalized)

        raw_output = await provider.optimize(transcript, ACTION_PROMPTS[AIActionType.CHAT])
        payload = self._parse_json_output(raw_output)

        # 模型可能返回 JSON（answer 字段），也可能直接返回纯文本；两者都归一到 answer。
        answer = str(payload.get("answer") or "").strip() or str(raw_output or "").strip()

        return {
            "action_type": AIActionType.CHAT.value,
            "answer": answer,
            "explanations": list(payload.get("explanations") or []),
            "source_text": transcript,
            "messages": normalized,
            "metadata": metadata or {},
        }

    async def run_action(
        self,
        action_type: AIActionType,
        source_text: str,
        source_app: str = "",
        source_context: str = "",
        auto_add_to_wordbook: bool = True,
        metadata: Optional[dict] = None,
    ) -> AIActionResult:
        """运行指定 AI 动作。"""

        resolved_text = (source_text or "").strip()
        if not resolved_text:
            raise ValueError("source_text 不能为空")

        provider = self._require_default_provider()
        prompt_template = ACTION_PROMPTS[action_type]
        raw_output = await provider.optimize(resolved_text, prompt_template)
        payload = self._parse_json_output(raw_output)

        result = AIActionResult(
            action_type=action_type,
            source_text=resolved_text,
            detected_language=payload.get("language", "unknown"),
            entry_type=payload.get("entry_type", "sentence"),
            translation=payload.get("translation", ""),
            explanations=list(payload.get("explanations") or []),
            examples=list(payload.get("examples") or []),
            answer=payload.get("answer", ""),
            metadata=metadata or {},
        )

        if action_type == AIActionType.TRANSLATE and auto_add_to_wordbook:
            entry = await self.wordbook_manager.capture_entry(
                source_text=resolved_text,
                normalized_text=resolved_text,
                language=result.detected_language,
                entry_type=ENTRY_TYPE_MAP.get(result.entry_type, WordbookEntryType.SENTENCE),
                translation=result.translation or result.answer,
                explanations=result.explanations,
                examples=result.examples,
                source_app=source_app,
                source_context=source_context,
                metadata=metadata,
            )
            result.saved_to_wordbook = True
            result.wordbook_entry_id = entry.id

        return result

    async def run_action_dict(self, action_type: AIActionType, **kwargs) -> dict:
        """返回字典结构，便于路由直接输出。"""

        return (await self.run_action(action_type=action_type, **kwargs)).to_dict()
