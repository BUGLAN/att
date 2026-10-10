"""记录列表的进程内索引：可删除、可重建的派生视图（**不使用任何数据库**）。

设计约束（task-9 第 4 项）：

- **源真相永远是 ``data/records/*.json``**：本模块只提供加速读取的派生视图，
  任何时刻删掉 ``data/.index/records.json`` 都不会丢数据，下次调用自动重建；
- **惰性装载 + 增量刷新**：首次访问时把全量记录装载进内存；之后每次刷新只用
  ``os.scandir`` + ``(mtime_ns, size)`` 比对，只有新增/变更的文件才会重新解析；
- **write-through**：API 写入路径调用 :meth:`RecordIndex.invalidate` 立即失效对应条目，
  后台任务等绕过 API 的写入由 mtime 比对兜底（下次 ``page()`` 自动发现）；
- **输出与 manager 完全一致**：对外返回的是 ``Record.from_dict(原始 JSON).to_dict()``，
  因此字段集合/顺序/类型都与 ``RecordManager.list_records()`` 路径保持一致；
- **失败不影响可用性**：本模块自身不吞异常，由 ``app.services.records_service``
  捕获后回落 manager 全目录扫描（接口绝不允许因为索引出错而 500）。
"""

from __future__ import annotations

import asyncio
import os
import threading
from dataclasses import dataclass
from datetime import datetime
from pathlib import Path
from typing import Optional

from app.records.models import Record, RecordStatus
from app.repositories.json_store import read_json_lenient, write_json_atomic
from app.utils.logging import get_logger

logger = get_logger(__name__)

# 清单格式版本：格式不兼容时直接忽略旧清单，重新由源 JSON 重建
MANIFEST_VERSION = 1
# 派生清单目录/文件名（位于 data/ 下，与 records/ 平级）
INDEX_DIR_NAME = ".index"
MANIFEST_FILE_NAME = "records.json"
# 单条记录文件名后缀
RECORD_FILE_SUFFIX = ".json"


@dataclass(slots=True)
class _IndexEntry:
    """一条记录的索引条目：内存中只保留原始 JSON + 排序/过滤所需的派生字段。"""

    record_id: str
    created_at: datetime
    status: str
    is_favorite: bool
    data: dict
    mtime_ns: int
    size: int

    def to_public_dict(self) -> dict:
        """按当前 Record 模型归一化输出（字段集合与 manager 路径一致）。"""
        return Record.from_dict(self.data).to_dict()


def _build_entry(
    record_id: str,
    data: Optional[dict],
    mtime_ns: int,
    size: int,
) -> Optional[_IndexEntry]:
    """把源 JSON 原始内容转成索引条目；损坏/非法数据返回 None（与 manager 一致地跳过）。"""
    if not isinstance(data, dict):
        return None

    try:
        record = Record.from_dict(data)
    except Exception as error:  # 损坏文件不能让整个列表接口失败，按 manager 一样跳过
        logger.warning(
            "记录索引跳过无法解析的记录文件",
            record_id=record_id,
            error=str(error),
        )
        return None

    return _IndexEntry(
        record_id=record.id,
        created_at=record.created_at,
        status=record.status.value,
        is_favorite=record.is_favorite,
        data=data,
        mtime_ns=mtime_ns,
        size=size,
    )


class RecordIndex:
    """单个 data 目录对应的记录索引实例（由 :func:`get_record_index` 缓存复用）。"""

    def __init__(self, data_path) -> None:
        self.data_path = Path(data_path)
        self.records_path = self.data_path / "records"
        self.manifest_path = self.data_path / INDEX_DIR_NAME / MANIFEST_FILE_NAME

        self._entries: dict[str, _IndexEntry] = {}
        # 清单里带回来的条目：mtime/size 校验通过前不视为可信，但仍可用于 diff
        self._loaded = False
        # 清单是否需要重写（内存索引已变化）
        self._manifest_stale = True
        self._lock = asyncio.Lock()

    # ==================== 公开 API ====================

    async def refresh(self) -> None:
        """增量刷新索引：scandir + (mtime_ns, size) 比对，只重读变化的文件。"""
        async with self._lock:
            await self._refresh_locked()

    async def page(
        self,
        *,
        limit: int,
        offset: int = 0,
        status: Optional[RecordStatus] = None,
        favorite_only: bool = False,
    ) -> tuple[list[dict], int]:
        """按创建时间倒序分页。

        Returns:
            (记录 dict 列表, 过滤后的总数)。总数用于校验索引与真实文件数一致，
            但 ``GET /api/records`` 的响应体仍是「裸数组」，不暴露 total。
        """
        # 刷新与「取快照」在同一把锁内完成：刷新会在工作线程里改写 _entries，
        # 必须先把页面切片固定成列表，再在锁外做 (可能较慢的) 模型归一化。
        async with self._lock:
            await self._refresh_locked()
            entries = self._filtered_entries(status=status, favorite_only=favorite_only)
            total = len(entries)
            if limit is None or limit < 0:
                window = entries[offset:]
            else:
                window = entries[offset:offset + limit]

        return [entry.to_public_dict() for entry in window], total

    def invalidate(self, record_id: Optional[str] = None) -> None:
        """写入路径的 write-through 失效：下次刷新会重读该文件（record_id=None 时全量重读）。"""
        if record_id is None:
            self._entries.clear()
            self._loaded = False
        else:
            self._entries.pop(record_id, None)
        # 标记清单待重写，并让下一次刷新必须真正执行
        self._manifest_stale = True

    # ==================== 内部实现 ====================

    def _filtered_entries(
        self,
        *,
        status: Optional[RecordStatus],
        favorite_only: bool,
    ) -> list[_IndexEntry]:
        """过滤 + 按 created_at 倒序排序（与 RecordManager.list_records 的排序键一致）。"""
        wanted_status: Optional[str] = None
        if status is not None:
            wanted_status = status.value if isinstance(status, RecordStatus) else str(status)

        entries = [
            entry
            for entry in self._entries.values()
            if (wanted_status is None or entry.status == wanted_status)
            and (not favorite_only or entry.is_favorite)
        ]
        entries.sort(key=lambda entry: entry.created_at, reverse=True)
        return entries

    async def _refresh_locked(self) -> None:
        """持锁的刷新流程（调用方不得重复持锁）。"""
        if not self._loaded:
            # 惰性装载：先尝试用派生清单做种子，失败就当作冷启动
            await asyncio.to_thread(self._load_manifest)

        changed, removed, scanned_count = await asyncio.to_thread(self._incremental_scan)

        if removed:
            for record_id in removed:
                self._entries.pop(record_id, None)

        self._loaded = True

        if changed or self._manifest_stale:
            self._manifest_stale = False
            await asyncio.to_thread(self._write_manifest)

        if changed or removed:
            logger.info(
                "记录索引增量刷新完成",
                records=len(self._entries),
                scanned=scanned_count,
                removed=len(removed),
            )

    def _incremental_scan(self) -> tuple[bool, set[str], int]:
        """线程内执行：scandir 源目录，只解析新增/变化的记录文件。

        Returns:
            (是否有新增/变更, 已删除的 record_id 集合, 扫描到的文件数)
        """
        records_path = self.records_path
        if not records_path.is_dir():
            # 源目录不存在（首次运行/被清空）：索引清空，但不算异常
            removed = set(self._entries)
            return bool(removed), removed, 0

        updates: dict[str, _IndexEntry] = {}
        seen: set[str] = set()

        with os.scandir(records_path) as iterator:
            for dir_entry in iterator:
                name = dir_entry.name
                if not name.endswith(RECORD_FILE_SUFFIX):
                    continue

                try:
                    if not dir_entry.is_file():
                        continue
                    stat_result = dir_entry.stat()
                except OSError as error:
                    # 文件在扫描过程中被删除/占用：跳过本次，下次刷新再来
                    logger.warning("记录索引扫描跳过不可读文件", path=name, error=str(error))
                    continue

                record_id = name[: -len(RECORD_FILE_SUFFIX)]
                seen.add(record_id)

                cached = self._entries.get(record_id)
                if (
                    cached is not None
                    and cached.mtime_ns == stat_result.st_mtime_ns
                    and cached.size == stat_result.st_size
                ):
                    # 未变化：直接复用内存里的原始 JSON，不读盘
                    continue

                data = read_json_lenient(dir_entry.path, None)
                entry = _build_entry(record_id, data, stat_result.st_mtime_ns, stat_result.st_size)
                if entry is None:
                    continue
                updates[record_id] = entry

        removed = set(self._entries) - seen
        for record_id, entry in updates.items():
            self._entries[record_id] = entry

        changed = bool(updates)
        return changed, removed, len(seen)

    def _load_manifest(self) -> None:
        """读取派生清单作为内存种子（清单不可信时静默忽略）。"""
        try:
            manifest = read_json_lenient(self.manifest_path, None)
        except Exception as error:  # 理论上是宽容读，仍兜一层
            logger.warning("读取记录索引清单失败", error=str(error))
            return

        if not isinstance(manifest, dict) or manifest.get("version") != MANIFEST_VERSION:
            return
        if manifest.get("records_path") != str(self.records_path):
            return

        raw_entries = manifest.get("entries")
        if not isinstance(raw_entries, dict):
            return

        restored = 0
        for record_id, payload in raw_entries.items():
            if not isinstance(payload, dict):
                continue
            entry = _build_entry(
                record_id,
                payload.get("data"),
                int(payload.get("mtime_ns") or 0),
                int(payload.get("size") or 0),
            )
            if entry is not None:
                self._entries[record_id] = entry
                restored += 1

        if restored:
            logger.info("记录索引清单装载完成", records=restored)

    def _write_manifest(self) -> None:
        """把内存索引写成可删除的派生清单（失败只记 warning，不影响列表接口）。"""
        payload = {
            "version": MANIFEST_VERSION,
            "records_path": str(self.records_path),
            "generated_at": datetime.now().isoformat(),
            "entries": {
                record_id: {
                    "mtime_ns": entry.mtime_ns,
                    "size": entry.size,
                    "data": entry.data,
                }
                for record_id, entry in self._entries.items()
            },
        }

        try:
            write_json_atomic(self.manifest_path, payload)
        except Exception as error:
            logger.warning(
                "写入记录索引清单失败（不影响查询）",
                path=str(self.manifest_path),
                error=str(error),
            )


# 按 data 目录缓存索引实例：同一进程内同一目录只保留一份内存索引
_INDEXES: dict[str, RecordIndex] = {}
_INDEXES_LOCK = threading.Lock()


def get_record_index(data_path) -> RecordIndex:
    """取得指定 data 目录的记录索引（同一路径复用同一实例）。"""
    key = str(Path(data_path).resolve())
    with _INDEXES_LOCK:
        index = _INDEXES.get(key)
        if index is None:
            index = RecordIndex(data_path)
            _INDEXES[key] = index
        return index


def reset_record_indexes() -> None:
    """清空索引注册表（仅供测试隔离使用）。"""
    with _INDEXES_LOCK:
        _INDEXES.clear()
