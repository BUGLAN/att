"""记录列表查询编排：优先进程内索引，异常时回落原来的 manager 全目录扫描。

拆分自 `app/api/routes.py` 的 `list_records`：

- 索引（``app/repositories/record_index.py``）只是**派生视图**，任何异常都必须
  被这里吞掉并回落 manager 扫描，保证 ``GET /api/records`` 不因索引问题返回 500；
- 回落路径必须与重构前行为一致：``list_records(limit=limit+offset)[offset:]``，
  在 offset=0 时与原实现逐字节等价。
"""

from __future__ import annotations

from pathlib import Path
from typing import Optional, Sequence

from app.records.manager import RecordManager
from app.records.models import RecordStatus
from app.repositories.record_index import RecordIndex, get_record_index
from app.utils.logging import get_logger

logger = get_logger(__name__)


def resolve_record_index(record_manager) -> Optional[RecordIndex]:
    """从 RecordManager 推导 data 目录并取得索引实例；无法推导时返回 None。"""
    if record_manager is None:
        return None

    data_path = getattr(record_manager, "data_path", None)
    if data_path is None:
        return None

    try:
        return get_record_index(Path(data_path))
    except Exception as error:
        logger.warning("记录索引初始化失败，将回落全目录扫描", error=str(error), exc_info=True)
        return None


async def list_record_page(
    *,
    record_manager: RecordManager,
    status: Optional[RecordStatus],
    limit: int,
    offset: int,
) -> list[dict]:
    """返回记录字典列表（按创建时间倒序，已应用 limit/offset/status）。"""
    index = resolve_record_index(record_manager)

    if index is not None:
        try:
            items, _total = await index.page(
                limit=limit,
                offset=offset,
                status=status,
            )
            return items
        except Exception as error:
            # 索引是派生数据：出错只能降级，不能让列表接口失败
            logger.warning(
                "记录索引不可用，回落 RecordManager 全目录扫描",
                error=str(error),
                exc_info=True,
            )
            invalidate_record_index(record_manager)

    return await _list_records_via_manager(
        record_manager=record_manager,
        status=status,
        limit=limit,
        offset=offset,
    )


async def _list_records_via_manager(
    *,
    record_manager: RecordManager,
    status: Optional[RecordStatus],
    limit: int,
    offset: int,
) -> list[dict]:
    """重构前的原始扫描路径（保留为索引不可用时的回落实现）。"""
    # manager 不支持 offset，多取 offset 条再切片即可保持排序语义不变
    fetch_limit = max(limit + offset, 0)
    records: Sequence = await record_manager.list_records(status=status, limit=fetch_limit)
    return [record.to_dict() for record in records[offset:offset + limit]]


def invalidate_record_index(record_manager, record_id: Optional[str] = None) -> None:
    """写入路径的 write-through：让索引立即失效（失败只记日志）。"""
    index = resolve_record_index(record_manager)
    if index is None:
        return

    try:
        index.invalidate(record_id)
    except Exception as error:
        logger.warning("记录索引失效操作失败（不影响写入）", error=str(error))
