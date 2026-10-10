"""
流式桥接目标选择测试

覆盖约束 C14 / 需求 FR-13.3 的回归点：
在 remote_bridge 配置下，模型服务管理器**不得**拉起本地 9000 子进程，
且状态探测必须指向远端 ASR 服务。
"""
import pytest

from app.config import settings
from app.services import streaming_bridge_target as bridge_module
from app.services.model_service_manager import ModelServiceManager
from app.services import qwen_streaming_service_manager as local_manager_module


class _FakeCapabilities:
    """替身：模拟远端桥接模式下的运行能力。"""

    model_service_supported = True
    model_service_mode = "managed_streaming_bridge"
    model_service_reason = None
    streaming_supported = True
    streaming_available = False
    streaming_reason = "流式模型服务尚未启动"
    streaming_backend = "remote_bridge"


class _FakeProvider:
    """替身：只需要提供运行能力查询。"""

    provider_type = "qwen_asr_streaming"
    model_name = "Qwen/Qwen3-ASR-1.7B"

    async def get_runtime_capabilities(self, recording_mode: str = "single") -> _FakeCapabilities:
        return _FakeCapabilities()


@pytest.fixture(autouse=True)
def _reset_bridge_target():
    """每个用例前后清空缓存的桥接目标，避免用例之间互相影响。"""
    bridge_module.reset_streaming_bridge_target()
    yield
    bridge_module.reset_streaming_bridge_target()


def test_local_target_without_base_url(monkeypatch):
    """未配置远端地址时应回落到本地托管目标。"""
    monkeypatch.setattr(settings, "qwen_streaming_backend", "auto")
    monkeypatch.setattr(settings, "qwen_streaming_base_url", "")

    assert bridge_module.is_remote_bridge() is False
    assert isinstance(bridge_module.get_streaming_bridge_target(), bridge_module.LocalStreamingBridgeTarget)


def test_remote_target_when_remote_bridge_configured(monkeypatch):
    """显式 remote_bridge + 地址时应使用远端目标。"""
    monkeypatch.setattr(settings, "qwen_streaming_backend", "remote_bridge")
    monkeypatch.setattr(settings, "qwen_streaming_base_url", "http://asr:9000")

    target = bridge_module.get_streaming_bridge_target()

    assert bridge_module.is_remote_bridge() is True
    assert isinstance(target, bridge_module.RemoteStreamingBridgeTarget)
    assert target.base_url == "http://asr:9000"


def test_remote_target_when_auto_with_base_url(monkeypatch):
    """auto 模式下只要配了地址就走远端，与 QwenStreamingProvider 的判定保持一致。"""
    monkeypatch.setattr(settings, "qwen_streaming_backend", "auto")
    monkeypatch.setattr(settings, "qwen_streaming_base_url", "http://asr:9000")

    assert bridge_module.is_remote_bridge() is True
    assert isinstance(bridge_module.get_streaming_bridge_target(), bridge_module.RemoteStreamingBridgeTarget)


def test_target_is_cached_until_config_changes(monkeypatch):
    """同一配置重复获取应返回同一个对象，配置变化后重建。"""
    monkeypatch.setattr(settings, "qwen_streaming_backend", "remote_bridge")
    monkeypatch.setattr(settings, "qwen_streaming_base_url", "http://asr:9000")

    first = bridge_module.get_streaming_bridge_target()
    assert bridge_module.get_streaming_bridge_target() is first

    monkeypatch.setattr(settings, "qwen_streaming_base_url", "http://asr-2:9000")
    second = bridge_module.get_streaming_bridge_target()

    assert second is not first
    assert second.base_url == "http://asr-2:9000"


def test_remote_status_reports_unavailable_when_service_unreachable(monkeypatch):
    """远端服务不可达时，状态必须是「明确不可用」而不是伪装成功。"""
    monkeypatch.setattr(settings, "qwen_streaming_backend", "remote_bridge")
    monkeypatch.setattr(settings, "qwen_streaming_base_url", "http://127.0.0.1:9")
    monkeypatch.setattr(settings, "qwen_streaming_timeout_seconds", 1.0)

    import asyncio

    target = bridge_module.get_streaming_bridge_target()
    status = asyncio.run(target.status())

    assert status["running"] is False
    assert status["ready"] is False
    assert status["managed"] is False
    assert "无法连接远端流式 ASR 服务" in (status["error"] or "")


def test_remote_stop_does_not_touch_local_subprocess(monkeypatch):
    """远端模式下 stop 不应触发本地子进程启停（生命周期由容器编排托管）。"""
    monkeypatch.setattr(settings, "qwen_streaming_backend", "remote_bridge")
    monkeypatch.setattr(settings, "qwen_streaming_base_url", "http://127.0.0.1:9")
    monkeypatch.setattr(settings, "qwen_streaming_timeout_seconds", 1.0)

    calls: list[str] = []
    monkeypatch.setattr(
        local_manager_module.qwen_streaming_service_manager,
        "stop",
        lambda *args, **kwargs: calls.append("stop") or {},
    )

    import asyncio

    asyncio.run(bridge_module.get_streaming_bridge_target().stop())

    assert calls == []


def test_model_service_status_uses_remote_target_without_local_spawn(monkeypatch):
    """
    C14 核心回归：远端模式下模型服务状态查询必须探测远端，且不得拉起本地子进程。
    """
    monkeypatch.setattr(settings, "qwen_streaming_backend", "remote_bridge")
    monkeypatch.setattr(settings, "qwen_streaming_base_url", "http://127.0.0.1:9")
    monkeypatch.setattr(settings, "qwen_streaming_timeout_seconds", 1.0)

    def _forbidden_start(*args, **kwargs):
        raise AssertionError("远端桥接模式下不应拉起本地 ASR 子进程（约束 C14）")

    monkeypatch.setattr(local_manager_module.qwen_streaming_service_manager, "start", _forbidden_start)

    import asyncio

    manager = ModelServiceManager()
    manager.configure(_FakeProvider())

    status = asyncio.run(manager.status())

    assert status["model_service_mode"] == "managed_streaming_bridge"
    # 远端不可达：必须是明确的不可用状态，而不是误报可用
    assert status["ready"] is False
    assert status["streaming_available"] is False
    # managed 应如实反映「不由本进程托管生命周期」
    assert status["managed"] is False
    assert status["base_url"] == "http://127.0.0.1:9"


def test_remote_status_is_not_loading_when_model_is_idle_and_unloaded(monkeypatch):
    """
    回归：远端可达、但模型「未加载且未在加载」时，loading 必须为 False。

    背景（实测踩坑）：前端把 model_service_loading=true 渲染成「模型加载中」，
    并据此**禁用「开启模型」按钮**。若此处把「未就绪」误报成「加载中」，
    用户就会陷入「流式用不了、加载按钮又点不动」的死局。
    """
    monkeypatch.setattr(settings, "qwen_streaming_backend", "remote_bridge")
    monkeypatch.setattr(settings, "qwen_streaming_base_url", "http://asr:9000")

    import asyncio

    target = bridge_module.get_streaming_bridge_target()

    async def _fake_probe() -> dict:
        # 容器刚启动、尚未预热时的真实响应
        return {
            "ready": False,
            "loaded": False,
            "loading": False,
            "engine_warmed": False,
            "model_name": "Qwen/Qwen3-ASR-1.7B",
        }

    monkeypatch.setattr(target, "_probe_health", _fake_probe)

    status = asyncio.run(target.status())

    assert status["running"] is True
    assert status["ready"] is False
    assert status["loading"] is False, "未加载且未在加载时不得上报 loading，否则前端按钮被锁死"
    assert status["error"] is None


def test_remote_status_reports_loading_while_model_is_loading(monkeypatch):
    """反向保证：模型确实在加载时必须如实上报 loading=True。"""
    monkeypatch.setattr(settings, "qwen_streaming_backend", "remote_bridge")
    monkeypatch.setattr(settings, "qwen_streaming_base_url", "http://asr:9000")

    import asyncio

    target = bridge_module.get_streaming_bridge_target()

    async def _fake_probe() -> dict:
        return {
            "ready": False,
            "loaded": False,
            "loading": True,
            "engine_warmed": False,
            "model_name": "Qwen/Qwen3-ASR-1.7B",
        }

    monkeypatch.setattr(target, "_probe_health", _fake_probe)

    status = asyncio.run(target.status())

    assert status["ready"] is False
    assert status["loading"] is True
