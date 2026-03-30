"""
Clipboard manager for JSON-based storage.
"""
import hashlib
import json
import re
from datetime import datetime
from pathlib import Path
from typing import List, Optional

import aiofiles

from app.clipboard.models import ClipboardItem, ClipboardItemType


URL_PATTERN = re.compile(r"^https?://", re.IGNORECASE)
WINDOWS_PATH_PATTERN = re.compile(r"^[a-zA-Z]:[\\/]")
UNC_PATH_PATTERN = re.compile(r"^\\\\[^\\]+\\[^\\]+")


class ClipboardManager:
    """Manages clipboard items using JSON files."""

    def __init__(self, data_path: Path):
        self.data_path = data_path
        self.clipboard_path = data_path / "clipboard"

    async def initialize(self) -> None:
        """Ensure directories exist."""
        self.clipboard_path.mkdir(parents=True, exist_ok=True)

    def _get_item_file(self, item_id: str) -> Path:
        return self.clipboard_path / f"{item_id}.json"

    async def _load_json(self, item_id: str) -> Optional[dict]:
        file_path = self._get_item_file(item_id)
        if not file_path.exists():
            return None

        async with aiofiles.open(file_path, "r", encoding="utf-8") as file:
            return json.loads(await file.read())

    async def _save_json(self, item: ClipboardItem) -> None:
        item.updated_at = datetime.now()
        async with aiofiles.open(self._get_item_file(item.id), "w", encoding="utf-8") as file:
            await file.write(json.dumps(item.to_dict(), ensure_ascii=False, indent=2))

    def _normalize_text(self, content: str) -> str:
        return (content or "").replace("\r\n", "\n").strip()

    def _detect_item_type(self, content: str) -> ClipboardItemType:
        normalized = self._normalize_text(content)
        if not normalized:
            return ClipboardItemType.TEXT

        if URL_PATTERN.match(normalized):
            return ClipboardItemType.LINK

        if WINDOWS_PATH_PATTERN.match(normalized) or UNC_PATH_PATTERN.match(normalized):
            return ClipboardItemType.FILE

        if "\n" in normalized and (
            "```" in normalized or
            re.search(r"\b(function|const|let|class|def|import|return)\b", normalized)
        ):
            return ClipboardItemType.CODE

        if len(normalized) > 280 or normalized.count("\n") >= 3:
            return ClipboardItemType.CODE

        return ClipboardItemType.TEXT

    def _build_content_hash(self, content: str) -> str:
        normalized = self._normalize_text(content)
        if not normalized:
            return ""
        return hashlib.sha256(normalized.encode("utf-8")).hexdigest()

    def _build_title(self, item_type: ClipboardItemType, content: str) -> str:
        normalized = self._normalize_text(content)
        if not normalized:
            return "空剪切板记录"

        first_line = normalized.splitlines()[0].strip()
        if item_type == ClipboardItemType.LINK:
            return first_line[:72]
        if item_type == ClipboardItemType.FILE:
            return Path(first_line).name or first_line[:72]
        return first_line[:36]

    def _build_summary(self, content: str) -> str:
        normalized = self._normalize_text(content)
        if not normalized:
            return ""
        single_line = " ".join(part.strip() for part in normalized.splitlines() if part.strip())
        return single_line[:240]

    async def create_item(
        self,
        content: str,
        source: str = "manual",
        item_type: Optional[ClipboardItemType] = None,
        title: Optional[str] = None,
        summary: Optional[str] = None,
        metadata: Optional[dict] = None
    ) -> ClipboardItem:
        normalized = self._normalize_text(content)
        resolved_type = item_type or self._detect_item_type(normalized)
        content_hash = self._build_content_hash(normalized)
        item = ClipboardItem(
            item_type=resolved_type,
            content=normalized,
            title=(title or self._build_title(resolved_type, normalized)).strip(),
            summary=(summary or self._build_summary(normalized)).strip(),
            source=source,
            content_size=len(normalized),
            duplicate_count=1,
            content_hash=content_hash,
            metadata=metadata or {}
        )
        await self._save_json(item)
        return item

    async def _find_duplicate_item(self, content: str) -> Optional[ClipboardItem]:
        normalized = self._normalize_text(content)
        if not normalized:
            return None

        content_hash = self._build_content_hash(normalized)

        for json_file in self.clipboard_path.glob("*.json"):
            try:
                data = await self._load_json(json_file.stem)
                if not data:
                    continue

                item = ClipboardItem.from_dict(data)
                if item.content_hash and item.content_hash != content_hash:
                    continue

                if self._normalize_text(item.content) == normalized:
                    if not item.content_hash:
                        item.content_hash = content_hash
                    return item
            except Exception:
                continue

        return None

    async def capture_item(
        self,
        content: str,
        source: str = "manual",
        item_type: Optional[ClipboardItemType] = None,
        title: Optional[str] = None,
        summary: Optional[str] = None,
        metadata: Optional[dict] = None
    ) -> ClipboardItem:
        normalized = self._normalize_text(content)
        duplicate = await self._find_duplicate_item(normalized)

        if duplicate:
            duplicate.updated_at = datetime.now()
            duplicate.source = source or duplicate.source
            duplicate.content_size = len(normalized)
            duplicate.content_hash = duplicate.content_hash or self._build_content_hash(normalized)
            duplicate.duplicate_count = max(1, int(duplicate.duplicate_count or 1)) + 1
            if metadata:
                duplicate.metadata = {
                    **(duplicate.metadata or {}),
                    **metadata
                }
            await self._save_json(duplicate)
            return duplicate

        return await self.create_item(
            content=normalized,
            source=source,
            item_type=item_type,
            title=title,
            summary=summary,
            metadata=metadata
        )

    async def get_item(self, item_id: str) -> Optional[ClipboardItem]:
        data = await self._load_json(item_id)
        return ClipboardItem.from_dict(data) if data else None

    async def get_item_image_path(self, item_id: str) -> Optional[Path]:
        item = await self.get_item(item_id)
        if not item or item.item_type != ClipboardItemType.IMAGE:
            return None

        image_path = Path((item.metadata or {}).get("image_path", "")).expanduser()
        if not image_path.exists() or not image_path.is_file():
            return None

        return image_path

    async def list_items(
        self,
        query: Optional[str] = None,
        item_type: Optional[ClipboardItemType] = None,
        favorite_only: bool = False,
        pinned_only: bool = False,
        limit: int = 100
    ) -> List[ClipboardItem]:
        items: List[ClipboardItem] = []
        for json_file in self.clipboard_path.glob("*.json"):
            try:
                data = await self._load_json(json_file.stem)
                if data:
                    items.append(ClipboardItem.from_dict(data))
            except Exception:
                continue

        normalized_query = (query or "").strip().lower()

        def matches(item: ClipboardItem) -> bool:
            if favorite_only and not item.is_favorite:
                return False
            if pinned_only and not item.is_pinned:
                return False
            if item_type and item.item_type != item_type:
                return False
            if not normalized_query:
                return True
            haystack = " ".join([
                item.title or "",
                item.summary or "",
                item.content or ""
            ]).lower()
            return normalized_query in haystack

        filtered = [item for item in items if matches(item)]
        filtered.sort(key=lambda value: (not value.is_pinned, -value.updated_at.timestamp()))
        return filtered[:limit]

    async def update_item(
        self,
        item_id: str,
        content: Optional[str] = None,
        title: Optional[str] = None,
        summary: Optional[str] = None,
        is_favorite: Optional[bool] = None,
        is_pinned: Optional[bool] = None,
        metadata: Optional[dict] = None
    ) -> Optional[ClipboardItem]:
        item = await self.get_item(item_id)
        if not item:
            return None

        content_changed = content is not None
        if content_changed:
            item.content = self._normalize_text(content)
            item.item_type = self._detect_item_type(item.content)
            item.content_size = len(item.content)
            item.content_hash = self._build_content_hash(item.content)

        if title is not None:
            item.title = title.strip()
        elif content_changed:
            item.title = self._build_title(item.item_type, item.content)

        if summary is not None:
            item.summary = summary.strip()
        elif content_changed:
            item.summary = self._build_summary(item.content)

        if is_favorite is not None:
            item.is_favorite = is_favorite
        if is_pinned is not None:
            item.is_pinned = is_pinned
        if metadata is not None:
            item.metadata = metadata

        await self._save_json(item)
        return item

    async def delete_item(self, item_id: str) -> bool:
        file_path = self._get_item_file(item_id)
        if not file_path.exists():
            return False
        file_path.unlink()
        return True

    async def get_stats(self) -> dict:
        items = await self.list_items(limit=10000)
        now = datetime.now()
        today_count = sum(1 for item in items if item.created_at.date() == now.date())
        favorites_count = sum(1 for item in items if item.is_favorite)
        pinned_count = sum(1 for item in items if item.is_pinned)
        return {
            "total": len(items),
            "today": today_count,
            "favorites": favorites_count,
            "pinned": pinned_count
        }
