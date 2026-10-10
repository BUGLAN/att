"""
Clipboard manager for JSON-based storage.
"""
import hashlib
import logging
import re
from datetime import datetime
from pathlib import Path
from typing import List, Optional

from app.clipboard.models import ClipboardItem, ClipboardItemType
from app.repositories.json_store import (
    get_async_lock,
    read_json_lenient_async,
    write_json_atomic_async,
)
from app.repositories.paths import is_safe_identifier, is_within_directory


logger = logging.getLogger(__name__)

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
        self.images_path.mkdir(parents=True, exist_ok=True)

    @property
    def images_path(self) -> Path:
        """剪贴板图片的唯一合法目录：data/clipboard/images/。"""
        return self.clipboard_path / "images"

    def _get_item_file(self, item_id: str) -> Path:
        """条目文件路径（ID 必须是单层安全文件名）。"""
        if not is_safe_identifier(item_id):
            raise ValueError(f"非法 clipboard item_id（含路径分隔符或穿越写法）：{item_id!r}")
        return self.clipboard_path / f"{item_id}.json"

    async def _load_json(self, item_id: str) -> Optional[dict]:
        """读取条目 JSON；非法 ID 或损坏文件按不存在处理。"""
        try:
            file_path = self._get_item_file(item_id)
        except ValueError as error:
            logger.warning("拒绝非法 clipboard item_id: %s", error)
            return None

        if not file_path.exists():
            return None

        data = await read_json_lenient_async(file_path, default=None)
        if data is None:
            logger.warning("剪贴板条目文件无法解析，按不存在处理: %s", file_path)
            return None
        return data

    async def _save_json(self, item: ClipboardItem) -> None:
        """原子写条目 JSON（调用方需自行持有资源锁）。"""
        item.updated_at = datetime.now()
        await write_json_atomic_async(self._get_item_file(item.id), item.to_dict())

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

    def _iter_image_path_candidates(self, metadata: Optional[dict]) -> List[Path]:
        """收集候选图片路径，并只保留解析后位于 data/clipboard/images/ 内的路径。

        审计报告 §4.1 缺陷 1：旧实现直接 ``Path(raw_image_path)``，
        传绝对路径即可借图片接口读取 .env / 任意日志。现在所有候选都必须通过
        目录包含性校验，越界（绝对路径、``..`` 穿越）一律拒绝并记 warning。
        """
        source = metadata or {}
        raw_image_path = str(source.get("image_path", "")).strip()
        stored_file_name = str(source.get("stored_file_name", "")).strip()

        candidates: List[Path] = []

        if raw_image_path:
            candidates.extend(self._expand_image_candidates(raw_image_path))

        if stored_file_name:
            candidates.append(self.images_path / Path(stored_file_name))

        safe_candidates: List[Path] = []
        for candidate in candidates:
            if is_within_directory(candidate, self.images_path):
                safe_candidates.append(candidate)
            else:
                logger.warning("拒绝越界的剪贴板图片路径: %s", candidate)

        return safe_candidates

    def _expand_image_candidates(self, raw_image_path: str) -> List[Path]:
        """展开历史多种写法（相对 / 绝对 / 带 data 前缀 / 纯文件名）。

        展开只负责「猜出可能的位置」，是否合法由 :meth:`_iter_image_path_candidates`
        的包含性校验统一裁决。
        """
        normalized = raw_image_path.replace("\\", "/").strip()
        candidates: List[Path] = []

        if Path(normalized).is_absolute():
            # 绝对路径仅作为候选：必须落在本进程的 data/clipboard/images/ 内才被采用
            candidates.append(Path(raw_image_path).expanduser())
        else:
            candidates.append(self.clipboard_path / Path(normalized))

        # 任意前缀（data/、/app/data/、旧机器绝对路径）统一重定位到当前 images 目录
        marker = "clipboard/images/"
        marker_index = normalized.lower().find(marker)
        if marker_index >= 0:
            candidates.append(self.images_path / Path(normalized[marker_index + len(marker):]))
        elif normalized.lower().startswith("images/"):
            candidates.append(self.images_path / Path(normalized[len("images/"):]))
        elif normalized and "/" not in normalized:
            candidates.append(self.images_path / Path(normalized))

        return candidates

    def _build_item(
        self,
        content: str,
        source: str,
        item_type: Optional[ClipboardItemType],
        title: Optional[str],
        summary: Optional[str],
        metadata: Optional[dict]
    ) -> ClipboardItem:
        """纯构造条目对象（不落盘），供 create_item / capture_item 复用。"""
        normalized = self._normalize_text(content)
        resolved_type = item_type or self._detect_item_type(normalized)
        content_hash = self._build_content_hash(normalized)
        return ClipboardItem(
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

    async def create_item(
        self,
        content: str,
        source: str = "manual",
        item_type: Optional[ClipboardItemType] = None,
        title: Optional[str] = None,
        summary: Optional[str] = None,
        metadata: Optional[dict] = None
    ) -> ClipboardItem:
        async with get_async_lock(self.clipboard_path):
            item = self._build_item(content, source, item_type, title, summary, metadata)
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

        # 「查重 + 写入」整体持锁：避免并发捕获时重复建条目或互相覆盖计数
        async with get_async_lock(self.clipboard_path):
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

            item = self._build_item(normalized, source, item_type, title, summary, metadata)
            await self._save_json(item)
            return item

    async def get_item(self, item_id: str) -> Optional[ClipboardItem]:
        data = await self._load_json(item_id)
        return ClipboardItem.from_dict(data) if data else None

    async def get_item_image_path(self, item_id: str) -> Optional[Path]:
        item = await self.get_item(item_id)
        if not item or item.item_type != ClipboardItemType.IMAGE:
            return None

        for image_path in self._iter_image_path_candidates(item.metadata):
            if image_path.exists() and image_path.is_file():
                return image_path

        return None

    def is_allowed_image_path(self, image_path: Path) -> bool:
        """公开的只读校验：图片路径必须位于 data/clipboard/images/ 内。"""
        return is_within_directory(image_path, self.images_path)

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
        # 读-改-写整体持锁
        async with get_async_lock(self.clipboard_path):
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
        """删除条目；item_id 必须是单层安全文件名，越界一律拒绝。"""
        try:
            file_path = self._get_item_file(item_id)
        except ValueError as error:
            logger.warning("拒绝越界删除剪贴板条目: %s", error)
            return False

        async with get_async_lock(self.clipboard_path):
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
