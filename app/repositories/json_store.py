"""JSON 文件存储基础设施：原子写、容错读、按资源路径的进程内锁。

设计约束（硬要求，见审计报告 §3.5）：
- **不使用任何数据库**：单个 JSON 文件始终是唯一真相源，本模块只提供落盘与读取能力；
- 落盘统一走 :func:`write_json_atomic` / :func:`write_json_atomic_async`（同目录 ``.tmp`` + ``os.replace``），
  禁止再出现「原地截断写」；
- 写盘格式固定为 ``indent=2`` + ``ensure_ascii=False``，保证 ``data/`` 目录可直接阅读、diff 与整体迁移；
- 锁注册表按资源路径划分：同一资源只有一把锁，串行化「读-改-写」，避免静默丢更新。
"""

from __future__ import annotations

import asyncio
import json
import os
import threading
import weakref
from pathlib import Path
from typing import Any, Dict, Optional, Union

PathLike = Union[str, os.PathLike]

# 统一的缩进：保持文件可读、可手改、可 git diff（"方便迁移"的物理基础）。
JSON_INDENT = 2

# asyncio.Lock 注册表：外层按事件循环弱引用缓存，内层按规范化的资源路径缓存。
# 用弱引用是为了避免 asyncio.run() 反复创建的事件循环被字典长期持有。
_async_locks: "weakref.WeakKeyDictionary[asyncio.AbstractEventLoop, Dict[str, asyncio.Lock]]" = (
    weakref.WeakKeyDictionary()
)
_async_locks_guard = threading.Lock()

# 同步代码（如 pydantic Settings 实例、非 async 的管理类方法）使用 threading.RLock。
# 使用可重入锁，允许同一个调用链里嵌套进入同一资源的临界区。
_thread_locks: Dict[str, threading.RLock] = {}
_thread_locks_guard = threading.Lock()


def _normalize_key(key: PathLike) -> str:
    """把资源路径规范化为锁字典的键（不要求文件已存在）。"""
    try:
        return str(Path(key).resolve())
    except OSError:
        return str(Path(key))


def get_async_lock(key: PathLike) -> asyncio.Lock:
    """获取「当前事件循环 + 指定资源路径」唯一对应的 ``asyncio.Lock``。

    必须在协程内调用（依赖 :func:`asyncio.get_running_loop`）。
    注意 ``asyncio.Lock`` 不可重入：持锁期间不要再次进入同一资源路径的加锁分支，
    内部实现请拆分为不带锁的私有方法。
    """

    loop = asyncio.get_running_loop()
    normalized = _normalize_key(key)

    with _async_locks_guard:
        locks = _async_locks.get(loop)
        if locks is None:
            locks = {}
            _async_locks[loop] = locks

        lock = locks.get(normalized)
        if lock is None:
            lock = asyncio.Lock()
            locks[normalized] = lock

        return lock


def get_thread_lock(key: PathLike) -> threading.RLock:
    """获取「指定资源路径」唯一对应的可重入线程锁（供同步代码使用）。"""

    normalized = _normalize_key(key)

    with _thread_locks_guard:
        lock = _thread_locks.get(normalized)
        if lock is None:
            lock = threading.RLock()
            _thread_locks[normalized] = lock
        return lock


def write_json_atomic(
    path: PathLike,
    data: Any,
    *,
    indent: int = JSON_INDENT,
    default: Optional[Any] = None,
) -> None:
    """原子写 JSON：先写同目录临时文件，再 ``os.replace`` 覆盖目标文件。

    任何一步失败都会清理临时文件并向上抛出异常，因此目标文件要么是旧内容、
    要么是完整的新内容，不会出现半截 JSON。
    """

    target = Path(path)
    target.parent.mkdir(parents=True, exist_ok=True)
    tmp_path = target.with_name(target.name + ".tmp")

    # 先序列化再开文件：序列化失败时不会碰到磁盘上的任何文件。
    payload = json.dumps(data, ensure_ascii=False, indent=indent, default=default)

    try:
        with open(tmp_path, "w", encoding="utf-8") as handle:
            handle.write(payload)
            handle.flush()
            os.fsync(handle.fileno())
        # Windows 同卷内 os.replace 是原子操作
        os.replace(tmp_path, target)
    except BaseException:
        try:
            if tmp_path.exists():
                tmp_path.unlink()
        except OSError:
            pass
        raise


async def write_json_atomic_async(
    path: PathLike,
    data: Any,
    *,
    indent: int = JSON_INDENT,
    default: Optional[Any] = None,
) -> None:
    """``write_json_atomic`` 的异步包装：落盘放到线程池，避免阻塞事件循环。"""

    await asyncio.to_thread(write_json_atomic, path, data, indent=indent, default=default)


def read_json(path: PathLike, default: Optional[Any] = None) -> Any:
    """严格读 JSON：文件不存在返回 ``default``，内容非法则抛出异常。"""

    target = Path(path)
    if not target.exists():
        return default

    with open(target, "r", encoding="utf-8") as handle:
        return json.load(handle)


def read_json_lenient(path: PathLike, default: Optional[Any] = None) -> Any:
    """宽容读 JSON：文件缺失、内容损坏时一律返回 ``default``，绝不抛异常。

    用于「读不到就必须能继续跑」的场景（如启动期读取配置文件）。
    """

    try:
        value = read_json(path, default=default)
    except (OSError, ValueError, TypeError):
        # json.JSONDecodeError 是 ValueError 的子类
        return default

    if value is None:
        return default
    return value


async def read_json_async(path: PathLike, default: Optional[Any] = None) -> Any:
    """``read_json`` 的异步包装。"""

    return await asyncio.to_thread(read_json, path, default)


async def read_json_lenient_async(path: PathLike, default: Optional[Any] = None) -> Any:
    """``read_json_lenient`` 的异步包装。"""

    return await asyncio.to_thread(read_json_lenient, path, default)


def ensure_dict(value: Any, *, logger: Optional[Any] = None, source: Optional[PathLike] = None) -> dict:
    """把「非 dict 的 JSON 顶层形状」统一收敛成空字典。

    典型场景：``settings.json`` 内容是 ``null`` / ``[]`` / ``"str"`` 时，
    旧实现会在 ``{**default, **loaded}`` 处抛 ``TypeError``，导致后端在 import 期直接起不来。
    """

    if isinstance(value, dict):
        return value

    if value is not None and logger is not None:
        # 使用 %-风格参数，兼容标准库 logging 与 structlog
        logger.warning(
            "JSON 顶层不是对象（实际类型 %s），已按空数据处理：%s",
            type(value).__name__,
            source,
        )

    return {}
