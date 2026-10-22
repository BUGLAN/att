"""本地文件转写接口（POST /api/transcribe）回归测试。

对应《ATT转写接口需求》§5 验收清单：
- 200 同步转写（want_segments 可选）
- 同一文件第二次秒回（内容哈希缓存，provider 只被调一次）
- 404 文件不存在 / 415 非媒体文件 / 413 超时长上限
- 503 引擎忙 + Retry-After
- >30 分钟走 202 任务 + 轮询 done
- X-ATT-Token 可选鉴权
- 在线 ASR 配置被强制回落本地 whisper（auto 语义）
- M1：engine 显式/auto 分流、输入适配矩阵、豆包凭据 409、缓存 v2（引擎分桶
  + v1 平铺迁移 + LRU 整条淘汰）、豆包超长分段与计费透出

测试不依赖 ffmpeg 与本机 data/ 与本机 .env：探测、抽音轨、provider、
全局 ASR 配置全部替换为桩。
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

    def __init__(self, text="你好世界，这是转写结果。"):
        self.calls = 0
        self.audio_paths = []
        self.text = text

    async def transcribe(self, audio_path, language="zh", **kwargs):
        self.calls += 1
        self.audio_paths.append(str(audio_path))
        return SimpleNamespace(
            text=self.text,
            duration=61.0,
            processing_time=0.2,
            segments=[SimpleNamespace(start=0.0, end=3.2, text="你好世界，"),
                      SimpleNamespace(start=3.2, end=5.0, text="这是转写结果。")],
        )


@pytest.fixture(autouse=True)
def pin_global_asr_config(monkeypatch):
    """固定 auto 解析的基线配置，与开发者本机 .env / data/settings.json 无关。

    conftest 的 autouse fixture 只隔离 user_settings；这里再把
    get_asr_provider_config 整个钉死成 whisper 基线（AGENTS 硬约束 #5 的延伸）。
    """
    monkeypatch.setattr(
        stt_factory,
        "get_asr_provider_config",
        lambda: _baseline_config(),
    )


def _baseline_config(api_key: str = "") -> ASRProviderConfig:
    return ASRProviderConfig(
        provider=ASRProviderType.WHISPER,
        language="zh",
        whisper={"model_size": "tiny", "device": "cpu", "compute_type": "int8", "download_root": ""},
        qwen_asr={},
        doubao_asr={"api_key": api_key},
    )


@pytest.fixture
def service(monkeypatch, temp_dir):
    """全新服务实例 + 桩 provider + 指向临时目录的缓存文件。"""
    instance = FileTranscriptionService()
    fake_provider = _FakeProvider()

    monkeypatch.setattr(instance, "_get_provider", lambda engine, config: fake_provider)
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
    async def _no_extract(target, engine):
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

def test_auto_engine_falls_back_to_local_whisper(monkeypatch, temp_dir):
    """engine=auto：全局配置指向豆包（在线）时回落本地 whisper，且不需要凭据。"""
    monkeypatch.setattr(
        stt_factory, "get_asr_provider_config", lambda: _baseline_config(api_key="")
    )
    monkeypatch.setattr(
        stt_factory,
        "get_asr_provider_config",
        lambda: ASRProviderConfig(
            provider=ASRProviderType.DOUBAO_ASR,
            language="zh",
            whisper={},
            qwen_asr={},
            doubao_asr={"api_key": ""},
        ),
    )

    service = FileTranscriptionService()
    engine, config = service._resolve_engine("auto")
    assert engine == ASRProviderType.WHISPER

    provider = service._get_provider(engine, config)
    assert isinstance(provider, WhisperProvider)


def test_video_container_extracts_audio_first(client, service, monkeypatch, temp_dir):
    """视频容器必须先抽音轨：provider 收到的是抽出来的 wav，而不是原始 mp4。"""
    media = _media_file(temp_dir, ".mp4")
    extracted_wav = _media_file(temp_dir, ".wav")
    _patch_probe(monkeypatch, service, 10.0)

    extract_calls = []

    async def _fake_extract(target, engine):
        extract_calls.append(str(target))
        return extracted_wav

    monkeypatch.setattr(service, "_extract_audio_if_needed", _fake_extract)

    response = client.post("/api/transcribe", json={"path": str(media)})

    assert response.status_code == 200
    assert extract_calls == [str(media)]
    assert service.fake_provider.audio_paths == [str(extracted_wav)]


# ---------------------------------------------------------------- M1：引擎分流

def test_engine_input_adaptation_matrix(monkeypatch, service, temp_dir):
    """适配矩阵（设计文档 §3.2）：

    - whisper / doubao：仅视频容器抽轨（尝试抽取 → 桩 ffmpeg 失败 → 415）；音频透传（None）；
    - qwen：除 .wav 外一律抽轨（soundfile 只认 wav/flac/ogg）。
    """
    async def _fail_ffmpeg(args):
        return b"", b"stub", 1

    monkeypatch.setattr(service, "_run_subprocess", _fail_ffmpeg)

    matrix = [
        ("whisper", ".mp3", False),
        ("whisper", ".mp4", True),
        ("qwen_asr", ".wav", False),
        ("qwen_asr", ".mp3", True),
        ("qwen_asr", ".mp4", True),
        ("doubao_asr", ".mp3", False),
        ("doubao_asr", ".mp4", True),
    ]
    for engine_name, suffix, should_extract in matrix:
        media = _media_file(temp_dir, suffix)
        engine = ASRProviderType(engine_name)
        if should_extract:
            with pytest.raises(FileTranscribeError) as exc:
                asyncio.run(service._extract_audio_if_needed(media, engine))
            assert exc.value.status_code == 415, f"{engine_name}{suffix} 应抽轨"
        else:
            assert asyncio.run(service._extract_audio_if_needed(media, engine)) is None, (
                f"{engine_name}{suffix} 应透传"
            )


def test_resolve_engine_explicit_auto_and_409(monkeypatch, service):
    """显式引擎 > auto；显式豆包缺凭据 409；auto 不因凭据缺失 409；非法值 422。"""
    monkeypatch.setattr(
        stt_factory, "get_asr_provider_config",
        lambda: ASRProviderConfig(
            provider=ASRProviderType.DOUBAO_ASR, language="zh",
            whisper={}, qwen_asr={}, doubao_asr={"api_key": "k"},
        ),
    )
    assert service._resolve_engine("doubao_asr")[0] == ASRProviderType.DOUBAO_ASR
    assert service._resolve_engine("qwen_asr")[0] == ASRProviderType.QWEN_ASR
    assert service._resolve_engine("auto")[0] == ASRProviderType.WHISPER  # 在线回落

    monkeypatch.setattr(
        stt_factory, "get_asr_provider_config",
        lambda: ASRProviderConfig(
            provider=ASRProviderType.DOUBAO_ASR, language="zh",
            whisper={}, qwen_asr={}, doubao_asr={"api_key": ""},
        ),
    )
    with pytest.raises(FileTranscribeError) as exc:
        service._resolve_engine("doubao_asr")
    assert exc.value.status_code == 409
    # auto 缺凭据不 409：回落 whisper 是既定语义
    assert service._resolve_engine("auto")[0] == ASRProviderType.WHISPER

    with pytest.raises(FileTranscribeError) as exc2:
        service._resolve_engine("gpt")
    assert exc2.value.status_code == 422


def test_engine_invalid_rejected_by_contract(client, temp_dir):
    """engine 非法值由路由层 Literal 拦截为 422。"""
    media = _media_file(temp_dir, ".mp3")
    response = client.post("/api/transcribe", json={"path": str(media), "engine": "gpt"})
    assert response.status_code == 422


def test_doubao_without_credentials_returns_409(client, monkeypatch, temp_dir):
    """显式 doubao_asr 且凭据缺失 → 409，detail 含可读指引。"""
    monkeypatch.setattr(stt_factory, "get_asr_provider_config", lambda: _baseline_config(api_key=""))
    media = _media_file(temp_dir, ".mp3")

    app = FastAPI()
    app.include_router(transcribe_file_router)
    with TestClient(app) as test_client:
        response = test_client.post(
            "/api/transcribe", json={"path": str(media), "engine": "doubao_asr"}
        )
    assert response.status_code == 409
    assert "Access Token" in response.json()["detail"]


# ---------------------------------------------------------------- M1：缓存 v2

def test_cache_v2_multi_engine_buckets(client, service, monkeypatch, temp_dir):
    """同文件多引擎：桶互不覆盖、各引擎独立秒回、聚合在一条目内。"""
    media = _media_file(temp_dir, ".mp3")
    _patch_probe(monkeypatch, service, 10.0)
    _patch_no_extract(monkeypatch, service)

    providers = {"whisper": _FakeProvider("白话转写"), "qwen_asr": _FakeProvider("文言转写")}
    monkeypatch.setattr(service, "_get_provider", lambda engine, config: providers[engine.value])

    first = client.post("/api/transcribe", json={"path": str(media), "engine": "whisper"})
    second = client.post("/api/transcribe", json={"path": str(media), "engine": "qwen_asr"})
    assert first.json()["text"] == "白话转写"
    assert second.json()["text"] == "文言转写"
    assert providers["whisper"].calls == 1 and providers["qwen_asr"].calls == 1

    # 两个引擎各自命中缓存（provider 不再被调）
    again_w = client.post("/api/transcribe", json={"path": str(media), "engine": "whisper"})
    again_q = client.post("/api/transcribe", json={"path": str(media), "engine": "qwen_asr"})
    assert again_w.json()["cached"] is True and again_w.json()["text"] == "白话转写"
    assert again_q.json()["cached"] is True and again_q.json()["text"] == "文言转写"
    assert providers["whisper"].calls == 1 and providers["qwen_asr"].calls == 1

    # 内部：同一内容哈希只有一条文件条目，两个引擎桶
    assert len(service._cache) == 1
    entry = next(iter(service._cache.values()))
    assert set(entry["engines"]) == {"whisper", "qwen_asr"}


def test_cache_v1_flat_entry_reads_as_whisper(client, service, monkeypatch, temp_dir):
    """v1 平铺缓存条目宽容读为 whisper；写新引擎时整条升级为 v2 结构。"""
    from app.repositories.json_store import write_json_atomic

    media = _media_file(temp_dir, ".mp3")
    content_hash = FileTranscriptionService._content_hash_sync(media)
    write_json_atomic(
        Path(temp_dir) / "transcribe_file_cache.json",
        {content_hash: {"text": "旧版结果", "duration_seconds": 5.0,
                        "engine": "whisper:large-v3", "segments": []}},
    )

    hit = client.post("/api/transcribe", json={"path": str(media), "engine": "whisper"})
    assert hit.status_code == 200
    assert hit.json()["cached"] is True
    assert hit.json()["text"] == "旧版结果"

    # qwen 未命中 → 转写 → 条目升级：whisper 桶保留旧值，qwen 桶新增
    _patch_probe(monkeypatch, service, 5.0)
    _patch_no_extract(monkeypatch, service)
    providers = {"whisper": _FakeProvider(), "qwen_asr": _FakeProvider("新引擎结果")}
    monkeypatch.setattr(service, "_get_provider", lambda engine, config: providers[engine.value])
    miss = client.post("/api/transcribe", json={"path": str(media), "engine": "qwen_asr"})
    assert miss.json()["text"] == "新引擎结果"

    entry = service._cache[content_hash]
    assert entry["engines"]["whisper"]["text"] == "旧版结果"
    assert entry["engines"]["qwen_asr"]["text"] == "新引擎结果"


def test_cache_lru_evicts_whole_file_entries(client, service, monkeypatch, temp_dir):
    """LRU 淘汰按整文件条目：多引擎变体不挤占其他文件的名额。"""
    monkeypatch.setattr(settings, "transcribe_file_cache_size", 1)
    file_a = _media_file(temp_dir, ".mp3")
    file_b = _media_file(temp_dir, ".wav", b"another-file")
    _patch_probe(monkeypatch, service, 10.0)
    _patch_no_extract(monkeypatch, service)

    # 文件 A 两个引擎 → 仍只是一条目；文件 B 进来后 A 整条被淘汰
    client.post("/api/transcribe", json={"path": str(file_a), "engine": "whisper"})
    client.post("/api/transcribe", json={"path": str(file_a), "engine": "qwen_asr"})
    assert len(service._cache) == 1

    client.post("/api/transcribe", json={"path": str(file_b), "engine": "whisper"})
    assert len(service._cache) == 1
    hash_b = FileTranscriptionService._content_hash_sync(file_b)
    assert hash_b in service._cache


# ---------------------------------------------------------------- M1：豆包分段

def test_doubao_long_file_segmented_with_billing(client, service, monkeypatch, temp_dir):
    """豆包超会话上限：按 30 分钟切段、逐段转写、时间戳偏移、计费累加。"""
    media = _media_file(temp_dir, ".mp3")
    _patch_probe(monkeypatch, service, 4000.0)  # 66 分钟 → 3 段（1800+1800+400）
    _patch_no_extract(monkeypatch, service)
    monkeypatch.setattr(settings, "transcribe_file_async_threshold_seconds", 7200)  # 保持同步路径
    monkeypatch.setattr(
        stt_factory, "get_asr_provider_config", lambda: _baseline_config(api_key="k")
    )

    slices = []

    async def _fake_slice(target, start, seconds):
        path = Path(temp_dir) / f"slice_{len(slices)}.wav"
        path.write_bytes(b"x")
        slices.append((start, seconds))
        return path

    monkeypatch.setattr(service, "_slice_audio", _fake_slice)

    from app.records.models import TranscriptionResult, TranscriptionSegment

    class _ChunkDoubao:
        provider_type = "doubao_asr"
        model_name = "doubao-fake"

        def __init__(self):
            self.calls = 0

        async def transcribe(self, audio_path, language="zh", **kwargs):
            index = self.calls
            self.calls += 1
            chunk_len = 1800.0 if index < 2 else 400.0
            return TranscriptionResult(
                segments=[TranscriptionSegment(
                    start=0.0, end=chunk_len, text=f"第{index + 1}段文本",
                    confidence=1.0, language="zh",
                )],
                language="zh", language_probability=1.0,
                duration=chunk_len, processing_time=0.1, billing_seconds=100.0,
            )

    doubao = _ChunkDoubao()
    monkeypatch.setattr(service, "_get_provider", lambda engine, config: doubao)

    response = client.post(
        "/api/transcribe",
        json={"path": str(media), "engine": "doubao_asr", "want_segments": True},
    )

    assert response.status_code == 200
    body = response.json()
    assert body["engine"] == "doubao_asr:doubao-fake"
    assert body["text"] == "第1段文本 第2段文本 第3段文本"
    assert body["billing"] == {"audio_duration_seconds": 300.0}
    assert slices == [(0.0, 1800.0), (1800.0, 1800.0), (3600.0, 400.0)]
    assert [seg["start"] for seg in body["segments"]] == [0.0, 1800.0, 3600.0]
    assert [seg["end"] for seg in body["segments"]] == [1800.0, 3600.0, 4000.0]
    assert doubao.calls == 3
