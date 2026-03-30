"""生词本数据模型。"""
from dataclasses import dataclass, field
from datetime import datetime
from enum import Enum
from typing import Optional
import uuid


class WordbookEntryType(str, Enum):
    """生词条目类型。"""

    WORD = "word"
    PHRASE = "phrase"
    SENTENCE = "sentence"


@dataclass
class WordbookEntry:
    """生词条目数据模型。"""

    id: str = field(default_factory=lambda: str(uuid.uuid4()))
    created_at: datetime = field(default_factory=datetime.now)
    updated_at: datetime = field(default_factory=datetime.now)
    source_text: str = ""
    normalized_text: str = ""
    language: str = "unknown"
    entry_type: WordbookEntryType = WordbookEntryType.WORD
    translation: str = ""
    explanations: list[str] = field(default_factory=list)
    examples: list[str] = field(default_factory=list)
    source_app: str = ""
    source_context: str = ""
    query_count: int = 1
    is_favorite: bool = False
    metadata: Optional[dict] = None

    def to_dict(self) -> dict:
        """转换为 JSON 可序列化字典。"""

        return {
            "id": self.id,
            "created_at": self.created_at.isoformat(),
            "updated_at": self.updated_at.isoformat(),
            "source_text": self.source_text,
            "normalized_text": self.normalized_text,
            "language": self.language,
            "entry_type": self.entry_type.value,
            "translation": self.translation,
            "explanations": list(self.explanations),
            "examples": list(self.examples),
            "source_app": self.source_app,
            "source_context": self.source_context,
            "query_count": int(self.query_count or 1),
            "is_favorite": self.is_favorite,
            "metadata": self.metadata or {},
        }

    @classmethod
    def from_dict(cls, data: dict) -> "WordbookEntry":
        """从字典恢复条目。"""

        return cls(
            id=data["id"],
            created_at=datetime.fromisoformat(data["created_at"]),
            updated_at=datetime.fromisoformat(data["updated_at"]),
            source_text=data.get("source_text", ""),
            normalized_text=data.get("normalized_text", ""),
            language=data.get("language", "unknown"),
            entry_type=WordbookEntryType(data.get("entry_type", WordbookEntryType.WORD.value)),
            translation=data.get("translation", ""),
            explanations=list(data.get("explanations") or []),
            examples=list(data.get("examples") or []),
            source_app=data.get("source_app", ""),
            source_context=data.get("source_context", ""),
            query_count=max(1, int(data.get("query_count", 1) or 1)),
            is_favorite=bool(data.get("is_favorite", False)),
            metadata=data.get("metadata") or {},
        )
