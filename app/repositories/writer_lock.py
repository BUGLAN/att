"""单写者约束：``data/.writer.lock`` + 心跳 + 陈旧接管。

背景（审计报告 §3.5 ②）：JSON 文件存储没有跨进程互斥能力，
原生应用与 Docker 容器共用同一份 ``data/`` 时会互相覆盖、静默丢数据。

这里不做分布式锁，而是用「单写者」约束：启动时抢占锁文件，持有期间周期性刷新心跳；
发现第二个写者时**直接报错**，把「静默丢数据」变成「明确的启动失败」。

只使用标准库实现，不引入任何第三方依赖。
"""

from __future__ import annotations

import os
import socket
import threading
import time
from dataclasses import asdict, dataclass
from datetime import datetime
from pathlib import Path
from typing import Optional

from app.repositories.json_store import read_json_lenient, write_json_atomic

# 锁文件名
WRITER_LOCK_FILENAME = ".writer.lock"

# 心跳超过该秒数即视为陈旧（可被接管）
DEFAULT_STALE_AFTER_SECONDS = 60.0

# 心跳刷新间隔
DEFAULT_HEARTBEAT_INTERVAL_SECONDS = 15.0

# 显式允许多写者的环境变量（迁移 / 调试用）
ALLOW_MULTI_WRITER_ENV = "ATT_ALLOW_MULTI_WRITER"

_ALLOW_MULTI_WRITER_VALUES = {"1", "true", "yes", "on"}

_logger: Optional[object] = None


def _get_logger():
    """延迟获取日志器，避免与 app.config 形成 import 环。"""

    global _logger
    if _logger is None:
        import logging

        _logger = logging.getLogger(__name__)
    return _logger


def _now_timestamp() -> float:
    return time.time()


def _to_iso(timestamp: float) -> str:
    return datetime.fromtimestamp(timestamp).isoformat()


def _parse_timestamp(value: object, fallback: float) -> float:
    """把锁文件里的 ISO 时间字符串解析成时间戳，解析不了就用兜底值。"""

    if isinstance(value, (int, float)):
        return float(value)

    if isinstance(value, str) and value:
        try:
            return datetime.fromisoformat(value).timestamp()
        except ValueError:
            return fallback

    return fallback


def allow_multi_writer() -> bool:
    """``ATT_ALLOW_MULTI_WRITER=1`` 时跳过单写者约束。"""

    value = str(os.getenv(ALLOW_MULTI_WRITER_ENV, "")).strip().lower()
    return value in _ALLOW_MULTI_WRITER_VALUES


def _is_process_alive(pid: int) -> bool:
    """判断 PID 是否仍在运行（仅标准库）。"""

    if pid <= 0:
        return False

    if pid == os.getpid():
        return True

    if os.name == "nt":
        return _is_process_alive_windows(pid)

    try:
        os.kill(pid, 0)
    except ProcessLookupError:
        return False
    except PermissionError:
        return True
    except OSError:
        return False

    return True


def _is_process_alive_windows(pid: int) -> bool:
    """Windows 下不能用 ``os.kill(pid, 0)``（会真的结束目标进程），改用 OpenProcess 查询。"""

    import ctypes
    from ctypes import wintypes

    process_query_limited_information = 0x1000
    still_active = 259

    kernel32 = ctypes.windll.kernel32  # type: ignore[attr-defined]
    handle = kernel32.OpenProcess(process_query_limited_information, False, pid)
    if not handle:
        return False

    try:
        exit_code = wintypes.DWORD()
        if not kernel32.GetExitCodeProcess(handle, ctypes.byref(exit_code)):
            return False
        return exit_code.value == still_active
    finally:
        kernel32.CloseHandle(handle)


@dataclass
class WriterLockInfo:
    """锁文件内容：谁是写者、什么时候启动、心跳到哪一刻。"""

    pid: int
    host: str
    started_at: str
    heartbeat_at: str
    port: Optional[int] = None
    version: int = 1

    # 仅内存字段：本次「获取」是环境变量旁路的结果，不落盘
    bypassed: bool = False

    def to_dict(self) -> dict:
        """序列化为锁文件内容（不含内存字段）。"""

        payload = asdict(self)
        payload.pop("bypassed", None)
        return payload

    @property
    def started_timestamp(self) -> float:
        return _parse_timestamp(self.started_at, 0.0)

    @property
    def heartbeat_timestamp(self) -> float:
        return _parse_timestamp(self.heartbeat_at, 0.0)

    @classmethod
    def from_dict(cls, data: object) -> Optional["WriterLockInfo"]:
        """从锁文件内容恢复；形状非法时返回 ``None``（按无锁处理）。"""

        if not isinstance(data, dict):
            return None

        try:
            pid = int(data.get("pid"))
        except (TypeError, ValueError):
            return None

        if pid <= 0:
            return None

        host = data.get("host")
        if not isinstance(host, str) or not host:
            host = "unknown"

        port_value = data.get("port")
        try:
            port = int(port_value) if port_value is not None else None
        except (TypeError, ValueError):
            port = None

        version_value = data.get("version", 1)
        try:
            version = int(version_value)
        except (TypeError, ValueError):
            version = 1

        now = _now_timestamp()
        return cls(
            pid=pid,
            host=host,
            started_at=str(data.get("started_at") or _to_iso(now)),
            heartbeat_at=str(data.get("heartbeat_at") or _to_iso(now)),
            port=port,
            version=version,
        )


class WriterLock:
    """基于锁文件的单写者约束实现。

    生命周期：``acquire()`` → （可选 ``heartbeat()``）→ ``release()``。
    冲突时抛 :class:`RuntimeError`，错误信息为中文，直接说明该关掉哪个进程或如何旁路。
    """

    def __init__(
        self,
        lock_path: Path,
        *,
        stale_after: float = DEFAULT_STALE_AFTER_SECONDS,
        heartbeat_interval: float = DEFAULT_HEARTBEAT_INTERVAL_SECONDS,
        auto_heartbeat: bool = True,
    ):
        self.lock_path = Path(lock_path)
        self.stale_after = float(stale_after)
        self.heartbeat_interval = float(heartbeat_interval)
        self.auto_heartbeat = bool(auto_heartbeat)

        self._mutex = threading.RLock()
        self._owned = False
        self._bypassed = False
        self._info: Optional[WriterLockInfo] = None
        self._stop_event = threading.Event()
        self._heartbeat_thread: Optional[threading.Thread] = None

    # ------------------------------------------------------------------ 查询

    @property
    def owned(self) -> bool:
        """当前实例是否持有锁（旁路时不算持有）。"""

        return self._owned

    @property
    def bypassed(self) -> bool:
        """本次获取是否走了环境变量旁路。"""

        return self._bypassed

    def read_info(self) -> Optional[WriterLockInfo]:
        """读取当前锁文件内容（不存在或非法时返回 ``None``）。"""

        return WriterLockInfo.from_dict(read_json_lenient(self.lock_path, default=None))

    # ------------------------------------------------------------------ 抢占

    def acquire(self, *, port: Optional[int] = None) -> WriterLockInfo:
        """抢占写者锁；失败抛中文 ``RuntimeError``。

        :param port: 可选的监听端口，仅用于排查时区分实例。
        """

        with self._mutex:
            if self._owned or self._bypassed:
                return self._info or self._build_info(port=port, bypassed=self._bypassed)

            if allow_multi_writer():
                # 旁路：不写锁文件，避免覆盖真正的持有者
                info = self._build_info(port=port, bypassed=True)
                self._bypassed = True
                self._info = info
                _get_logger().warning(
                    "已通过 %s 跳过单写者约束，多写者并发可能导致数据覆盖：%s",
                    ALLOW_MULTI_WRITER_ENV,
                    self.lock_path,
                )
                return info

            existing = self.read_info()
            if existing is not None:
                stale, reason = self._evaluate_staleness(existing)
                if not stale:
                    raise RuntimeError(self._build_conflict_message(existing))
                _get_logger().warning(
                    "检测到陈旧写者锁并接管（%s）：%s", reason, self.lock_path
                )

            info = self._build_info(port=port, bypassed=False)
            write_json_atomic(self.lock_path, info.to_dict())

            self._info = info
            self._owned = True

            if self.auto_heartbeat:
                self._start_heartbeat()

            _get_logger().info(
                "已获取单写者锁：%s（pid=%s, host=%s）", self.lock_path, info.pid, info.host
            )
            return info

    # ------------------------------------------------------------------ 心跳

    def heartbeat(self) -> None:
        """刷新一次心跳时间戳（自动心跳线程会周期调用）。"""

        with self._mutex:
            if not self._owned or self._info is None:
                return

            self._info.heartbeat_at = _to_iso(_now_timestamp())
            write_json_atomic(self.lock_path, self._info.to_dict())

    # ------------------------------------------------------------------ 释放

    def release(self) -> None:
        """释放锁：停止心跳并删除锁文件（仅当锁仍属于本进程）。"""

        with self._mutex:
            self._stop_event.set()
            thread = self._heartbeat_thread
            self._heartbeat_thread = None

            owned = self._owned
            bypassed = self._bypassed
            self._owned = False
            self._bypassed = False
            self._info = None

        if thread is not None and thread.is_alive():
            thread.join(timeout=2.0)

        if not owned or bypassed:
            return

        current = self.read_info()
        if current is not None and current.pid != os.getpid():
            _get_logger().warning("锁文件已被其他进程接管，跳过删除：%s", self.lock_path)
            return

        try:
            self.lock_path.unlink()
        except FileNotFoundError:
            return
        except OSError as error:  # pragma: no cover - 权限异常极少见
            _get_logger().warning("删除写者锁失败：%s（%s）", self.lock_path, error)
            return

        _get_logger().info("已释放单写者锁：%s", self.lock_path)

    # ------------------------------------------------------------------ 内部

    def _build_info(self, *, port: Optional[int], bypassed: bool) -> WriterLockInfo:
        now = _now_timestamp()
        return WriterLockInfo(
            pid=os.getpid(),
            host=socket.gethostname() or "unknown",
            started_at=_to_iso(now),
            heartbeat_at=_to_iso(now),
            port=port,
            bypassed=bypassed,
        )

    def _evaluate_staleness(self, info: WriterLockInfo) -> tuple[bool, str]:
        """判断已有锁是否陈旧：心跳过期，或同机进程已不存在。"""

        heartbeat_age = _now_timestamp() - info.heartbeat_timestamp
        if heartbeat_age > self.stale_after:
            return True, f"心跳已过期 {heartbeat_age:.1f}s（阈值 {self.stale_after:.0f}s）"

        if info.host == (socket.gethostname() or "unknown") and not _is_process_alive(info.pid):
            return True, f"原持有进程 {info.pid} 已不存在"

        return False, ""

    def _build_conflict_message(self, info: WriterLockInfo) -> str:
        return (
            "检测到另一个 ATT 后端进程正在写同一份数据目录，已拒绝启动："
            f"pid={info.pid}、host={info.host}、started_at={info.started_at}、heartbeat_at={info.heartbeat_at}。"
            "JSON 文件存储不支持多进程同时写入（会静默丢数据），请先关闭另一个后端进程"
            f"（Docker 容器或原生应用），或设置环境变量 {ALLOW_MULTI_WRITER_ENV}=1 显式允许多写者。"
        )

    def _start_heartbeat(self) -> None:
        self._stop_event.clear()
        thread = threading.Thread(
            target=self._heartbeat_loop,
            name="att-writer-lock-heartbeat",
            daemon=True,
        )
        self._heartbeat_thread = thread
        thread.start()

    def _heartbeat_loop(self) -> None:
        while not self._stop_event.wait(self.heartbeat_interval):
            try:
                self.heartbeat()
            except Exception as error:  # pragma: no cover - 磁盘异常时只记录不崩
                _get_logger().warning("写者锁心跳刷新失败：%s", error)


# ---------------------------------------------------------------------- 单例

_writer_lock: Optional[WriterLock] = None
_writer_lock_guard = threading.Lock()


def get_writer_lock(data_path: Optional[Path] = None) -> WriterLock:
    """获取进程级写者锁实例（按 data 目录区分）。"""

    global _writer_lock

    if data_path is None:
        from app.config import settings

        data_path = settings.data_path

    lock_path = Path(data_path) / WRITER_LOCK_FILENAME

    with _writer_lock_guard:
        if _writer_lock is None or _writer_lock.lock_path != lock_path:
            _writer_lock = WriterLock(lock_path)
        return _writer_lock


def acquire_writer_lock(data_path: Optional[Path] = None, *, port: Optional[int] = None) -> WriterLockInfo:
    """抢占单写者锁并返回锁信息；冲突时抛中文 ``RuntimeError``。"""

    return get_writer_lock(data_path).acquire(port=port)


def release_writer_lock() -> None:
    """释放进程级写者锁（应用 shutdown 时调用）。"""

    global _writer_lock

    with _writer_lock_guard:
        lock = _writer_lock
        _writer_lock = None

    if lock is not None:
        lock.release()
