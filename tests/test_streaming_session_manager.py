"""流式会话管理器收尾行为测试。

覆盖 2026-10 豆包流式排查确认的两个收尾缺陷的修复：

1. 空文本（极短点击 / 无有效语音）不再抛错，而是返回 record=null 的正常结果；
2. 自动优化不再内联阻塞 stream_finished 回包（LLM 无超时，内联等待会让
   前端在「正在完成流式转写」卡最长 10 分钟），改为后台任务 + WS 广播。
"""
import asyncio
from dataclasses import dataclass
from pathlib import Path
from types import SimpleNamespace

from app.records.models import RecordStatus
from app.services import optimization_stream_service
from app.services import streaming_session_manager as ssm_module
from app.services.streaming_session_manager import (
    ManagedStreamingSession,
    StreamingSessionManager,
)


@dataclass
class FakeRecord:
    id: str
    status: RecordStatus = RecordStatus.TRANSCRIBED
    raw_text: str = ""
    optimized_text: str = ""

    def to_dict(self):
        return {
            "id": self.id,
            "status": self.status.value,
            "raw_text": self.raw_text,
            "optimized_text": self.optimized_text,
        }


class FakeRecordManager:
    def __init__(self):
        self.counter = 0
        self.records = {}
        self.updates = []
        self.error_messages = []

    async def create_record(self, audio_path, duration, status, raw_text):
        self.counter += 1
        record = FakeRecord(id=f"record-{self.counter}", status=status, raw_text=raw_text)
        self.records[record.id] = record
        return record

    async def update_record(self, record_id, **kwargs):
        record = self.records[record_id]
        for key, value in kwargs.items():
            setattr(record, key, value)
        self.updates.append((record_id, dict(kwargs)))
        return record

    async def set_error_message(self, record_id, message):
        self.error_messages.append((record_id, message))


class FakeOptimizer:
    def __init__(self, gate: asyncio.Event = None, error: Exception = None):
        self._gate = gate
        self._error = error
        self.calls = []

    async def optimize(self, text, preset_id=None):
        self.calls.append(text)
        if self._gate is not None:
            await self._gate.wait()
        if self._error is not None:
            raise self._error
        return SimpleNamespace(optimized_text=f"优化后的{text}", fallback_reason=None)


class FakeWsManager:
    def __init__(self):
        self.messages = []

    async def broadcast(self, message):
        self.messages.append(message)


class FakeWavWriter:
    def __init__(self, path: Path):
        self.path = path

    def writeframes(self, frames):
        pass

    def close(self):
        pass


class FakeAsrSession:
    def __init__(self, text: str = ""):
        self._text = text
        self.finished = False
        self.closed = False

    def finish(self):
        self.finished = True
        return SimpleNamespace(text=self._text, language="zh-CN")

    def close(self):
        self.closed = True

    def cancel(self):
        self.closed = True


def _build_manager(monkeypatch, tmp_path, *, auto_optimize=False, optimizer=None):
    """构造注入了假依赖的会话管理器，并接管 user_settings 与 ws 广播。"""
    ws = FakeWsManager()
    # 增量/完成事件统一由 optimization_stream_service 广播
    monkeypatch.setattr(optimization_stream_service, "ws_manager", ws)
    monkeypatch.setattr(
        ssm_module,
        "user_settings",
        SimpleNamespace(get=lambda key, default=None: True if key == "auto_optimize" else default),
    )

    manager = StreamingSessionManager(FakeRecordManager(), optimizer or FakeOptimizer())
    return manager, ws


def _inject_session(manager: StreamingSessionManager, tmp_path: Path, text: str) -> ManagedStreamingSession:
    wav_path = tmp_path / "session.wav"
    wav_path.write_bytes(b"")
    session = ManagedStreamingSession(
        session_id="session-1",
        asr_session=FakeAsrSession(text=text),
        wav_path=wav_path,
        wav_writer=FakeWavWriter(wav_path),
        created_at=None,
        sample_count=16000,
    )
    manager._sessions["session-1"] = session
    return session


def test_finish_session_empty_text_returns_null_record(monkeypatch, tmp_path):
    """极短点击 / 无有效语音：空文本走正常返回（record=None），不再抛错。"""
    manager, _ = _build_manager(monkeypatch, tmp_path, auto_optimize=False)
    session = _inject_session(manager, tmp_path, text="   ")

    payload = asyncio.run(manager.finish_session("session-1"))

    assert payload["record"] is None
    assert payload["raw_text"] == ""
    assert not session.wav_path.exists(), "空文本会话的音频不应留档"
    assert session.asr_session.closed
    assert manager.record_manager.records == {}, "空文本不应创建记录"


def test_finish_session_returns_before_optimize_completes(monkeypatch, tmp_path):
    """自动优化在后台执行：finish_session 必须在优化完成前就返回 transcribed 记录。"""
    gate = asyncio.Event()
    optimizer = FakeOptimizer(gate=gate)
    manager, ws = _build_manager(monkeypatch, tmp_path, auto_optimize=True, optimizer=optimizer)
    session = _inject_session(manager, tmp_path, text="会议内容")

    async def scenario():
        payload = await manager.finish_session("session-1")
        # 关键断言：优化还卡在 gate 上，回包就已经带着 transcribed 记录返回了
        assert payload["record"]["status"] == RecordStatus.TRANSCRIBED.value
        assert payload["record"]["optimized_text"] == ""
        await asyncio.sleep(0)  # 让后台优化任务拿到执行权
        assert optimizer.calls == ["会议内容"], "优化应已被后台任务发起"

        gate.set()
        pending = [task for task in asyncio.all_tasks() if task is not asyncio.current_task()]
        await asyncio.gather(*pending)
        return payload, ws

    payload, ws = asyncio.run(scenario())

    record = manager.record_manager.records[payload["record"]["id"]]
    assert record.status == RecordStatus.OPTIMIZED
    assert record.optimized_text == "优化后的会议内容"
    assert [message["type"] for message in ws.messages] == [
        "optimization_started",
        "optimization_delta",
        "optimization_completed",
    ]


def test_finish_session_optimize_failure_resets_and_broadcasts(monkeypatch, tmp_path):
    """后台优化失败：记录复位 transcribed 并广播 optimization_failed。"""
    manager, ws = _build_manager(
        monkeypatch,
        tmp_path,
        auto_optimize=True,
        optimizer=FakeOptimizer(error=RuntimeError("LLM 超时")),
    )
    session = _inject_session(manager, tmp_path, text="会议内容")

    async def scenario():
        payload = await manager.finish_session("session-1")
        pending = [task for task in asyncio.all_tasks() if task is not asyncio.current_task()]
        await asyncio.gather(*pending)
        return payload

    payload = asyncio.run(scenario())

    record = manager.record_manager.records[payload["record"]["id"]]
    assert record.status == RecordStatus.TRANSCRIBED, "优化失败必须复位，不能把记录永久留在 optimizing"
    failed = [message for message in ws.messages if message["type"] == "optimization_failed"]
    assert len(failed) == 1
    assert failed[0]["data"]["record_id"] == record.id
