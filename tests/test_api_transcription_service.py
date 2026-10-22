"""转写后台任务回归测试（原 `_transcribe_audio_background` 137 行下沉后的行为护栏）。

用假 provider / 假记录管理器 / 假 WebSocket 管理器验证状态机与事件序列：
1. 正常转写：transcribing → transcribed，并广播 started/completed；
2. 自动优化开启：继续 optimizing → optimized，并广播对应事件；
3. 自动优化失败：状态必须复位为 transcribed（不得永久卡在 optimizing）；
4. 转写失败：状态置 failed 并广播 error；
5. 分段转写：成功写回转写文本与耗时，失败置 failed 并广播 segment_failed。
"""
import asyncio
from types import SimpleNamespace

import pytest

from app.records.models import RecordStatus
from app.services import transcription_service


class _FakeASRProvider:
    """假 ASR provider：可配置成功/失败。"""

    def __init__(self, text: str = "识别文本", fail: bool = False):
        self.text = text
        self.fail = fail
        self.calls = []

    async def transcribe(self, audio_path, vad_filter=True):
        self.calls.append((audio_path, vad_filter))
        if self.fail:
            raise RuntimeError("模拟转写失败")
        return SimpleNamespace(
            text=self.text,
            duration=1.5,
            processing_time=0.25,
        )


class _FakeOptimizer:
    """假文本优化器：可配置成功/失败（v2 无降级通道）。"""

    def __init__(self, fail: bool = False):
        self.fail = fail

    async def optimize(self, text, preset_id=None, **kwargs):
        if self.fail:
            raise RuntimeError("模拟优化失败")
        return SimpleNamespace(
            optimized_text=f"优化:{text}",
            processing_time=0.05,
            tokens_used=10,
            model_string="deepseek/deepseek-flash"
        )


class _FakeRecordManager:
    """假记录管理器：记录每次 update_record 的关键字段。"""

    def __init__(self):
        self.updates = []
        # 错误原因写入记录（真实实现为 async def set_error_message）
        self.error_messages = []

    async def update_record(self, record_id, **fields):
        self.updates.append((record_id, fields))
        return SimpleNamespace(id=record_id, **fields)

    async def set_error_message(self, record_id, message):
        self.error_messages.append((record_id, message))

    @property
    def statuses(self):
        return [fields.get("status") for _rid, fields in self.updates if "status" in fields]


class _FakeSessionManager:
    """假会话管理器：单片段，可配置更新失败。"""

    def __init__(self, fail_update: bool = False):
        self.updates = []
        self.fail_update = fail_update
        self.segment = SimpleNamespace(id="seg-1", sequence=1)

    async def get_session(self, session_id):
        return SimpleNamespace(id=session_id, segments=[self.segment])

    async def update_segment(self, session_id, segment_id, **fields):
        if self.fail_update and fields.get("status") == RecordStatus.FAILED:
            raise RuntimeError("模拟状态写入失败")
        self.updates.append((session_id, segment_id, fields))

    @property
    def statuses(self):
        return [fields.get("status") for _s, _seg, fields in self.updates if "status" in fields]


class _FakeWebSocketManager:
    """假 WebSocket 管理器：记录广播事件类型。"""

    def __init__(self):
        self.events = []

    async def broadcast(self, message, exclude=None):
        self.events.append(message["type"])

    def get_active_web_client(self):
        return "client-1"


class _Settings(dict):
    """最小可用的 user_settings 替身。"""

    def get(self, key, default=None):
        return super().get(key, default)


@pytest.fixture
def fake_ws(monkeypatch):
    fake = _FakeWebSocketManager()
    monkeypatch.setattr(transcription_service, "ws_manager", fake)
    return fake


# ==================== 单条录音 ====================

def test_transcribe_success_updates_status_and_broadcasts(fake_ws):
    """正常转写：transcribing → transcribed，事件序列正确。"""
    manager = _FakeRecordManager()

    asyncio.run(transcription_service.transcribe_audio_background(
        "rec-1",
        "a.wav",
        _FakeASRProvider("你好"),
        manager,
        _FakeOptimizer(),
        _Settings(auto_optimize=False),
    ))

    assert manager.statuses == [RecordStatus.TRANSCRIBING, RecordStatus.TRANSCRIBED]
    assert manager.updates[1][1]["raw_text"] == "你好"
    assert manager.updates[1][1]["processing_time"] == 0.25
    assert fake_ws.events == ["transcription_started", "transcription_completed"]


def test_auto_optimize_success_reaches_optimized(fake_ws):
    """auto_optimize 开启：继续 optimizing → optimized 并写入优化文本。"""
    manager = _FakeRecordManager()

    asyncio.run(transcription_service.transcribe_audio_background(
        "rec-2",
        "a.wav",
        _FakeASRProvider("原始"),
        manager,
        _FakeOptimizer(),
        _Settings(auto_optimize=True, default_preset_id="preset-1"),
    ))

    assert manager.statuses == [
        RecordStatus.TRANSCRIBING,
        RecordStatus.TRANSCRIBED,
        RecordStatus.OPTIMIZING,
        RecordStatus.OPTIMIZED,
    ]
    assert manager.updates[-1][1]["optimized_text"] == "优化:原始"
    assert fake_ws.events == [
        "transcription_started",
        "transcription_completed",
        "optimization_started",
        "optimization_completed",
    ]


def test_auto_optimize_success_no_degraded_event(fake_ws):
    """v2 移除了 .env 内置 key 兜底（docs/LLM配置模块重设计.md §9.2）：
    优化成功就是成功，事件序列里不得再出现 optimization_degraded。"""
    manager = _FakeRecordManager()

    asyncio.run(transcription_service.transcribe_audio_background(
        "rec-ok",
        "a.wav",
        _FakeASRProvider("原始"),
        manager,
        _FakeOptimizer(),
        _Settings(auto_optimize=True, default_preset_id="preset-1"),
    ))

    assert manager.statuses[-1] == RecordStatus.OPTIMIZED
    assert fake_ws.events == [
        "transcription_started",
        "transcription_completed",
        "optimization_started",
        "optimization_completed",
    ]
    assert "optimization_degraded" not in fake_ws.events


def test_auto_optimize_failure_resets_to_transcribed(fake_ws):
    """自动优化失败必须复位为 transcribed（真实数据曾出现永久 optimizing）。"""
    manager = _FakeRecordManager()

    asyncio.run(transcription_service.transcribe_audio_background(
        "rec-3",
        "a.wav",
        _FakeASRProvider("原始"),
        manager,
        _FakeOptimizer(fail=True),
        _Settings(auto_optimize=True),
    ))

    assert manager.statuses[-1] == RecordStatus.TRANSCRIBED
    assert RecordStatus.OPTIMIZING in manager.statuses
    # 失败原因必须持久化到记录（Record.error），否则用户看不到历史错误
    assert manager.error_messages == [("rec-3", "模拟优化失败")]
    # 优化失败不广播 optimization_completed，也不广播 error（与原实现一致）
    assert "optimization_completed" not in fake_ws.events
    assert fake_ws.events == [
        "transcription_started",
        "transcription_completed",
        "optimization_started",
    ]


def test_transcribe_failure_marks_failed_and_broadcasts_error(fake_ws):
    """转写失败：状态置 failed 并广播 error。"""
    manager = _FakeRecordManager()

    asyncio.run(transcription_service.transcribe_audio_background(
        "rec-4",
        "a.wav",
        _FakeASRProvider(fail=True),
        manager,
        _FakeOptimizer(),
        _Settings(),
    ))

    assert manager.statuses == [RecordStatus.TRANSCRIBING, RecordStatus.FAILED]
    assert fake_ws.events == ["transcription_started", "error"]


# ==================== 多段录音片段 ====================

def test_segment_transcribe_success(fake_ws):
    """分段转写成功：写回转写文本/耗时，并广播 segment_transcribed（带 sequence）。"""
    manager = _FakeSessionManager()
    provider = _FakeASRProvider("分段文本")

    asyncio.run(transcription_service.transcribe_segment_background(
        "sess-1", "seg-1", "seg.wav", provider, manager
    ))

    # 分段转写必须带 vad_filter=False
    assert provider.calls == [("seg.wav", False)]
    assert manager.statuses == [RecordStatus.TRANSCRIBING, RecordStatus.TRANSCRIBED]
    assert manager.updates[-1][2]["raw_text"] == "分段文本"
    assert fake_ws.events == ["segment_transcribed"]


def test_segment_transcribe_failure_marks_failed(fake_ws):
    """分段转写失败：置 failed 并广播 segment_failed。"""
    manager = _FakeSessionManager()

    asyncio.run(transcription_service.transcribe_segment_background(
        "sess-2", "seg-1", "seg.wav", _FakeASRProvider(fail=True), manager
    ))

    assert manager.statuses == [RecordStatus.TRANSCRIBING, RecordStatus.FAILED]
    assert fake_ws.events == ["segment_failed"]


def test_completion_metadata_includes_electron_active(fake_ws):
    """完成事件必须带 electron_active 元数据（前端据此决定是否刷新）。"""
    metadata = transcription_service.build_completion_metadata(record_id="rec-5")

    assert metadata == {"record_id": "rec-5", "electron_active": True}
