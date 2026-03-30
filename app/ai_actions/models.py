"""AI 动作数据模型。"""
from dataclasses import dataclass, field
from enum import Enum
from typing import Optional


class AIActionType(str, Enum):
    """AI 动作类型。"""

    TRANSLATE = "translate"
    EXPLAIN = "explain"
    SUMMARIZE = "summarize"
    REWRITE = "rewrite"
    CHAT = "chat"


@dataclass
class AIActionResult:
    """AI 动作返回结果。"""

    action_type: AIActionType
    source_text: str
    detected_language: str = "unknown"
    entry_type: str = "sentence"
    translation: str = ""
    explanations: list[str] = field(default_factory=list)
    examples: list[str] = field(default_factory=list)
    answer: str = ""
    saved_to_wordbook: bool = False
    wordbook_entry_id: Optional[str] = None
    metadata: Optional[dict] = None

    def to_dict(self) -> dict:
        """转换为字典。"""

        return {
            "action_type": self.action_type.value,
            "source_text": self.source_text,
            "detected_language": self.detected_language,
            "entry_type": self.entry_type,
            "translation": self.translation,
            "explanations": list(self.explanations),
            "examples": list(self.examples),
            "answer": self.answer,
            "saved_to_wordbook": self.saved_to_wordbook,
            "wordbook_entry_id": self.wordbook_entry_id,
            "metadata": self.metadata or {},
        }
