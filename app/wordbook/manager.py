"""基于 JSON 的生词本管理器。"""
import logging
from datetime import datetime
from pathlib import Path
from typing import Optional

from app.repositories.json_store import (
    get_async_lock,
    read_json_lenient_async,
    write_json_atomic_async,
)
from app.repositories.paths import is_safe_identifier
from app.wordbook.models import WordbookEntry, WordbookEntryType

logger = logging.getLogger(__name__)


class WordbookManager:
    """管理生词本条目。"""

    def __init__(self, data_path: Path):
        self.data_path = data_path
        self.wordbook_path = data_path / "wordbook"

    async def initialize(self) -> None:
        """确保目录存在。"""

        self.wordbook_path.mkdir(parents=True, exist_ok=True)

    def _get_entry_file(self, entry_id: str) -> Path:
        """条目文件路径（ID 必须是单层安全文件名，防止路径穿越）。"""
        if not is_safe_identifier(entry_id):
            raise ValueError(f"非法 wordbook entry_id（含路径分隔符或穿越写法）：{entry_id!r}")
        return self.wordbook_path / f"{entry_id}.json"

    def _normalize_text(self, text: str) -> str:
        return (text or "").replace("\r\n", "\n").strip()

    async def _load_json(self, entry_id: str) -> Optional[dict]:
        """读取条目 JSON；非法 ID 或损坏文件按不存在处理。"""
        try:
            file_path = self._get_entry_file(entry_id)
        except ValueError as error:
            logger.warning("拒绝非法 wordbook entry_id: %s", error)
            return None

        if not file_path.exists():
            return None

        data = await read_json_lenient_async(file_path, default=None)
        if data is None:
            logger.warning("生词条目文件无法解析，按不存在处理: %s", file_path)
            return None
        return data

    async def _save_json(self, entry: WordbookEntry) -> None:
        """原子写条目 JSON（调用方需自行持有资源锁）。"""
        entry.updated_at = datetime.now()
        await write_json_atomic_async(self._get_entry_file(entry.id), entry.to_dict())

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

    def _build_entry(
        self,
        source_text: str,
        normalized_text: str,
        language: str,
        entry_type: WordbookEntryType,
        translation: str,
        explanations: Optional[list[str]],
        examples: Optional[list[str]],
        source_app: str,
        source_context: str,
        metadata: Optional[dict],
    ) -> WordbookEntry:
        """纯构造条目对象（不落盘）。"""

        return WordbookEntry(
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

        async with get_async_lock(self.wordbook_path):
            entry = self._build_entry(
                source_text,
                normalized_text,
                language,
                entry_type,
                translation,
                explanations,
                examples,
                source_app,
                source_context,
                metadata,
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

        # 「查重 + 写入」整体持锁
        async with get_async_lock(self.wordbook_path):
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

            entry = self._build_entry(
                source_text,
                resolved_text,
                language,
                entry_type,
                translation,
                explanations,
                examples,
                source_app,
                source_context,
                metadata,
            )
            await self._save_json(entry)
            return entry

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

        # 读-改-写整体持锁
        async with get_async_lock(self.wordbook_path):
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
