"""JSON 文件存储基础设施（不使用任何数据库）。

本包提供「唯一落盘点 + 进程内锁 + 单写者约束 + 路径安全校验」四类能力，
让 JSON 文件继续作为唯一真相源，保证 `data/` 目录可以整体搬迁。
"""

from app.repositories.json_store import (
    ensure_dict,
    get_async_lock,
    get_thread_lock,
    read_json,
    read_json_async,
    read_json_lenient,
    read_json_lenient_async,
    write_json_atomic,
    write_json_atomic_async,
)
from app.repositories.paths import is_safe_identifier, is_within_directory
from app.repositories.writer_lock import (
    ALLOW_MULTI_WRITER_ENV,
    WriterLock,
    WriterLockInfo,
    acquire_writer_lock,
    get_writer_lock,
    release_writer_lock,
)

__all__ = [
    "ALLOW_MULTI_WRITER_ENV",
    "WriterLock",
    "WriterLockInfo",
    "acquire_writer_lock",
    "ensure_dict",
    "get_async_lock",
    "get_thread_lock",
    "get_writer_lock",
    "is_safe_identifier",
    "is_within_directory",
    "read_json",
    "read_json_async",
    "read_json_lenient",
    "read_json_lenient_async",
    "release_writer_lock",
    "write_json_atomic",
    "write_json_atomic_async",
]
