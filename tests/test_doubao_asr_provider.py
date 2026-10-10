"""火山豆包流式 provider 单测（假 socket，不联网）。

锁死的点：
1. 建连时三个握手头齐全，且全量请求带 result_type=full / show_utterances=true；
2. 音频按 200ms 切帧，末包 flags=0b0010；
3. 文本按 definite 前缀累计，中间结果 = 已确认 + 尾巴；
4. 错误帧/异常断连都归一化成可读中文并抛出；
5. finish 返回整场文字稿并关闭连接；
6. 批量转写复用同一条 SAUC 会话；
7. 缺凭据时给出可执行的提示，而不是等到推流才炸。
"""
import asyncio
import json
import struct
from collections import deque

import numpy as np
import pytest

from app.stt.models import ASRProviderError
from app.stt.providers import doubao_asr_provider as provider_module
from app.stt.providers.doubao_asr_provider import (
    DoubaoASRProvider,
    DoubaoStreamingProvider,
)
from app.stt.providers.volcengine_sauc import (
    MSG_AUDIO_ONLY_REQUEST,
    MSG_FULL_CLIENT_REQUEST,
    SaucSessionError,
    build_header,
)
from app.stt.providers.volcengine_sauc import (
    COMPRESSION_NONE,
    MSG_FULL_SERVER_RESPONSE,
    MSG_SERVER_ERROR_RESPONSE,
    SERIALIZATION_JSON,
    FLAG_NO_SEQUENCE,
)

FAKE_ENDPOINT = "wss://openspeech.example/api/v3/sauc/bigmodel"


class FakeSocket:
    """最小 websocket 替身：记录发出的帧，按队列吐回服务端帧。"""

    def __init__(self, inbox=None):
        self.sent = []
        self.inbox = deque(inbox or [])
        self.closed = False
        self.close_args = None

    def send(self, data):
        self.sent.append(data)

    def recv(self, timeout=None):
        if self.inbox:
            return self.inbox.popleft()
        raise TimeoutError()

    def close(self, code=1000, reason=""):
        self.closed = True
        self.close_args = (code, reason)


def server_result(payload: dict, *, last: bool = False, sequence: int = 1) -> bytes:
    body = json.dumps(payload, ensure_ascii=False).encode("utf-8")
    flags = 0b0011 if last else 0b0001
    header = build_header(MSG_FULL_SERVER_RESPONSE, flags, SERIALIZATION_JSON, COMPRESSION_NONE)
    return header + struct.pack(">i", sequence) + struct.pack(">I", len(body)) + body


def server_error(code: int, message: str) -> bytes:
    body = message.encode("utf-8")
    header = build_header(MSG_SERVER_ERROR_RESPONSE, FLAG_NO_SEQUENCE, SERIALIZATION_JSON, COMPRESSION_NONE)
    return header + struct.pack(">I", code) + struct.pack(">I", len(body)) + body


def sent_audio_frames(socket: FakeSocket):
    """返回 [(payload, flags)]，只取纯音频请求帧。"""
    frames = []
    for frame in socket.sent:
        if (frame[1] >> 4) != MSG_AUDIO_ONLY_REQUEST:
            continue
        size = struct.unpack(">I", frame[4:8])[0]
        frames.append((frame[8 : 8 + size], frame[1] & 0x0F))
    return frames


def make_provider(**overrides) -> DoubaoStreamingProvider:
    params = {
        "api_key": "test-token",
        "endpoint": FAKE_ENDPOINT,
        "drain_timeout_seconds": 0.0,
        "finish_timeout_seconds": 0.2,
        "finish_quiet_seconds": 0.05,
    }
    params.update(overrides)
    return DoubaoStreamingProvider(**params)


def patch_socket(monkeypatch, socket: FakeSocket, captured: dict | None = None) -> FakeSocket:
    def fake_connect(uri, **kwargs):
        if captured is not None:
            captured["uri"] = uri
            captured["kwargs"] = kwargs
        return socket

    monkeypatch.setattr(provider_module, "_ws_connect", fake_connect)
    return socket


# ==================== 建连 ====================


def test_create_session_sends_handshake_headers_and_full_request(monkeypatch):
    captured = {}
    socket = patch_socket(monkeypatch, FakeSocket(), captured)
    provider = make_provider()

    session = asyncio.run(provider.create_session(language="zh", context="上一次我们聊了版本范围"))

    headers = captured["kwargs"][provider_module._HEADER_KWARG]
    assert captured["uri"] == FAKE_ENDPOINT
    assert headers["X-Api-Key"] == "test-token"
    assert headers["X-Api-Resource-Id"] == "volc.seedasr.sauc.duration"
    assert headers["X-Api-Connect-Id"] == session.connect_id

    full_request = socket.sent[0]
    assert (full_request[1] >> 4) == MSG_FULL_CLIENT_REQUEST
    payload = json.loads(full_request[8:].decode("utf-8"))
    assert payload["request"]["result_type"] == "full"
    assert payload["request"]["show_utterances"] is True
    assert payload["audio"]["language"] == "zh-CN"
    context = json.loads(payload["request"]["corpus"]["context"])
    assert context["context_data"][0]["text"] == "上一次我们聊了版本范围"


def test_create_session_disables_system_proxy(monkeypatch):
    """websockets 15+ 的 proxy 默认为 True，会自动读取系统代理。

    macOS 上读的是系统代理设置而非环境变量，且 SOCKS 代理缺少 python-socks 时
    直接抛 ImportError，表现为“流式连接已断开”。建连必须显式 proxy=None 直连。
    """
    if not provider_module._PROXY_KWARG_SUPPORTED:
        pytest.skip("当前 websockets 版本不支持 proxy 参数，无需关闭代理")

    captured = {}
    patch_socket(monkeypatch, FakeSocket(), captured)
    provider = make_provider()

    asyncio.run(provider.create_session(language="zh"))

    assert captured["kwargs"]["proxy"] is None


def test_create_session_without_key_raises_readable_error(monkeypatch):
    patch_socket(monkeypatch, FakeSocket())
    provider = make_provider(api_key="")

    with pytest.raises(ASRProviderError) as error:
        asyncio.run(provider.create_session())

    assert "Access Token" in str(error.value)


def test_check_availability_reports_backend(monkeypatch):
    result = asyncio.run(make_provider().check_availability())

    assert result["available"] is True
    assert result["backend"] == "doubao_sauc"
    assert result["model_name"] == "bigmodel"


# ==================== 推流与结果 ====================


def test_append_audio_chunks_to_200ms(monkeypatch):
    socket = patch_socket(monkeypatch, FakeSocket())
    session = asyncio.run(make_provider().create_session())

    session.append_audio(np.arange(5000, dtype=np.int16))

    frames = sent_audio_frames(socket)
    assert len(frames) == 1
    assert len(frames[0][0]) == 6400  # 200ms @16kHz 单声道 16bit
    assert frames[0][1] == FLAG_NO_SEQUENCE


def test_append_audio_accumulates_definite_prefix(monkeypatch):
    socket = FakeSocket(
        [
            server_result(
                {
                    "audio_info": {"duration": 200},
                    "result": {
                        "text": "第一句。",
                        "utterances": [{"text": "第一句。", "definite": False}],
                    },
                }
            ),
            server_result(
                {
                    "audio_info": {"duration": 400},
                    "result": {
                        "text": "第一句。第二句",
                        "utterances": [
                            {"text": "第一句。", "definite": True, "start_time": 0, "end_time": 300},
                            {"text": "第二句", "definite": False},
                        ],
                    },
                }
            ),
        ]
    )
    patch_socket(monkeypatch, socket)
    session = asyncio.run(make_provider().create_session())

    update = session.append_audio(np.zeros(3200, dtype=np.int16))

    # 已确认句 + 未定稿尾巴，前端拿到的就是整场文本
    assert update.text == "第一句。第二句"
    assert update.is_final is False
    assert session.audio_duration_ms == 400


def test_append_audio_falls_back_to_cumulative_text_without_utterances(monkeypatch):
    socket = FakeSocket(
        [
            server_result({"result": {"text": "没有句级信息的一整段"}}),
        ]
    )
    patch_socket(monkeypatch, socket)
    session = asyncio.run(make_provider().create_session())

    update = session.append_audio(np.zeros(3200, dtype=np.int16))

    assert update.text == "没有句级信息的一整段"


def test_append_audio_ignores_unknown_frames(monkeypatch):
    """结构异常的帧必须静默忽略，不能断流。"""
    socket = FakeSocket([b"\x11\x91\x11\x00\x00\x00\x00\x63{}"])
    patch_socket(monkeypatch, socket)
    session = asyncio.run(make_provider().create_session())

    update = session.append_audio(np.zeros(3200, dtype=np.int16))

    assert update.text == ""


def test_error_frame_raises_normalized_error(monkeypatch):
    socket = FakeSocket([server_error(45000001, "invalid request")])
    patch_socket(monkeypatch, socket)
    session = asyncio.run(make_provider().create_session())

    with pytest.raises(SaucSessionError) as error:
        session.append_audio(np.zeros(3200, dtype=np.int16))

    assert "请求参数无效" in str(error.value)
    assert error.value.kind == "protocol"
    assert error.value.retryable is False


def test_server_close_reports_network_error(monkeypatch):
    class FakeClosed(Exception):
        def __init__(self, code, reason=""):
            super().__init__(reason)
            self.code = code
            self.reason = reason

    class ClosingSocket(FakeSocket):
        def recv(self, timeout=None):
            raise FakeClosed(1006, "abnormal closure")

    monkeypatch.setattr(provider_module, "ConnectionClosed", FakeClosed)
    socket = ClosingSocket()
    patch_socket(monkeypatch, socket)
    session = asyncio.run(make_provider().create_session())

    with pytest.raises(SaucSessionError) as error:
        session.append_audio(np.zeros(3200, dtype=np.int16))

    assert error.value.kind == "network"
    assert error.value.retryable is True


# ==================== 收尾 ====================


def test_finish_sends_final_packet_and_returns_full_text(monkeypatch):
    socket = FakeSocket(
        [
            server_result(
                {
                    "audio_info": {"duration": 800},
                    "result": {
                        "text": "第一句。第二句。",
                        "utterances": [
                            {"text": "第一句。", "definite": True},
                            {"text": "第二句。", "definite": True},
                        ],
                    },
                },
                last=True,
            )
        ]
    )
    patch_socket(monkeypatch, socket)
    session = asyncio.run(make_provider().create_session())

    session.append_audio(np.zeros(3200, dtype=np.int16))
    update = session.finish()

    assert update.is_final is True
    assert update.text == "第一句。第二句。"
    frames = sent_audio_frames(socket)
    assert frames[-1][1] == 0b0010  # 末包
    assert socket.closed is True


def test_finish_flushes_remainder_and_returns_pending_text(monkeypatch):
    """末包必须把不足 200ms 的余量带上；没有 definite 时返回待定文本。"""
    socket = FakeSocket(
        [server_result({"result": {"text": "只有待定内容", "utterances": [{"text": "只有待定内容", "definite": False}]}})]
    )
    patch_socket(monkeypatch, socket)
    session = asyncio.run(make_provider().create_session())

    session.append_audio(np.zeros(4000, dtype=np.int16))  # 3200 已发，剩 800
    update = session.finish()

    frames = sent_audio_frames(socket)
    assert frames[-1][1] == 0b0010
    assert len(frames[-1][0]) == 800 * 2
    assert update.text == "只有待定内容"


def test_cancel_closes_socket_without_raising(monkeypatch):
    socket = patch_socket(monkeypatch, FakeSocket())
    session = asyncio.run(make_provider().create_session())

    session.cancel()

    assert socket.closed is True
    assert socket.close_args == (1000, "client cancel")


# ==================== 批量转写 ====================


def test_batch_transcribe_reuses_sauc_session(monkeypatch, temp_dir):
    import soundfile as sf

    audio_path = temp_dir / "sample.wav"
    sf.write(str(audio_path), np.zeros(16000, dtype=np.int16), 16000)

    socket = FakeSocket(
        [
            server_result(
                {
                    "audio_info": {"duration": 1000},
                    "result": {
                        "text": "批量转写结果。",
                        "utterances": [{"text": "批量转写结果。", "definite": True}],
                    },
                },
                last=True,
            )
        ]
    )
    patch_socket(monkeypatch, socket)
    provider = DoubaoASRProvider(api_key="test-token", endpoint=FAKE_ENDPOINT)

    result = asyncio.run(provider.transcribe(str(audio_path), language="zh"))

    assert result.text == "批量转写结果。"
    assert result.duration == pytest.approx(1.0, abs=0.05)
    assert result.segments[0].end == pytest.approx(1.0, abs=0.05)
    assert socket.closed is True


def test_batch_transcribe_empty_audio_raises(monkeypatch, temp_dir):
    import soundfile as sf

    audio_path = temp_dir / "empty.wav"
    sf.write(str(audio_path), np.zeros(0, dtype=np.int16), 16000)

    patch_socket(monkeypatch, FakeSocket())
    provider = DoubaoASRProvider(api_key="test-token", endpoint=FAKE_ENDPOINT)

    with pytest.raises(ASRProviderError):
        asyncio.run(provider.transcribe(str(audio_path)))


# ==================== 运行时能力 ====================


def test_runtime_capabilities_streaming_available_when_configured():
    provider = DoubaoASRProvider(api_key="test-token", endpoint=FAKE_ENDPOINT)

    capabilities = asyncio.run(provider.get_runtime_capabilities("streaming"))

    assert capabilities.model_service_mode == "online"
    assert capabilities.streaming_supported is True
    assert capabilities.streaming_available is True
    assert capabilities.streaming_backend == "doubao_sauc"


def test_runtime_capabilities_streaming_blocked_without_key():
    provider = DoubaoASRProvider(api_key="", endpoint=FAKE_ENDPOINT)

    capabilities = asyncio.run(provider.get_runtime_capabilities("streaming"))

    assert capabilities.streaming_available is False
    assert "Access Token" in capabilities.streaming_reason
