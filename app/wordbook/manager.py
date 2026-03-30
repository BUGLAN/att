"""基于 JSON 的生词本管理器。"""
import json
from datetime import datetime
from pathlib import Path
from typing import Optional

import aiofiles

from app.wordbook.models import WordbookEntry, WordbookEntryType


class WordbookManager:
    """管理生词本条目。"""

    def __init__(self, data_path: Path):
        self.data_path = data_path
        self.wordbook_path = data_path / "wordbook"

    async def initialize(self) -> None:
        """确保目录存在。"""

        self.wordbook_path.mkdir(parents=True, exist_ok=True)

    def _get_entry_file(self, entry_id: str) -> Path:
        return self.wordbook_path / f"{entry_id}.json"

    def _normalize_text(self, text: str) -> str:
        return (text or "").replace("\r\n", "\n").strip()

    async def _load_json(self, entry_id: str) -> Optional[dict]:
        file_path = self._get_entry_file(entry_id)
        if not file_path.exists():
            return None

        async with aiofiles.open(file_path, "r", encoding="utf-8") as file:
            return json.loads(await file.read())

    async def _save_json(self, entry: WordbookEntry) -> None:
        entry.updated_at = datetime.now()
        async with aiofiles.open(self._get_entry_file(entry.id), "w", encoding="utf-8") as file:
            await file.write(json.dumps(entry.to_dict(), ensure_ascii=False, indent=2))

    async def _find_duplicate_entry(
        self,
        normalized_text: str,
        language: str,
        entry_type: WordbookEntryType,
    ) -> Optional[WordbookEntry]:
        for json_file in self.wordbook_path.glob("*.json"):
            try:
                data = await self._load_json(json_file.stem)
                if not data:
                    continue

                entry = WordbookEntry.from_dict(data)
                if (
                    self._normalize_text(entry.normalized_text) == normalized_text
                    and entry.language == language
                    and entry.entry_type == entry_type
                ):
                    return entry
            except Exception:
                continue

        return None

    async def create_entry(
        self,
        source_text: str,
        normalized_text: str,
        language: str,
        entry_type: WordbookEntryType,
        translation: str,
        explanations: Optional[list[str]] = None,
        examples: Optional[list[str]] = None,
        source_app: str = "",
        source_context: str = "",
        metadata: Optional[dict] = None,
    ) -> WordbookEntry:
        """创建新条目。"""

        entry = WordbookEntry(
            source_text=self._normalize_text(source_text),
            normalized_text=self._normalize_text(normalized_text),
            language=(language or "unknown").strip() or "unknown",
            entry_type=entry_type,
            translation=self._normalize_text(translation),
            explanations=list(explanations or []),
            examples=list(examples or []),
            source_app=(source_app or "").strip(),
            source_context=self._normalize_text(source_context),
            metadata=metadata or {},
        )
        await self._save_json(entry)
        return entry

    async def capture_entry(
        self,
        source_text: str,
        normalized_text: str,
        language: str,
        entry_type: WordbookEntryType,
        translation: str,
        explanations: Optional[list[str]] = None,
        examples: Optional[list[str]] = None,
        source_app: str = "",
        source_context: str = "",
        metadata: Optional[dict] = None,
    ) -> WordbookEntry:
        """写入条目；命中重复时只累加查询次数。"""

        resolved_text = self._normalize_text(normalized_text or source_text)
        duplicate = await self._find_duplicate_entry(resolved_text, language or "unknown", entry_type)
        if duplicate:
            duplicate.query_count = max(1, int(duplicate.query_count or 1)) + 1
            duplicate.source_text = self._normalize_text(source_text) or duplicate.source_text
            duplicate.translation = self._normalize_text(translation) or duplicate.translation
            if explanations:
                duplicate.explanations = list(explanations)
            if examples:
                duplicate.examples = list(examples)
            if source_app:
                duplicate.source_app = source_app.strip()
            if source_context:
                duplicate.source_context = self._normalize_text(source_context)
            if metadata:
                duplicate.metadata = {
                    **(duplicate.metadata or {}),
                    **metadata,
                }
            await self._save_json(duplicate)
            return duplicate

        return await self.create_entry(
            source_text=source_text,
            normalized_text=resolved_text,
            language=language,
            entry_type=entry_type,
            translation=translation,
            explanations=explanations,
            examples=examples,
            source_app=source_app,
            source_context=source_context,
            metadata=metadata,
        )

    async def get_entry(self, entry_id: str) -> Optional[WordbookEntry]:
        """读取单条词条。"""

        data = await self._load_json(entry_id)
        return WordbookEntry.from_dict(data) if data else None

    async def list_entries(
        self,
        query: Optional[str] = None,
        entry_type: Optional[WordbookEntryType] = None,
        favorite_only: bool = False,
        limit: int = 100,
    ) -> list[WordbookEntry]:
        """列出生词条目。"""

        items: list[WordbookEntry] = []
        for json_file in self.wordbook_path.glob("*.json"):
            try:
                data = await self._load_json(json_file.stem)
                if data:
                    items.append(WordbookEntry.from_dict(data))
            except Exception:
                continue

        normalized_query = self._normalize_text(query or "").lower()

        def matches(item: WordbookEntry) -> bool:
            if favorite_only and not item.is_favorite:
                return False
            if entry_type and item.entry_type != entry_type:
                return False
            if not normalized_query:
                return True
            haystack = " ".join([
                item.source_text,
                item.normalized_text,
                item.translation,
                " ".join(item.explanations),
                item.source_context,
            ]).lower()
            return normalized_query in haystack

        filtered = [item for item in items if matches(item)]
        filtered.sort(key=lambda value: -value.updated_at.timestamp())
        return filtered[:limit]

    async def update_entry(
        self,
        entry_id: str,
        translation: Optional[str] = None,
        explanations: Optional[list[str]] = None,
        examples: Optional[list[str]] = None,
        is_favorite: Optional[bool] = None,
        metadata: Optional[dict] = None,
    ) -> Optional[WordbookEntry]:
        """更新已有条目。"""

        entry = await self.get_entry(entry_id)
        if not entry:
            return None

        if translation is not None:
            entry.translation = self._normalize_text(translation)
        if explanations is not None:
            entry.explanations = list(explanations)
        if examples is not None:
            entry.examples = list(examples)
        if is_favorite is not None:
            entry.is_favorite = is_favorite
        if metadata is not None:
            entry.metadata = metadata

        await self._save_json(entry)
        return entry

    async def get_stats(self) -> dict:
        """返回词库统计。"""

        items = await self.list_entries(limit=100000)
        today = datetime.now().date()
        return {
            "total": len(items),
            "today": sum(1 for item in items if item.created_at.date() == today),
            "favorites": sum(1 for item in items if item.is_favorite),
            "queried_total": sum(max(1, int(item.query_count or 1)) for item in items),
        }
