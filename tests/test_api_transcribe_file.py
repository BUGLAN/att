"""本地文件转写接口（POST /api/transcribe）回归测试。

对应《ATT转写接口需求》§5 验收清单：
- 200 同步转写（want_segments 可选）
- 同一文件第二次秒回（内容哈希缓存，provider 只被调一次）
- 404 文件不存在 / 415 非媒体文件 / 413 超时长上限
- 503 引擎忙 + Retry-After
- >30 分钟走 202 任务 + 轮询 done
- X-ATT-Token 可选鉴权
- 在线 ASR 配置被强制回落本地 whisper

测试不依赖 ffmpeg 与本机 data/：探测、抽音轨、provider 全部替换为桩。
"""
import asyncio
import time
from pathlib import Path
from types import SimpleNamespace

import pytest
from fastapi import FastAPI
from fastapi.testclient import TestClient

from app.api.routers import transcribe_file as transcribe_file_router_module
from app.api.routers.transcribe_file import router as transcribe_file_router
from app.config import settings
from app.services.file_transcription_service import (
    FileTranscribeError,
    FileTranscriptionService,
)
from app.stt import factory as stt_factory
from app.stt.models import ASRProviderConfig, ASRProviderType
from app.stt.providers.whisper_provider import WhisperProvider


class _FakeProvider:
    """假本地引擎：记录调用次数与收到的音频路径。"""

    provider_type = "whisper"
    model_name = "fake-medium"

    def __init__(self):
        self.calls = 0
        self.audio_paths = []

    async def transcribe(self, audio_path, language="zh", **kwargs):
        self.calls += 1
        self.audio_paths.append(str(audio_path))
        return SimpleNamespace(
            text="你好世界，这是转写结果。",
            duration=61.0,
            processing_time=0.2,
            segments=[SimpleNamespace(start=0.0, end=3.2, text="你好世界，"),
                      SimpleNamespace(start=3.2, end=5.0, text="这是转写结果。")],
        )


@pytest.fixture
def service(monkeypatch, temp_dir):
    """全新服务实例 + 桩 provider + 指向临时目录的缓存文件。"""
    instance = FileTranscriptionService()
    fake_provider = _FakeProvider()

    monkeypatch.setattr(instance, "_get_provider", lambda: fake_provider)
    monkeypatch.setattr(
        instance, "_cache_path", lambda: Path(temp_dir) / "transcribe_file_cache.json"
    )
    monkeypatch.setattr(
        transcribe_file_router_module, "file_transcription_service", instance
    )
    instance.fake_provider = fake_provider
    return instance


@pytest.fixture
def client(service):
    """仅挂载文件转写路由的测试应用。"""
    app = FastAPI()
    app.include_router(transcribe_file_router)
    with TestClient(app) as test_client:
        yield test_client


def _media_file(temp_dir: Path, suffix: str, content: bytes = b"fake-media-bytes") -> Path:
    target = Path(temp_dir) / f"sample{suffix}"
    target.write_bytes(content)
    return target


def _patch_probe(monkeypatch, service, duration: float):
    async def _fake_probe(target):
        return duration

    monkeypatch.setattr(service, "_probe_duration", _fake_probe)


def _patch_no_extract(monkeypatch, service):
    """音频容器路径：视频抽音轨默认透传，避免测试真的拉起 ffmpeg。"""
    async def _no_extract(target):
        return None

    monkeypatch.setattr(service, "_extract_audio_if_needed", _no_extract)


# ---------------------------------------------------------------- 同步转写

def test_transcribe_sync_ok(client, service, monkeypatch, temp_dir):
    media = _media_file(temp_dir, ".mp3")
    _patch_probe(monkeypatch, service, 754.2)

    response = client.post("/api/transcribe", json={"path": str(media)})

    assert response.status_code == 200
    body = response.json()
    assert body["text"] == "你好世界，这是转写结果。"
    assert body["duration_seconds"] == 754.2
    assert body["engine"] == "whisper:fake-medium"
    assert body["cached"] is False
    assert "segments" not in body  # 默认不返回分段
    assert service.fake_provider.calls == 1


def test_transcribe_want_segments(client, service, monkeypatch, temp_dir):
    media = _media_file(temp_dir, ".wav")
    _patch_probe(monkeypatch, service, 61.0)

    response = client.post(
        "/api/transcribe", json={"path": str(media), "want_segments": True}
    )

    assert response.status_code == 200
    segments = response.json()["segments"]
    assert len(segments) == 2
    assert segments[0] == {"start": 0.0, "end": 3.2, "text": "你好世界，"}


def test_cache_second_call_skips_engine(client, service, monkeypatch, temp_dir):
    media = _media_file(temp_dir, ".mp3")
    _patch_probe(monkeypatch, service, 10.0)

    first = client.post("/api/transcribe", json={"path": str(media)})
    second = client.post("/api/transcribe", json={"path": str(media)})

    assert first.status_code == 200
    assert second.status_code == 200
    assert second.json()["cached"] is True
    assert second.json()["text"] == first.json()["text"]
    # 内容哈希缓存：同内容只允许真的转写一次
    assert service.fake_provider.calls == 1


# ---------------------------------------------------------------- 错误码

def test_missing_file_404(client, monkeypatch, temp_dir):
    response = client.post(
        "/api/transcribe", json={"path": str(Path(temp_dir) / "no_such.mp4")}
    )
    assert response.status_code == 404


def test_relative_path_404(client):
    response = client.post("/api/transcribe", json={"path": "relative/dir/a.mp3"})
    assert response.status_code == 404


def test_txt_file_415(client, temp_dir):
    txt = Path(temp_dir) / "notes.txt"
    txt.write_text("不是媒体文件", encoding="utf-8")
    response = client.post("/api/transcribe", json={"path": str(txt)})
    assert response.status_code == 415


def test_undecodable_media_415(client, service, monkeypatch, temp_dir):
    garbage = _media_file(temp_dir, ".wav")

    async def _fail_probe(target):
        raise FileTranscribeError(415, "无法解码：文件损坏或不是有效的音视频文件")

    monkeypatch.setattr(service, "_probe_duration", _fail_probe)
    response = client.post("/api/transcribe", json={"path": str(garbage)})
    assert response.status_code == 415


def test_duration_limit_413(client, service, monkeypatch, temp_dir):
    media = _media_file(temp_dir, ".mp4")
    _patch_probe(monkeypatch, service, 3 * 3600)  # 3 小时 > 默认上限 2 小时

    response = client.post("/api/transcribe", json={"path": str(media)})

    assert response.status_code == 413
    assert "分钟" in response.json()["detail"]


def test_busy_503_with_retry_after(client, service, monkeypatch, temp_dir):
    media = _media_file(temp_dir, ".mp3")
    _patch_probe(monkeypatch, service, 10.0)

    async def _hold_lock():
        await service._lock.acquire()

    asyncio.run(_hold_lock())  # 模拟另一个转写正在占用单并发闸门
    try:
        response = client.post("/api/transcribe", json={"path": str(media)})
    finally:
        service._lock.release()

    assert response.status_code == 503
    assert response.headers["Retry-After"] == str(
        settings.transcribe_file_busy_retry_after_seconds
    )


# ---------------------------------------------------------------- 异步任务

def test_long_file_202_then_done(client, service, monkeypatch, temp_dir):
    media = _media_file(temp_dir, ".flv")
    _patch_probe(monkeypatch, service, 2400.0)  # 40 分钟 > 默认 30 分钟阈值
    _patch_no_extract(monkeypatch, service)     # 后台任务里也不要真的跑 ffmpeg

    response = client.post("/api/transcribe", json={"path": str(media)})
    assert response.status_code == 202
    job_id = response.json()["job_id"]

    deadline = time.time() + 10
    status_payload = None
    while time.time() < deadline:
        poll = client.get(f"/api/transcribe/jobs/{job_id}")
        assert poll.status_code == 200
        status_payload = poll.json()
        if status_payload["status"] != "running":
            break
        time.sleep(0.05)

    assert status_payload is not None
    assert status_payload["status"] == "done"
    assert status_payload["result"]["text"] == "你好世界，这是转写结果。"
    assert status_payload["error"] == ""


def test_unknown_job_404(client):
    response = client.get("/api/transcribe/jobs/not-exist")
    assert response.status_code == 404


# ---------------------------------------------------------------- 鉴权

def test_token_required(monkeypatch, service, temp_dir):
    monkeypatch.setattr(settings, "transcribe_file_api_token", "secret-token")
    media = _media_file(temp_dir, ".mp3")
    _patch_probe(monkeypatch, service, 10.0)

    app = FastAPI()
    app.include_router(transcribe_file_router)
    with TestClient(app) as client:
        denied = client.post("/api/transcribe", json={"path": str(media)})
        assert denied.status_code == 401

        wrong = client.post(
            "/api/transcribe",
            json={"path": str(media)},
            headers={"X-ATT-Token": "wrong"},
        )
        assert wrong.status_code == 401

        ok = client.post(
            "/api/transcribe",
            json={"path": str(media)},
            headers={"X-ATT-Token": "secret-token"},
        )
        assert ok.status_code == 200


# ---------------------------------------------------------------- 引擎选择

def test_provider_fallback_forces_local_whisper(monkeypatch, temp_dir):
    """全局 ASR 配置指向豆包（在线）时，文件转写必须回落本地 whisper。"""
    doubao_config = ASRProviderConfig(
        provider=ASRProviderType.DOUBAO_ASR,
        language="zh",
        whisper={},
        qwen_asr={},
        doubao_asr={"api_key": "dummy"},
    )
    monkeypatch.setattr(
        stt_factory, "get_asr_provider_config", lambda: doubao_config
    )

    service = FileTranscriptionService()
    provider = service._get_provider()
    assert isinstance(provider, WhisperProvider)


def test_video_container_extracts_audio_first(client, service, monkeypatch, temp_dir):
    """视频容器必须先抽音轨：provider 收到的是抽出来的 wav，而不是原始 mp4。"""
    media = _media_file(temp_dir, ".mp4")
    extracted_wav = _media_file(temp_dir, ".wav")
    _patch_probe(monkeypatch, service, 10.0)

    extract_calls = []

    async def _fake_extract(target):
        extract_calls.append(str(target))
        return extracted_wav

    monkeypatch.setattr(service, "_extract_audio_if_needed", _fake_extract)

    response = client.post("/api/transcribe", json={"path": str(media)})

    assert response.status_code == 200
    assert extract_calls == [str(media)]
    assert service.fake_provider.audio_paths == [str(extracted_wav)]
