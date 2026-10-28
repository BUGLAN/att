"""ChatGPT 听写 provider 测试：移植逻辑的关键行为（对应 codex-asr 的实现）。

锁死的点：
1. 鉴权四件套：Bearer / originator / 桌面版 UA / ChatGPT-Account-Id（从 JWT 提取），
   缺任何一个都可能被边缘风控拦成 403；
2. multipart 只带 file + 可选 language，不带 session/attempt 等多余字段；
3. HTTP 状态码归一成可执行中文（401/403 风控/429/5xx）；403 要区分 HTML 风控页；
4. text 为空是合法结果（音频没有语音），不得抛错；
5. 令牌明文不出现在任何异常文案里。
"""
from __future__ import annotations

import asyncio
import base64
import json
import wave
import math
import struct
from pathlib import Path

import httpx
import pytest

from app.stt.factory import get_asr_provider_config, create_asr_provider
from app.stt.models import ASRProviderError, ASRProviderType
from app.stt.providers.chatgpt_asr_provider import (
    DEFAULT_ENDPOINT,
    ChatGPTASRProvider,
    account_id_from_access_token,
    strip_bearer_prefix,
)


def _fake_jwt(account_id: str = "acct-1234") -> str:
    """构造只含 chatgpt_account_id 声明的伪 JWT（签名段随意，客户端不校验）。"""
    header = base64.urlsafe_b64encode(b'{"alg":"RS256"}').rstrip(b"=").decode()
    claims = {
        "https://api.openai.com/auth": {"chatgpt_account_id": account_id},
    }
    payload = base64.urlsafe_b64encode(json.dumps(claims).encode()).rstrip(b"=").decode()
    return f"{header}.{payload}.sig"


def _make_wav(path: Path, seconds: float = 0.2) -> Path:
    with wave.open(str(path), "wb") as w:
        w.setnchannels(1)
        w.setsampwidth(2)
        w.setframerate(16000)
        frames = b"".join(
            struct.pack("<h", int(3000 * math.sin(2 * math.pi * 440 * i / 16000)))
            for i in range(int(16000 * seconds))
        )
        w.writeframes(frames)
    return path


class _FakeResponse:
    def __init__(self, status_code: int, text: str = ""):
        self.status_code = status_code
        self._text = text

    def json(self):
        return json.loads(self._text)

    @property
    def text(self):
        return self._text


# ==================== 工具函数 ====================

def test_strip_bearer_prefix_tolerates_pasted_prefix():
    assert strip_bearer_prefix("Bearer abc") == "abc"
    assert strip_bearer_prefix("bearer abc") == "abc"
    assert strip_bearer_prefix("abc") == "abc"
    assert strip_bearer_prefix("  ") == ""


def test_account_id_extraction_from_jwt():
    assert account_id_from_access_token(_fake_jwt("acct-1234")) == "acct-1234"
    # 提取失败（非 JWT / 缺声明）返回空串，不抛错
    assert account_id_from_access_token("not-a-jwt") == ""
    assert account_id_from_access_token("a.b.c") == ""


# ==================== 鉴权头 ====================

def test_auth_headers_contain_all_four_required_fields():
    provider = ChatGPTASRProvider(api_key=_fake_jwt("acct-5678"))
    headers = provider._auth_headers()

    assert headers["Authorization"].startswith("Bearer ")
    assert headers["originator"] == "Codex Desktop"
    assert headers["User-Agent"].startswith("Codex Desktop/")
    assert "windows" in headers["User-Agent"] or "linux" in headers["User-Agent"] or "macos" in headers["User-Agent"]
    assert headers["ChatGPT-Account-Id"] == "acct-5678"


def test_auth_headers_omit_account_id_when_extraction_fails():
    provider = ChatGPTASRProvider(api_key="not-a-jwt")
    headers = provider._auth_headers()
    assert "ChatGPT-Account-Id" not in headers


# ==================== 配置校验 ====================

def test_ensure_config_requires_token():
    with pytest.raises(ASRProviderError) as excinfo:
        ChatGPTASRProvider(api_key="").ensure_config()
    assert "设置" in str(excinfo.value)


def test_ensure_config_rejects_non_http_endpoint():
    with pytest.raises(ASRProviderError):
        ChatGPTASRProvider(api_key="tok", endpoint="chatgpt.com/transcribe").ensure_config()


def test_runtime_capabilities_online_ready_and_no_streaming(tmp_path):
    provider = ChatGPTASRProvider(api_key="tok")

    caps = asyncio.run(provider.get_runtime_capabilities("single"))
    assert caps.model_service_mode == "online"
    assert caps.online_ready is True
    assert caps.streaming_supported is False

    streaming_caps = asyncio.run(provider.get_runtime_capabilities("streaming"))
    assert streaming_caps.model_service_supported is False
    assert "流式" in (streaming_caps.model_service_reason or "")


# ==================== 转写请求 ====================

def test_transcribe_sends_expected_multipart_and_parses_text(tmp_path, monkeypatch):
    wav = _make_wav(tmp_path / "speech.wav")
    captured: dict = {}

    def fake_post(url, *, headers, files, data, timeout):
        captured.update(url=url, headers=headers, files=files, data=data)
        return _FakeResponse(200, json.dumps({"text": "你好，测试文本", "asset_format": "wav"}))

    monkeypatch.setattr(
        "app.stt.providers.chatgpt_asr_provider.httpx.post", fake_post
    )
    provider = ChatGPTASRProvider(api_key=_fake_jwt("acct-9"))
    result = asyncio.run(provider.transcribe(str(wav), "zh"))

    assert captured["url"] == DEFAULT_ENDPOINT
    # multipart 只带 file 与 language，不带 session/attempt/duration
    assert set(captured["data"].keys()) == {"language"}
    filename, _payload, content_type = captured["files"]["file"]
    assert filename.endswith(".wav")
    assert content_type == "audio/wav"
    # 结果
    assert result.segments[0].text == "你好，测试文本"
    assert result.duration == pytest.approx(0.2, abs=0.05)


def test_transcribe_omits_language_when_auto(tmp_path, monkeypatch):
    wav = _make_wav(tmp_path / "s.wav")
    captured: dict = {}

    def fake_post(url, *, headers, files, data, timeout):
        captured["data"] = data
        return _FakeResponse(200, json.dumps({"text": "x"}))

    monkeypatch.setattr(
        "app.stt.providers.chatgpt_asr_provider.httpx.post", fake_post
    )
    asyncio.run(ChatGPTASRProvider(api_key="tok").transcribe(str(wav), "auto"))
    assert captured["data"] == {}


def test_transcribe_empty_text_is_legal(tmp_path, monkeypatch):
    wav = _make_wav(tmp_path / "s.wav")

    def fake_post(url, *, headers, files, data, timeout):
        return _FakeResponse(200, json.dumps({"text": ""}))

    monkeypatch.setattr(
        "app.stt.providers.chatgpt_asr_provider.httpx.post", fake_post
    )
    result = asyncio.run(ChatGPTASRProvider(api_key="tok").transcribe(str(wav), "zh"))
    assert result.segments == []
    assert result.duration > 0


# ==================== 错误归一 ====================

@pytest.mark.parametrize(
    "status_code,body,expect_fragment",
    [
        (401, '{"error":"unauthorized"}', "令牌"),
        (403, "<html>cloudflare</html>", "风控"),
        (403, '{"detail":"Forbidden"}', "403"),
        (404, "{}", "404"),
        (413, "{}", "过大"),
        (429, "{}", "限流"),
        (500, "{}", "故障"),
    ],
)
def test_error_classification_by_status(tmp_path, monkeypatch, status_code, body, expect_fragment):
    wav = _make_wav(tmp_path / "s.wav")

    def fake_post(url, *, headers, files, data, timeout):
        return _FakeResponse(status_code, body)

    monkeypatch.setattr(
        "app.stt.providers.chatgpt_asr_provider.httpx.post", fake_post
    )
    with pytest.raises(ASRProviderError) as excinfo:
        asyncio.run(ChatGPTASRProvider(api_key="tok-secret-value").transcribe(str(wav), "zh"))

    message = str(excinfo.value)
    assert expect_fragment in message
    # 令牌明文绝不进异常文案
    assert "tok-secret-value" not in message


def test_connection_error_mentions_service(tmp_path, monkeypatch):
    wav = _make_wav(tmp_path / "s.wav")

    def fake_post(url, *, headers, files, data, timeout):
        raise httpx.ConnectError("connection refused")

    monkeypatch.setattr(
        "app.stt.providers.chatgpt_asr_provider.httpx.post", fake_post
    )
    with pytest.raises(ASRProviderError) as excinfo:
        asyncio.run(ChatGPTASRProvider(api_key="tok").transcribe(str(wav), "zh"))
    assert "无法连接" in str(excinfo.value)


# ==================== 工厂装配 ====================

def test_factory_builds_chatgpt_provider_from_runtime_settings():
    config = get_asr_provider_config(
        {
            "asr_provider": "chatgpt_asr",
            "chatgpt_asr_api_key": "Bearer runtime-token",
            "chatgpt_asr_endpoint": "https://example.test/transcribe",
            "chatgpt_asr_timeout_seconds": 90,
        }
    )

    assert config.provider is ASRProviderType.CHATGPT_ASR
    assert config.chatgpt_asr["endpoint"] == "https://example.test/transcribe"
    assert config.chatgpt_asr["timeout_seconds"] == 90.0

    provider = create_asr_provider(config)
    assert isinstance(provider, ChatGPTASRProvider)
    assert provider.endpoint == "https://example.test/transcribe"
    # Bearer 前缀在 provider 构造时剥掉（容忍用户连前缀一起粘贴）
    assert provider.api_key == "runtime-token"


def test_factory_defaults_when_no_overrides():
    config = get_asr_provider_config({"asr_provider": "chatgpt_asr"})

    assert config.provider is ASRProviderType.CHATGPT_ASR
    assert config.chatgpt_asr["endpoint"] == DEFAULT_ENDPOINT
    assert config.chatgpt_asr["timeout_seconds"] == 120.0
