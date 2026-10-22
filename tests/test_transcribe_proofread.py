"""LLM 滑动窗口校对管道测试（设计文档 docs/文件转写-多引擎与LLM校对-设计.md §4）。

覆盖：
- 纯函数：窗口双上限切分 / LLM 输出宽容解析 / 键匹配与相似度验证 / 参数钳制
- 管道：成功、验证反馈重试、失败批保留原文、网关整体不可用降级、空文本跳过、
  多窗口混合结果、进度回调
- 服务集成：proofread 强制 202、结果字段（raw_text / proofread 元数据）、
  校对变体缓存秒回、ASR 桶命中仅补跑校对、无 LLM 时 409

LLM 网关与模型解析全部打桩，不依赖本机 data/llm_v2.json。
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
from app.services import file_transcription_service as service_module
from app.services import transcribe_proofread as proofread_module
from app.services.file_transcription_service import FileTranscriptionService
from app.services.transcribe_proofread import (
    ProofreadOptions,
    ProofreadResult,
    TranscribeProofreader,
    parse_llm_json,
    split_windows,
    validate_window,
)
from app.stt import factory as stt_factory
from app.stt.models import ASRProviderConfig, ASRProviderType


# ---------------------------------------------------------------- 纯函数

def test_split_windows_dual_limit():
    """双上限：段数与字符数同时生效，只切在段边界。"""
    texts = ["x" * 10] * 85  # 85 段 × 10 字
    windows = split_windows(texts, ProofreadOptions(batch_segments=40, batch_chars=4000))
    assert [len(w) for w in windows] == [40, 40, 5]

    # 字符上限先生效：40 段 × 10 字 = 400 字 > 350 → 每 35 段一窗
    windows = split_windows(texts, ProofreadOptions(batch_segments=40, batch_chars=350))
    assert [len(w) for w in windows] == [35, 35, 15]

    assert split_windows([], ProofreadOptions()) == []


def test_split_windows_single_oversized_segment():
    """单段超过字符上限时独立成窗（豆包整场单 segment 场景）。"""
    windows = split_windows(["长" * 9000, "短文本"], ProofreadOptions(batch_segments=40, batch_chars=4000))
    assert windows == [[0], [1]]


def test_parse_llm_json_tolerant():
    assert parse_llm_json('{"0": "甲"}') == {"0": "甲"}
    assert parse_llm_json('```json\n{"0": "甲",}\n```') == {"0": "甲"}
    assert parse_llm_json('好的，结果如下：\n{"0": "甲"} \n以上。') == {"0": "甲"}
    assert parse_llm_json("[]") is None          # 非对象
    assert parse_llm_json("") is None
    assert parse_llm_json("完全不是 JSON") is None


def test_validate_window_keys_and_similarity():
    original = {"0": "今天天气真好我们出去散步", "1": "第二句内容"}
    # 键集合不等 → 拒绝并给出缺失反馈
    ok, feedback = validate_window(original, {"0": "今天天气真好我们出去散步"})
    assert not ok and "缺少键" in feedback
    # 改写过头（相似度崩塌）→ 拒绝
    ok, feedback = validate_window(
        original, {"0": "今天天气真好我们出去散步", "1": "完全不同的另外一句话出现了"}
    )
    assert not ok and "改动过大" in feedback
    # 正常纠错 → 通过
    ok, _ = validate_window(
        original, {"0": "今天天气真好，我们出去散步。", "1": "第二句内容"}
    )
    assert ok


def test_proofread_options_clamp():
    base = ProofreadOptions.from_request(None)
    assert (base.batch_segments, base.batch_chars, base.context_chars) == (40, 4000, 200)

    clamped = ProofreadOptions.from_request(
        {"batch_segments": 99999, "batch_chars": 1, "context_chars": "abc"}
    )
    assert clamped.batch_segments == 200
    assert clamped.batch_chars == 200
    assert clamped.context_chars == 200


# ---------------------------------------------------------------- 管道（假网关）

def _fake_model():
    entry = SimpleNamespace(
        model="fake/proof-model", label="Fake · proof",
        params=SimpleNamespace(temperature=0.2, max_tokens=1000, timeout_s=10, retries=0),
    )
    profile = SimpleNamespace(api_base=None, extra_headers=None)
    return entry, profile, "plain-key"


class _FakeGateway:
    """按脚本顺序回放的假网关；脚本项为 Exception 时抛出。"""

    def __init__(self, replies):
        self.replies = list(replies)
        self.calls = 0
        self.messages_seen = []

    async def chat(self, *, messages, **kwargs):
        self.messages_seen.append(messages)
        self.calls += 1
        reply = self.replies.pop(0)
        if isinstance(reply, Exception):
            raise reply
        return SimpleNamespace(text=reply, total_tokens=1, truncated=False)


def _segments(*texts):
    return [{"start": float(i), "end": float(i) + 1, "text": t} for i, t in enumerate(texts)]


@pytest.fixture
def patched_model(monkeypatch):
    """管道与服务两处 resolve 都指向假模型。"""
    def _install(monkeypatch):
        monkeypatch.setattr(proofread_module, "resolve_proofread_model", lambda model_id: _fake_model())
        monkeypatch.setattr(service_module, "resolve_proofread_model", lambda model_id: _fake_model())
    _install(monkeypatch)
    return _install


def _install_gateway(monkeypatch, replies):
    gateway = _FakeGateway(replies)
    import app.llm.gateway as gateway_module

    monkeypatch.setattr(gateway_module, "get_llm_gateway", lambda: gateway)
    return gateway


def test_pipeline_success(monkeypatch, patched_model):
    gateway = _install_gateway(monkeypatch, ['{"0": "纠错后第一句。", "1": "纠错后第二句。"}'])
    result = asyncio.run(
        TranscribeProofreader(None).run(_segments("纠错后第一句呃", "纠错后第二句嗯"))
    )
    assert result.applied is True
    assert result.text == "纠错后第一句。 纠错后第二句。"
    assert [seg["text"] for seg in result.segments] == ["纠错后第一句。", "纠错后第二句。"]
    assert result.requests == 1 and result.windows == 1
    assert result.fallback_batches == []
    assert result.meta()["llm_model"] == "Fake · proof"
    assert gateway.calls == 1


def test_pipeline_retry_with_feedback(monkeypatch, patched_model):
    """第一次缺键 → 反馈进对话 → 第二次通过（requests=2）。"""
    _install_gateway(
        monkeypatch,
        ['{"0": "纠错后第一句。"}', '{"0": "纠错后第一句。", "1": "纠错后第二句。"}'],
    )
    result = asyncio.run(
        TranscribeProofreader(None).run(_segments("第一句", "第二句"))
    )
    assert result.applied is True
    assert result.requests == 2


def test_pipeline_fallback_batch_keeps_original(monkeypatch, patched_model):
    """验证耗尽（3 次都不过）→ 该窗保留原文，任务照常 applied。"""
    _install_gateway(monkeypatch, ["完全跑偏的改写结果"] * 3)
    result = asyncio.run(
        TranscribeProofreader(None).run(_segments("原始句子内容"))
    )
    assert result.applied is True
    assert result.text == "原始句子内容"
    assert result.fallback_batches == [0]
    assert result.requests == 3


def test_pipeline_gateway_failure_degrades(monkeypatch, patched_model):
    """网关整体不可用 → applied=False + error，原文照常交付。"""
    _install_gateway(monkeypatch, [RuntimeError("连接超时")])
    result = asyncio.run(
        TranscribeProofreader(None).run(_segments("原始句子内容"))
    )
    assert result.applied is False
    assert result.text == "原始句子内容"
    assert "连接超时" in result.error
    assert result.meta()["applied"] is False


def test_pipeline_empty_asr_skips(monkeypatch, patched_model):
    result = asyncio.run(
        TranscribeProofreader(None).run(_segments("", "  "))
    )
    assert result.applied is False
    assert result.reason == "asr_empty"
    assert result.meta().get("reason") == "asr_empty"


def test_pipeline_multi_window_mixed(monkeypatch, patched_model):
    """batch_segments=1 切两窗：一窗成功一窗失败，正文混合。"""
    _install_gateway(
        monkeypatch,
        ['{"0": "第一窗纠错。"}'] + ["跑偏"] * 3,
    )
    result = asyncio.run(
        TranscribeProofreader("m1", ProofreadOptions(batch_segments=1, context_chars=0))
        .run(_segments("第一窗原文呃", "第二窗原文"))
    )
    assert result.applied is True
    assert result.text == "第一窗纠错。 第二窗原文"
    assert result.fallback_batches == [1]
    assert result.windows == 2


def test_pipeline_progress_callback(monkeypatch, patched_model):
    _install_gateway(monkeypatch, ['{"0": "一"}', '{"0": "二"}'])
    seen = []
    asyncio.run(
        TranscribeProofreader(
            None,
            ProofreadOptions(batch_segments=1, context_chars=0),
            on_progress=lambda done, total: seen.append((done, total)),
        ).run(_segments("一", "二"))
    )
    assert seen[-1] == (2, 2)
    assert {total for _, total in seen} == {2}


# ---------------------------------------------------------------- 服务集成

@pytest.fixture
def service(monkeypatch, temp_dir):
    instance = FileTranscriptionService()

    class _Provider:
        provider_type = "whisper"
        model_name = "fake-medium"
        calls = 0

        async def transcribe(self, audio_path, language="zh", **kwargs):
            _Provider.calls += 1
            from app.records.models import TranscriptionResult, TranscriptionSegment

            return TranscriptionResult(
                segments=[
                    TranscriptionSegment(start=0.0, end=2.0, text="大家好呃今天学习机器学习", confidence=1.0, language="zh"),
                ],
                language="zh", language_probability=1.0, duration=2.0, processing_time=0.1,
            )

    monkeypatch.setattr(instance, "_get_provider", lambda engine, config: _Provider())
    monkeypatch.setattr(instance, "_cache_path", lambda: Path(temp_dir) / "transcribe_file_cache.json")
    monkeypatch.setattr(
        transcribe_file_router_module, "file_transcription_service", instance
    )
    monkeypatch.setattr(
        stt_factory, "get_asr_provider_config",
        lambda: ASRProviderConfig(
            provider=ASRProviderType.WHISPER, language="zh",
            whisper={"model_size": "tiny"}, qwen_asr={}, doubao_asr={"api_key": ""},
        ),
    )
    instance.fake_provider = _Provider
    return instance


@pytest.fixture
def client(service):
    app = FastAPI()
    app.include_router(transcribe_file_router)
    with TestClient(app) as test_client:
        yield test_client


def _media(temp_dir: Path) -> Path:
    path = Path(temp_dir) / "sample.mp3"
    path.write_bytes(b"fake")
    return path


def _no_ffmpeg(monkeypatch, service, duration=10.0):
    async def _probe(target):
        return duration

    async def _extract(target, engine):
        return None

    monkeypatch.setattr(service, "_probe_duration", _probe)
    monkeypatch.setattr(service, "_extract_audio_if_needed", _extract)


def _poll_done(client, job_id, timeout=10.0):
    deadline = time.time() + timeout
    payload = None
    while time.time() < deadline:
        response = client.get(f"/api/transcribe/jobs/{job_id}")
        payload = response.json()
        if payload["status"] != "running":
            return payload
        time.sleep(0.05)
    raise AssertionError(f"任务未在 {timeout}s 内完成：{payload}")


def test_proofread_forces_202_and_merges_result(
    client, service, monkeypatch, patched_model, temp_dir
):
    media = _media(temp_dir)
    _no_ffmpeg(monkeypatch, service)
    _install_gateway(monkeypatch, ['{"0": "大家好，今天学习机器学习。"}'])

    response = client.post(
        "/api/transcribe", json={"path": str(media), "proofread": True, "want_segments": True}
    )
    assert response.status_code == 202  # 短文件 + 校对 → 强制 202（设计 §4.1）

    job = _poll_done(client, response.json()["job_id"])
    assert job["status"] == "done"
    result = job["result"]
    assert result["raw_text"] == "大家好呃今天学习机器学习"
    assert result["text"] == "大家好，今天学习机器学习。"
    assert result["proofread"]["applied"] is True
    assert result["proofread"]["windows"] == 1
    assert [seg["text"] for seg in result["segments"]] == ["大家好，今天学习机器学习。"]


def test_proofread_without_llm_returns_409(client, service, monkeypatch, temp_dir):
    media = _media(temp_dir)

    def _raise(model_id):
        raise RuntimeError("尚未配置可用的 LLM 模型，请在设置页添加")

    monkeypatch.setattr(service_module, "resolve_proofread_model", _raise)
    response = client.post("/api/transcribe", json={"path": str(media), "proofread": True})
    assert response.status_code == 409
    assert "LLM" in response.json()["detail"]


def test_proofread_variant_cached_second_call_instant(
    client, service, monkeypatch, patched_model, temp_dir
):
    media = _media(temp_dir)
    _no_ffmpeg(monkeypatch, service)
    gateway = _install_gateway(monkeypatch, ['{"0": "大家好，今天学习机器学习。"}'])

    first = client.post("/api/transcribe", json={"path": str(media), "proofread": True})
    assert first.status_code == 202
    assert _poll_done(client, first.json()["job_id"])["status"] == "done"

    second = client.post("/api/transcribe", json={"path": str(media), "proofread": True})
    assert second.status_code == 200  # 校对变体命中 → 秒回
    body = second.json()
    assert body["cached"] is True
    assert body["proofread"]["applied"] is True
    assert body["raw_text"] and body["text"] == "大家好，今天学习机器学习。"
    assert gateway.calls == 1  # LLM 没有被二次调用
    assert service.fake_provider.calls == 1  # ASR 也没有


def test_proofread_reuses_cached_asr_bucket(
    client, service, monkeypatch, patched_model, temp_dir
):
    """先只转 ASR（同步 200），再开校对 → 202 只跑校对阶段，ASR 不重跑。"""
    media = _media(temp_dir)
    _no_ffmpeg(monkeypatch, service)

    plain = client.post("/api/transcribe", json={"path": str(media)})
    assert plain.status_code == 200
    assert service.fake_provider.calls == 1

    _install_gateway(monkeypatch, ['{"0": "大家好，今天学习机器学习。"}'])
    job_response = client.post("/api/transcribe", json={"path": str(media), "proofread": True})
    assert job_response.status_code == 202
    job = _poll_done(client, job_response.json()["job_id"])
    assert job["status"] == "done"
    assert job["result"]["proofread"]["applied"] is True
    assert service.fake_provider.calls == 1  # ASR 复用缓存桶，未重跑
