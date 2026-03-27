"""
Data models for clipboard management.
"""
from dataclasses import dataclass, field
from datetime import datetime
from enum import Enum
from typing import Optional
import uuid


class ClipboardItemType(str, Enum):
    """Clipboard item type."""
    TEXT = "text"
    LINK = "link"
    CODE = "code"
    FILE = "file"
    IMAGE = "image"


@dataclass
class ClipboardItem:
    """Clipboard item data model."""
    id: str = field(default_factory=lambda: str(uuid.uuid4()))
    created_at: datetime = field(default_factory=datetime.now)
    updated_at: datetime = field(default_factory=datetime.now)
    item_type: ClipboardItemType = ClipboardItemType.TEXT
    content: str = ""
    title: str = ""
    summary: str = ""
    source: str = "manual"
    is_favorite: bool = False
    is_pinned: bool = False
    content_size: int = 0
    metadata: Optional[dict] = None

    def to_dict(self) -> dict:
        """Convert to dictionary for JSON storage."""
        return {
            "id": self.id,
            "created_at": self.created_at.isoformat(),
            "updated_at": self.updated_at.isoformat(),
            "item_type": self.item_type.value,
            "content": self.content,
            "title": self.title,
            "summary": self.summary,
            "source": self.source,
            "is_favorite": self.is_favorite,
            "is_pinned": self.is_pinned,
            "content_size": self.content_size,
            "metadata": self.metadata or {}
        }

    @classmethod
    def from_dict(cls, data: dict) -> "ClipboardItem":
        """Create from dictionary."""
        return cls(
            id=data["id"],
            created_at=datetime.fromisoformat(data["created_at"]),
            updated_at=datetime.fromisoformat(data["updated_at"]),
            item_type=ClipboardItemType(data.get("item_type", ClipboardItemType.TEXT.value)),
            content=data.get("content", ""),
            title=data.get("title", ""),
            summary=data.get("summary", ""),
            source=data.get("source", "manual"),
            is_favorite=data.get("is_favorite", False),
            is_pinned=data.get("is_pinned", False),
            content_size=data.get("content_size", 0),
            metadata=data.get("metadata") or {}
        )
