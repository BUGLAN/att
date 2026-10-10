"""豆包 ASR 接线测试：工厂、流式会话管理器分派、health 字段。

这些是「新 provider 真的接进链路了吗」的护栏：
1. factory 能按 asr_provider=doubao_asr 造出 provider，并从加密设置里取回密钥；
2. StreamingSessionManager 会按当前 provider 分派到豆包流式实现（不再硬编码 Qwen）；
3. 非流式 provider 走流式录音时给出可读错误；
4. /api/health 的 streaming_available 对在线 provider 直接来自运行时能力。
"""
import pytest

from app.services import streaming_session_manager as manager_module
from app.services.streaming_session_manager import StreamingSessionManager
from app.stt.factory import create_asr_provider, get_asr_provider_config
from app.stt.models import ASRProviderError
from app.stt.providers.doubao_asr_provider import DoubaoASRProvider, DoubaoStreamingProvider
from app.stt.providers.whisper_provider import WhisperProvider


def test_factory_creates_doubao_provider_from_runtime_settings():
    config = get_asr_provider_config(
        {
            "asr_provider": "doubao_asr",
            "doubao_asr_api_key": "runtime-token",
            "doubao_asr_resource_id": "volc.bigasr.sauc.duration",
            "doubao_asr_end_window_size": 600,
        }
    )

    provider = create_asr_provider(config)

    assert isinstance(provider, DoubaoASRProvider)
    assert provider.provider_type == "doubao_asr"
    assert provider.model_name == "bigmodel"
    assert provider.api_key == "runtime-token"
    assert provider.resource_id == "volc.bigasr.sauc.duration"
    assert provider.streaming_provider.end_window_size == 600


def test_factory_resolves_encrypted_user_setting(monkeypatch, temp_dir):
    """用户设置里的密钥是密文，factory 必须解回明文；解密失败则回退环境变量。"""
    from app.utils import encryption as encryption_module
    from app.utils.encryption import APIKeyEncryption

    isolated = APIKeyEncryption(str(temp_dir / ".encryption_key"))
    monkeypatch.setattr(encryption_module, "_encryption", isolated)
    encrypted = isolated.encrypt("stored-token")

    config = get_asr_provider_config(
        {
            "asr_provider": "doubao_asr",
            "doubao_asr_api_key_encrypted": encrypted,
        }
    )

    assert config.doubao_asr["api_key"] == "stored-token"


def test_streaming_manager_dispatches_to_doubao(monkeypatch):
    monkeypatch.setattr(
        manager_module,
        "get_asr_provider_config",
        lambda: get_asr_provider_config(
            {
                "asr_provider": "doubao_asr",
                "doubao_asr_api_key": "runtime-token",
            }
        ),
    )
    manager = StreamingSessionManager(record_manager=None, optimizer=None)

    provider = manager._get_provider()

    assert isinstance(provider, DoubaoStreamingProvider)
    assert provider.provider_type == "doubao_asr_streaming"
    assert provider.chunk_samples == 3200  # 200ms @16kHz


def test_streaming_manager_rejects_non_streaming_provider(monkeypatch):
    monkeypatch.setattr(
        manager_module,
        "get_asr_provider_config",
        lambda: get_asr_provider_config({"asr_provider": "whisper", "whisper_model": "tiny"}),
    )
    manager = StreamingSessionManager(record_manager=None, optimizer=None)

    with pytest.raises(RuntimeError) as error:
        manager._get_provider()

    assert "不支持真流式录音" in str(error.value)


def test_health_reports_doubao_streaming_availability(monkeypatch):
    """在线 provider 不需要「开启模型」，health 直接回报可用性。"""
    import asyncio

    from app.config import settings as app_settings
    from app.services import model_service
    from app.services.model_service_manager import model_service_manager

    provider = DoubaoASRProvider(api_key="", endpoint="wss://example.invalid/x")
    monkeypatch.setattr(model_service_manager, "_asr_provider", provider)

    class _UserSettings:
        def get(self, key, default=None):
            return {
                "asr_provider": "doubao_asr",
                "doubao_asr_api_key": "",
            }.get(key, default)

    payload = asyncio.run(
        model_service.build_health_payload(
            app_settings=app_settings,
            user_settings=_UserSettings(),
            asr_provider=provider,
            probe_runtime=False,
            streaming_manager_getter=lambda: None,
        )
    )

    assert payload["asr_model"] == "bigmodel"
    assert payload["streaming_backend"] == "doubao_sauc"
    # 未配置密钥时可用性必须是 False，并带出可读原因
    assert payload["streaming_available"] is False
    assert "Access Token" in (payload["streaming_error"] or "")


def test_whisper_provider_still_created_by_factory():
    provider = create_asr_provider(get_asr_provider_config({"asr_provider": "whisper"}))

    assert isinstance(provider, WhisperProvider)


class _NoopBridge:
    async def start(self, wait_until_ready: bool = False) -> dict:
        return {}

    async def stop(self) -> dict:
        return {}

    async def status(self) -> dict:
        return {"managed": False, "running": False, "loading": False, "ready": False, "error": None}


def _doubao_manager(monkeypatch, provider):
    """构造一个绑定指定 provider 的模型服务管理器（替换掉真实桥接探测）。"""
    from app.services import model_service_manager as module

    monkeypatch.setattr(module, "get_streaming_bridge_target", lambda: _NoopBridge())
    manager = module.ModelServiceManager()
    manager.configure(provider)
    return manager


def test_model_service_ready_for_online_provider_without_manual_start(monkeypatch):
    """在线 provider 不需要点「开启模型」：凭据配齐就必须是就绪态，否则录音按钮会一直禁用。"""
    import asyncio

    manager = _doubao_manager(
        monkeypatch,
        DoubaoASRProvider(api_key="runtime-token", endpoint="wss://example.invalid/x"),
    )

    status = asyncio.run(manager.status())

    assert status["model_service_mode"] == "online"
    assert status["ready"] is True
    assert status["streaming_available"] is True
    assert status["streaming_backend"] == "doubao_sauc"


def test_model_service_not_ready_for_online_provider_without_key(monkeypatch):
    import asyncio

    manager = _doubao_manager(
        monkeypatch,
        DoubaoASRProvider(api_key="", endpoint="wss://example.invalid/x"),
    )

    status = asyncio.run(manager.status())

    assert status["ready"] is False
    assert status["streaming_available"] is False
    assert "Access Token" in status["error"]


def test_doubao_provider_error_is_asr_provider_error():
    provider = DoubaoASRProvider(api_key="", endpoint="wss://example.invalid/x")

    with pytest.raises(ASRProviderError):
        provider.streaming_provider.ensure_config()
