"""单写者约束（data/.writer.lock）的回归测试。

覆盖任务要求的 5 个场景：
1. 新锁：空目录下可获取，锁文件字段完整；
2. 活锁拒绝：已有存活进程持有 → RuntimeError（中文提示，指出环境变量旁路）；
3. 陈旧接管：心跳过期，或同机 PID 已不存在 → 可接管；
4. 环境变量旁路：ATT_ALLOW_MULTI_WRITER=1 时跳过约束，且不覆盖原持有者的锁文件；
5. 正常释放：release() 删除锁文件，随后可被再次获取。
"""
import json
import os
import time
from datetime import datetime, timedelta
from pathlib import Path

import pytest

from app.repositories import writer_lock as writer_lock_module
from app.repositories.writer_lock import (
    ALLOW_MULTI_WRITER_ENV,
    WRITER_LOCK_FILENAME,
    WriterLock,
)


def _read_lock_file(lock_path: Path) -> dict:
    return json.loads(lock_path.read_text(encoding="utf-8"))


def _write_lock_file(lock_path: Path, *, pid: int, host: str, heartbeat_age_seconds: float) -> dict:
    now = datetime.now()
    payload = {
        "pid": pid,
        "host": host,
        "started_at": (now - timedelta(seconds=heartbeat_age_seconds + 30)).isoformat(),
        "heartbeat_at": (now - timedelta(seconds=heartbeat_age_seconds)).isoformat(),
        "port": None,
        "version": 1,
    }
    lock_path.parent.mkdir(parents=True, exist_ok=True)
    lock_path.write_text(json.dumps(payload, ensure_ascii=False, indent=2), encoding="utf-8")
    return payload


def _find_dead_pid() -> int:
    """找一个当前确定不存在的 PID（自校验，避免平台差异导致误判）。"""

    for candidate in (2**31 - 1, 2**31 - 2, 2**31 - 3, 999_999_999):
        if not writer_lock_module._is_process_alive(candidate):
            return candidate
    pytest.skip("无法构造“已不存在”的 PID")


# --------------------------------------------------------------------- 场景 1

def test_acquire_creates_lock_file_with_expected_fields(tmp_path: Path):
    lock_path = tmp_path / WRITER_LOCK_FILENAME
    lock = WriterLock(lock_path, auto_heartbeat=False)

    info = lock.acquire(port=8000)

    assert lock_path.exists()
    assert lock.owned is True
    assert info.pid == os.getpid()
    assert info.host
    assert info.port == 8000

    payload = _read_lock_file(lock_path)
    assert payload["pid"] == os.getpid()
    assert payload["host"]
    assert datetime.fromisoformat(payload["started_at"])
    assert datetime.fromisoformat(payload["heartbeat_at"])
    assert "bypassed" not in payload, "内存字段不应落盘"

    lock.release()
    assert not lock_path.exists()


# --------------------------------------------------------------------- 场景 2

def test_live_lock_is_rejected_with_chinese_message(tmp_path: Path):
    lock_path = tmp_path / WRITER_LOCK_FILENAME
    _write_lock_file(lock_path, pid=os.getpid(), host=writer_lock_module.socket.gethostname(), heartbeat_age_seconds=0)

    lock = WriterLock(lock_path, auto_heartbeat=False)

    with pytest.raises(RuntimeError) as error:
        lock.acquire()

    message = str(error.value)
    assert "拒绝启动" in message
    assert str(os.getpid()) in message
    assert ALLOW_MULTI_WRITER_ENV in message, "错误提示必须告诉用户如何显式允许多写者"
    assert lock.owned is False

    # 冲突时必须保持原锁文件不动
    assert _read_lock_file(lock_path)["pid"] == os.getpid()


def test_live_lock_with_future_heartbeat_rejected_across_hosts(tmp_path: Path):
    """host 不同（Docker 与原生共用 data/）且心跳新鲜时同样拒绝。"""

    lock_path = tmp_path / WRITER_LOCK_FILENAME
    _write_lock_file(lock_path, pid=os.getpid(), host="another-container", heartbeat_age_seconds=0)

    lock = WriterLock(lock_path, auto_heartbeat=False)

    with pytest.raises(RuntimeError):
        lock.acquire()


# --------------------------------------------------------------------- 场景 3

def test_stale_heartbeat_is_taken_over(tmp_path: Path):
    lock_path = tmp_path / WRITER_LOCK_FILENAME
    # 进程仍存活（用自身 PID），但心跳已过期 120s > 60s 阈值
    _write_lock_file(lock_path, pid=os.getpid(), host=writer_lock_module.socket.gethostname(), heartbeat_age_seconds=120)

    lock = WriterLock(lock_path, auto_heartbeat=False, stale_after=60.0)
    info = lock.acquire()

    assert lock.owned is True
    assert info.pid == os.getpid()
    assert _read_lock_file(lock_path)["pid"] == os.getpid()
    lock.release()


def test_dead_pid_is_taken_over(tmp_path: Path):
    lock_path = tmp_path / WRITER_LOCK_FILENAME
    dead_pid = _find_dead_pid()
    _write_lock_file(lock_path, pid=dead_pid, host=writer_lock_module.socket.gethostname(), heartbeat_age_seconds=0)

    lock = WriterLock(lock_path, auto_heartbeat=False)
    info = lock.acquire()

    assert lock.owned is True
    assert info.pid == os.getpid()
    assert _read_lock_file(lock_path)["pid"] == os.getpid()
    lock.release()


def test_broken_lock_file_is_treated_as_free(tmp_path: Path):
    lock_path = tmp_path / WRITER_LOCK_FILENAME
    lock_path.write_text("null", encoding="utf-8")

    lock = WriterLock(lock_path, auto_heartbeat=False)
    lock.acquire()

    assert lock.owned is True
    assert _read_lock_file(lock_path)["pid"] == os.getpid()
    lock.release()


# --------------------------------------------------------------------- 场景 4

def test_env_bypass_skips_conflict_and_keeps_existing_lock(tmp_path: Path, monkeypatch):
    lock_path = tmp_path / WRITER_LOCK_FILENAME
    original = _write_lock_file(
        lock_path,
        pid=os.getpid(),
        host=writer_lock_module.socket.gethostname(),
        heartbeat_age_seconds=0,
    )

    monkeypatch.setenv(ALLOW_MULTI_WRITER_ENV, "1")

    lock = WriterLock(lock_path, auto_heartbeat=False)
    info = lock.acquire()

    assert info.bypassed is True
    assert lock.bypassed is True
    assert lock.owned is False

    # 旁路不得覆盖真正的持有者
    assert _read_lock_file(lock_path) == original

    lock.release()
    assert lock_path.exists(), "旁路实例 release() 不应删除他人的锁文件"


def test_env_bypass_is_off_by_default(tmp_path: Path):
    lock_path = tmp_path / WRITER_LOCK_FILENAME
    _write_lock_file(lock_path, pid=os.getpid(), host=writer_lock_module.socket.gethostname(), heartbeat_age_seconds=0)

    assert writer_lock_module.allow_multi_writer() is False

    lock = WriterLock(lock_path, auto_heartbeat=False)
    with pytest.raises(RuntimeError):
        lock.acquire()


# --------------------------------------------------------------------- 场景 5

def test_release_allows_next_acquire(tmp_path: Path):
    lock_path = tmp_path / WRITER_LOCK_FILENAME

    first = WriterLock(lock_path, auto_heartbeat=False)
    first.acquire()
    assert lock_path.exists()

    first.release()
    assert not lock_path.exists()
    assert first.owned is False

    second = WriterLock(lock_path, auto_heartbeat=False)
    second.acquire()
    assert second.owned is True
    second.release()


def test_release_does_not_delete_someone_elses_lock(tmp_path: Path):
    lock_path = tmp_path / WRITER_LOCK_FILENAME
    lock = WriterLock(lock_path, auto_heartbeat=False)
    lock.acquire()

    # 模拟锁被另一个进程接管
    _write_lock_file(lock_path, pid=os.getpid() + 1, host="other-host", heartbeat_age_seconds=0)

    lock.release()

    assert lock_path.exists()
    assert _read_lock_file(lock_path)["pid"] == os.getpid() + 1


def test_heartbeat_refreshes_timestamp(tmp_path: Path):
    lock_path = tmp_path / WRITER_LOCK_FILENAME
    lock = WriterLock(lock_path, auto_heartbeat=False)
    lock.acquire()

    before = datetime.fromisoformat(_read_lock_file(lock_path)["heartbeat_at"])
    time.sleep(0.05)
    lock.heartbeat()
    after = datetime.fromisoformat(_read_lock_file(lock_path)["heartbeat_at"])

    assert after > before
    lock.release()


def test_auto_heartbeat_thread_is_released(tmp_path: Path):
    lock_path = tmp_path / WRITER_LOCK_FILENAME
    lock = WriterLock(lock_path, auto_heartbeat=True, heartbeat_interval=0.05)
    lock.acquire()

    thread = lock._heartbeat_thread
    assert thread is not None and thread.is_alive()

    time.sleep(0.12)
    lock.release()

    assert not lock_path.exists()
    assert lock._heartbeat_thread is None
    assert not thread.is_alive(), "release() 必须停掉心跳线程"
