"""WS /api/asr/stream 回归测试。

覆盖审计报告 §3.2 第 3 条与任务 T4 第 3 项：
- 收到 websocket.disconnect 后必须调用 cancel_session 释放会话（原实现直接 break→return，会话泄漏）；
- get_streaming_session_manager() 抛异常时不得因为 manager 未绑定而 NameError；
- 对可能已关闭的 socket 发送错误帧要兜住异常。
测试用假 manager 替换真实流式实现，不加载任何模型。
"""
import importlib
import time
from pathlib import Path

import pytest
from fastapi import FastAPI
from fastapi.testclient import TestClient

from app.api import routes


class _FakeStreamingManager:
    """记录会话生命周期的假管理器。"""

    def __init__(self, fail_append: bool = False):
        self.created = []
        self.cancelled = []
        self.finished = []
        self.fail_append = fail_append

    async def create_session(self, language="zh", context=""):
        session_id = f"session-{len(self.created) + 1}"
        self.created.append(session_id)
        return {"session_id": session_id, "language": language, "sample_rate": 16000}

    async def append_audio(self, session_id, payload):
        if self.fail_append:
            raise RuntimeError("模拟流式追加失败")
        return {"text": "部分文本", "language": "zh", "duration": 0.1}

    async def finish_session(self, session_id):
        self.finished.append(session_id)
        return {"raw_text": "最终文本", "duration": 0.1}

    async def cancel_session(self, session_id):
        self.cancelled.append(session_id)


@pytest.fixture
def ws_client(monkeypatch):
    """构造只挂载 api 路由的测试应用。"""
    app = FastAPI()
    app.include_router(routes.router)
    return TestClient(app)


def _wait_until(predicate, timeout: float = 5.0, interval: float = 0.02) -> bool:
    """在超时时间内轮询等待条件成立。"""
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if predicate():
            return True
        time.sleep(interval)
    return predicate()


def test_disconnect_cancels_session(ws_client, monkeypatch):
    """客户端断开（websocket.disconnect 帧）后必须释放会话，不能泄漏。"""
    fake_manager = _FakeStreamingManager()
    monkeypatch.setattr(routes, "streaming_session_manager", fake_manager)

    with ws_client.websocket_connect("/api/asr/stream") as websocket:
        websocket.send_json({"type": "stream_start", "language": "zh"})
        ready = websocket.receive_json()
        assert ready["type"] == "stream_ready"
        session_id = ready["data"]["session_id"]

        # 模拟客户端断开：服务端 receive() 会拿到 websocket.disconnect
        websocket.send({"type": "websocket.disconnect", "code": 1000})
        assert _wait_until(lambda: session_id in fake_manager.cancelled)

    assert fake_manager.created == [session_id]
    assert fake_manager.cancelled == [session_id]


def test_explicit_cancel_is_not_repeated(ws_client, monkeypatch):
    """stream_cancel 已经释放过会话，退出时不得重复释放。"""
    fake_manager = _FakeStreamingManager()
    monkeypatch.setattr(routes, "streaming_session_manager", fake_manager)

    with ws_client.websocket_connect("/api/asr/stream") as websocket:
        websocket.send_json({"type": "stream_start"})
        session_id = websocket.receive_json()["data"]["session_id"]

        websocket.send_json({"type": "stream_cancel"})
        cancelled = websocket.receive_json()
        assert cancelled["type"] == "stream_cancelled"

        websocket.send({"type": "websocket.disconnect", "code": 1000})
        assert _wait_until(lambda: session_id in fake_manager.cancelled)

    assert fake_manager.cancelled == [session_id]


def test_finish_clears_session_without_extra_cancel(ws_client, monkeypatch):
    """stream_finish 正常结束后不应再触发 cancel_session。"""
    fake_manager = _FakeStreamingManager()
    monkeypatch.setattr(routes, "streaming_session_manager", fake_manager)

    with ws_client.websocket_connect("/api/asr/stream") as websocket:
        websocket.send_json({"type": "stream_start"})
        session_id = websocket.receive_json()["data"]["session_id"]

        websocket.send_json({"type": "stream_finish"})
        finished = websocket.receive_json()
        assert finished["type"] == "stream_finished"

        websocket.send({"type": "websocket.disconnect", "code": 1000})
        time.sleep(0.1)

    assert fake_manager.finished == [session_id]
    assert fake_manager.cancelled == []


def test_append_failure_reports_error_and_releases_session(ws_client, monkeypatch):
    """追加音频失败时：返回 stream_error 并释放会话（原实现会在 except 里再次引用 manager）。"""
    fake_manager = _FakeStreamingManager(fail_append=True)
    monkeypatch.setattr(routes, "streaming_session_manager", fake_manager)

    with ws_client.websocket_connect("/api/asr/stream") as websocket:
        websocket.send_json({"type": "stream_start"})
        session_id = websocket.receive_json()["data"]["session_id"]

        websocket.send_bytes(b"\x00\x00" * 512)
        error_message = websocket.receive_json()
        assert error_message["type"] == "stream_error"
        assert "模拟流式追加失败" in error_message["data"]["error"]
        assert _wait_until(lambda: session_id in fake_manager.cancelled)


def test_manager_unavailable_reports_error_without_name_error(ws_client, monkeypatch):
    """get_streaming_session_manager() 抛异常时必须回 stream_error，而不是 NameError 导致连接静默关闭。"""
    def _raise_manager_error():
        raise RuntimeError("流式会话管理器未就绪")

    monkeypatch.setattr(routes, "get_streaming_session_manager", _raise_manager_error)

    with ws_client.websocket_connect("/api/asr/stream") as websocket:
        message = websocket.receive_json()

    assert message["type"] == "stream_error"
    assert "流式会话管理器未就绪" in message["data"]["error"]


def test_ping_pong_and_unknown_command(ws_client, monkeypatch):
    """协议保持：ping→pong；未知命令→stream_error。"""
    fake_manager = _FakeStreamingManager()
    monkeypatch.setattr(routes, "streaming_session_manager", fake_manager)

    with ws_client.websocket_connect("/api/asr/stream") as websocket:
        websocket.send_json({"type": "ping"})
        assert websocket.receive_json() == {"type": "pong"}

        websocket.send_json({"type": "unknown_command"})
        error_message = websocket.receive_json()
        assert error_message["type"] == "stream_error"
        assert "未知的流式命令" in error_message["data"]["error"]


def test_audio_before_start_reports_error(ws_client, monkeypatch):
    """未 stream_start 就发送音频：报错而不是崩溃。"""
    fake_manager = _FakeStreamingManager()
    monkeypatch.setattr(routes, "streaming_session_manager", fake_manager)

    with ws_client.websocket_connect("/api/asr/stream") as websocket:
        websocket.send_bytes(b"\x00\x00" * 16)
        error_message = websocket.receive_json()

    assert error_message["type"] == "stream_error"
    assert "流式会话尚未启动" in error_message["data"]["error"]


# ==================== STT provider 源码编码回归 ====================
# 说明：WS /api/asr/stream 由 QwenStreamingProvider（qwen_streaming_runtime_provider.py）
# 提供，因此这三个 provider 源文件的编码状态直接决定流式链路的可运行性，
# 乱码回归测试放在本文件内（任务 T4 只允许改动三个测试文件）。

PROJECT_ROOT = Path(__file__).resolve().parent.parent

PROVIDER_SOURCE_FILES = (
    "app/stt/providers/whisper_provider.py",
    "app/stt/providers/qwen_asr_provider.py",
    "app/stt/providers/qwen_streaming_runtime_provider.py",
)

PROVIDER_MODULES = (
    "app.stt.providers.whisper_provider",
    "app.stt.providers.qwen_asr_provider",
    "app.stt.providers.qwen_streaming_runtime_provider",
)


@pytest.mark.parametrize("relative_path", PROVIDER_SOURCE_FILES)
def test_provider_source_has_no_private_use_characters(relative_path):
    """三个 provider 源文件不得再含私用区字符（U+E000–U+F8FF，GBK 乱码的典型残留）。"""
    text = (PROJECT_ROOT / relative_path).read_text(encoding="utf-8")
    offenders = sorted({char for char in text if 0xE000 <= ord(char) <= 0xF8FF})

    assert offenders == [], (
        f"{relative_path} 仍含私用区字符: {[hex(ord(char)) for char in offenders]}"
    )


@pytest.mark.parametrize("module_name", PROVIDER_MODULES)
def test_provider_module_importable(module_name):
    """三个 provider 模块必须能正常 import（乱码不会破坏语法）。"""
    module = importlib.import_module(module_name)

    assert module is not None


@pytest.mark.parametrize(
    ("relative_path", "expected_text"),
    (
        ("app/stt/providers/whisper_provider.py", "faster-whisper 封装"),
        ("app/stt/providers/whisper_provider.py", "检测到 Apple Silicon，切换到 Core ML 兼容模式"),
        ("app/stt/providers/whisper_provider.py", "Whisper 转写完成"),
        ("app/stt/providers/qwen_asr_provider.py", "Qwen3-ASR provider 实现"),
        ("app/stt/providers/qwen_asr_provider.py", "Qwen3-ASR 依赖未安装，请先执行"),
        ("app/stt/providers/qwen_asr_provider.py", "Qwen3-ASR 转写完成"),
        ("app/stt/providers/qwen_streaming_runtime_provider.py", "远端桥接 Qwen 流式会话"),
        ("app/stt/providers/qwen_streaming_runtime_provider.py", "远端 Qwen 流式服务健康检查返回了无效 JSON"),
        ("app/stt/providers/qwen_streaming_runtime_provider.py", "创建远端 Qwen 流式会话失败"),
    ),
)
def test_garbled_messages_restored(relative_path, expected_text):
    """乱码还原后的中文（含用户可见错误提示）必须真实存在于源码中。"""
    text = (PROJECT_ROOT / relative_path).read_text(encoding="utf-8")

    assert expected_text in text
