"""流式优化 HTTP 端点回归测试（NDJSON）。"""
import asyncio
import json

from fastapi import FastAPI
from fastapi.testclient import TestClient

from app.api import routes
from app.llm.optimizer import OptimizationResult, OptimizationStreamEvent
from app.records.manager import RecordManager
from app.records.models import RecordStatus


class _StreamingOptimizer:
    def __init__(self, error: Exception | None = None):
        self.error = error
        self.calls = []

    async def optimize_stream(self, text, preset_id=None, **kwargs):
        self.calls.append(("stream", text))
        if self.error:
            raise self.error
        yield OptimizationStreamEvent(type="delta", text="优化")
        yield OptimizationStreamEvent(type="delta", text="文本")
        yield OptimizationStreamEvent(
            type="done",
            result=OptimizationResult(
                original_text=text,
                optimized_text="优化文本",
                processing_time=0.01,
                tokens_used=42,
                model_string="deepseek/deepseek-flash",
            ),
        )

    async def optimize(self, text, preset_id=None, **kwargs):
        self.calls.append(("fallback", text))
        if self.error:
            raise self.error
        return OptimizationResult(
            original_text=text,
            optimized_text="降级文本",
            processing_time=0.02,
            tokens_used=10,
            model_string="deepseek/deepseek-flash",
        )


class _FallbackOptimizer:
    """流式接口不可用、非流式可用的上游：验证服务端降级路径。"""

    def __init__(self):
        self.calls = []

    async def optimize_stream(self, text, preset_id=None, **kwargs):
        self.calls.append("stream")
        raise RuntimeError("stream unsupported")
        yield  # pragma: no cover

    async def optimize(self, text, preset_id=None, **kwargs):
        self.calls.append("fallback")
        return OptimizationResult(
            original_text=text,
            optimized_text="降级文本",
            processing_time=0.02,
            tokens_used=10,
            model_string="deepseek/deepseek-flash",
        )


def _build_client(monkeypatch, temp_dir, optimizer):
    record_manager = RecordManager(temp_dir)
    asyncio.run(record_manager.initialize())

    monkeypatch.setattr(routes, "record_manager", record_manager)
    monkeypatch.setattr(routes, "optimizer", optimizer)
    monkeypatch.setattr(routes, "streaming_session_manager", None)

    app = FastAPI()
    app.include_router(routes.router)
    return TestClient(app), record_manager


def _create_record(record_manager, text="原始文本"):
    return asyncio.run(record_manager.create_record(
        audio_path="",
        duration=1.0,
        status=RecordStatus.TRANSCRIBED,
        raw_text=text,
    ))


def _read_events(response):
    events = []
    for line in response.iter_lines():
        if line:
            events.append(json.loads(line))
    return events


def test_optimize_stream_returns_ndjson_and_persists(monkeypatch, temp_dir):
    optimizer = _StreamingOptimizer()
    client, manager = _build_client(monkeypatch, temp_dir, optimizer)
    record = _create_record(manager)

    with client.stream("POST", f"/api/records/{record.id}/optimize/stream") as response:
        assert response.status_code == 200
        assert response.headers["content-type"].startswith("application/x-ndjson")
        events = _read_events(response)

    assert [event["type"] for event in events] == ["start", "delta", "delta", "done"]
    assert events[1]["text"] == "优化"
    assert events[3]["optimized_text"] == "优化文本"
    assert events[3]["tokens_used"] == 42

    saved = asyncio.run(manager.get_record(record.id))
    assert saved.status == RecordStatus.OPTIMIZED
    assert saved.optimized_text == "优化文本"


def test_optimize_stream_error_resets_record(monkeypatch, temp_dir):
    optimizer = _StreamingOptimizer(error=RuntimeError("模拟流式失败"))
    client, manager = _build_client(monkeypatch, temp_dir, optimizer)
    record = _create_record(manager)

    with client.stream("POST", f"/api/records/{record.id}/optimize/stream") as response:
        events = _read_events(response)

    assert events[0]["type"] == "start"
    assert events[-1]["type"] == "error"
    assert "模拟流式失败" in events[-1]["error"]
    saved = asyncio.run(manager.get_record(record.id))
    assert saved.status == RecordStatus.TRANSCRIBED
    assert "模拟流式失败" in (saved.error or "")


def test_optimize_stream_falls_back_when_stream_unsupported(monkeypatch, temp_dir):
    optimizer = _FallbackOptimizer()
    client, manager = _build_client(monkeypatch, temp_dir, optimizer)
    record = _create_record(manager)

    with client.stream("POST", f"/api/records/{record.id}/optimize/stream") as response:
        events = _read_events(response)

    assert [event["type"] for event in events] == ["start", "delta", "done"]
    assert events[1]["text"] == "降级文本"
    assert events[-1]["degraded"] is True
    assert optimizer.calls == ["stream", "fallback"]

    saved = asyncio.run(manager.get_record(record.id))
    assert saved.status == RecordStatus.OPTIMIZED
    assert saved.optimized_text == "降级文本"


def test_optimize_stream_missing_record_returns_404(monkeypatch, temp_dir):
    client, _ = _build_client(monkeypatch, temp_dir, _StreamingOptimizer())
    response = client.post("/api/records/no-such-record/optimize/stream")
    assert response.status_code == 404


def test_cancel_optimize_without_active_task_returns_false(monkeypatch, temp_dir):
    client, manager = _build_client(monkeypatch, temp_dir, _StreamingOptimizer())
    record = _create_record(manager)
    response = client.post(f"/api/records/{record.id}/optimize/cancel")
    assert response.status_code == 200
    assert response.json()["cancelled"] is False
