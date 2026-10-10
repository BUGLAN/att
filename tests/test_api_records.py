"""records API 回归测试。

覆盖审计报告 §4.2 第 5/7 条与 §3.2 第 6 条：
- GET /api/records/{id}/audio：空 audio_path / 目录 / 文件不存在都必须是 404，而不是 500；
- POST /api/records/{id}/optimize：LLM 失败后状态必须复位，不得永久卡在 optimizing；
- POST /api/audio/upload：必须先按 settings.max_upload_size 拒绝超大请求。
测试全部使用隔离的临时数据目录，不触碰真实 data/。
"""
import asyncio
from pathlib import Path
from types import SimpleNamespace

from fastapi import FastAPI
from fastapi.testclient import TestClient

from app.api import routes
from app.records.manager import RecordManager
from app.records.models import RecordStatus


class _StubSettings:
    """只暴露本次用到的配置项，避免依赖真实 data/ 与 .env。"""

    def __init__(self, audio_records_path: Path, max_upload_size: int = 500 * 1024 * 1024):
        self.audio_records_path = audio_records_path
        self.max_upload_size = max_upload_size


class _FailingOptimizer:
    """模拟 LLM 调用失败的优化器。"""

    async def optimize(self, text, style="default", **kwargs):
        raise RuntimeError("模拟 LLM 调用失败")


class _SuccessOptimizer:
    """模拟优化成功的优化器。"""

    async def optimize(self, text, style="default", **kwargs):
        # fallback_reason 必须与真实 OptimizationResult 对齐：路由靠它判断是否降级
        return SimpleNamespace(
            optimized_text="优化后的文本",
            processing_time=0.01,
            fallback_reason=None
        )


class _RejectingAudioProcessor:
    """音频校验一律失败的处理器：用于证明请求已越过大小校验、进入音频校验阶段。"""

    async def validate_audio(self, file_content: bytes, filename: str):
        raise ValueError("模拟音频格式不支持")


def _build_client(monkeypatch, temp_dir, *, optimizer=None, audio_processor=None,
                  max_upload_size: int = 500 * 1024 * 1024):
    """构造仅挂载 api 路由的测试应用，并把存储与配置指向隔离目录。"""
    record_manager = RecordManager(temp_dir)
    asyncio.run(record_manager.initialize())

    monkeypatch.setattr(routes, "settings", _StubSettings(temp_dir / "audio" / "records", max_upload_size))
    monkeypatch.setattr(routes, "record_manager", record_manager)
    monkeypatch.setattr(routes, "optimizer", optimizer)
    monkeypatch.setattr(routes, "audio_processor", audio_processor)
    # 关闭流式会话管理器缓存，避免跨测试串用
    monkeypatch.setattr(routes, "streaming_session_manager", None)

    app = FastAPI()
    app.include_router(routes.router)
    return TestClient(app), record_manager


def _create_record(record_manager: RecordManager, **kwargs):
    return asyncio.run(record_manager.create_record(**kwargs))


# ==================== GET /records/{id}/audio ====================

def test_audio_empty_path_returns_404(monkeypatch, temp_dir):
    """空 audio_path（真实数据 36 条）必须是 404，而不是 FileResponse('.') 的 500。"""
    client, manager = _build_client(monkeypatch, temp_dir)
    record = _create_record(manager, audio_path="", duration=1.0)

    response = client.get(f"/api/records/{record.id}/audio")

    assert response.status_code == 404
    assert response.json()["detail"] == "Audio file not found"


def test_audio_whitespace_path_returns_404(monkeypatch, temp_dir):
    """只有空白的 audio_path 同样按缺失处理。"""
    client, manager = _build_client(monkeypatch, temp_dir)
    record = _create_record(manager, audio_path="   ", duration=1.0)

    response = client.get(f"/api/records/{record.id}/audio")

    assert response.status_code == 404


def test_audio_directory_path_returns_404(monkeypatch, temp_dir):
    """audio_path 指向目录时 Path.exists() 为真，但必须是 404（FileResponse 无法处理目录）。"""
    client, manager = _build_client(monkeypatch, temp_dir)
    record = _create_record(manager, audio_path=str(temp_dir), duration=1.0)

    response = client.get(f"/api/records/{record.id}/audio")

    assert response.status_code == 404


def test_audio_missing_file_returns_404(monkeypatch, temp_dir):
    """音频文件已被删除时返回 404。"""
    client, manager = _build_client(monkeypatch, temp_dir)
    record = _create_record(manager, audio_path=str(temp_dir / "not-exist.wav"), duration=1.0)

    response = client.get(f"/api/records/{record.id}/audio")

    assert response.status_code == 404


def test_audio_existing_file_returns_200(monkeypatch, temp_dir):
    """正常音频路径仍返回 200 与文件内容（确认修复没有误伤正常播放）。"""
    client, manager = _build_client(monkeypatch, temp_dir)
    audio_file = temp_dir / "sample.wav"
    payload = b"RIFF" + b"\x00" * 64
    audio_file.write_bytes(payload)
    record = _create_record(manager, audio_path=str(audio_file), duration=1.0)

    response = client.get(f"/api/records/{record.id}/audio")

    assert response.status_code == 200
    assert response.content == payload
    assert response.headers["content-type"] == "audio/wav"


def test_audio_unknown_record_returns_404(monkeypatch, temp_dir):
    """记录不存在仍是 404（契约不变）。"""
    client, _ = _build_client(monkeypatch, temp_dir)

    response = client.get("/api/records/no-such-record/audio")

    assert response.status_code == 404
    assert response.json()["detail"] == "Record not found"


# ==================== POST /records/{id}/optimize ====================

def test_optimize_failure_resets_status(monkeypatch, temp_dir):
    """LLM 失败时必须复位状态：不能永久停留在 optimizing（真实数据已有 2 条）。"""
    client, manager = _build_client(monkeypatch, temp_dir, optimizer=_FailingOptimizer())
    record = _create_record(
        manager,
        audio_path="",
        duration=1.0,
        status=RecordStatus.TRANSCRIBED,
        raw_text="需要优化的原始文本",
    )

    response = client.post(f"/api/records/{record.id}/optimize")

    assert response.status_code == 502
    assert "文本优化失败" in response.json()["detail"]

    stored = asyncio.run(manager.get_record(record.id))
    assert stored.status == RecordStatus.TRANSCRIBED
    assert stored.status != RecordStatus.OPTIMIZING


def test_optimize_success_sets_optimized(monkeypatch, temp_dir):
    """成功路径保持既有行为：状态置 optimized 并返回优化文本。"""
    client, manager = _build_client(monkeypatch, temp_dir, optimizer=_SuccessOptimizer())
    record = _create_record(
        manager,
        audio_path="",
        duration=1.0,
        status=RecordStatus.TRANSCRIBED,
        raw_text="需要优化的原始文本",
    )

    response = client.post(f"/api/records/{record.id}/optimize")

    assert response.status_code == 200
    assert response.json()["optimized_text"] == "优化后的文本"
    assert response.json()["original_text"] == "需要优化的原始文本"

    stored = asyncio.run(manager.get_record(record.id))
    assert stored.status == RecordStatus.OPTIMIZED
    assert stored.optimized_text == "优化后的文本"


def test_optimize_without_raw_text_returns_400(monkeypatch, temp_dir):
    """没有可优化文本时仍是 400（契约不变）。"""
    client, manager = _build_client(monkeypatch, temp_dir, optimizer=_SuccessOptimizer())
    record = _create_record(manager, audio_path="", duration=1.0, raw_text="")

    response = client.post(f"/api/records/{record.id}/optimize")

    assert response.status_code == 400


# ==================== POST /audio/upload 大小上限 ====================

def test_upload_rejects_file_over_max_upload_size(monkeypatch, temp_dir):
    """超过 settings.max_upload_size 的上传必须被拒绝（该配置此前全仓零读取点）。"""
    client, _ = _build_client(
        monkeypatch,
        temp_dir,
        audio_processor=_RejectingAudioProcessor(),
        max_upload_size=16,
    )

    response = client.post(
        "/api/audio/upload",
        files={"file": ("large.wav", b"A" * 4096, "audio/wav")},
    )

    assert response.status_code == 413
    assert "过大" in response.json()["detail"]


def test_upload_within_limit_reaches_audio_validation(monkeypatch, temp_dir):
    """未超限时必须继续走原有音频校验（此处 400 来自音频校验，证明不是被大小校验误杀）。"""
    client, _ = _build_client(
        monkeypatch,
        temp_dir,
        audio_processor=_RejectingAudioProcessor(),
        max_upload_size=500 * 1024 * 1024,
    )

    response = client.post(
        "/api/audio/upload",
        files={"file": ("small.wav", b"A" * 4096, "audio/wav")},
    )

    assert response.status_code == 400
    assert "模拟音频格式不支持" in response.json()["detail"]
