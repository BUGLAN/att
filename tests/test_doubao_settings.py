"""豆包 ASR 设置项回归测试。

锁死的点：
1. 明文 Access Token 只写不读：PUT 后落盘的是密文，settings.json 里没有明文键；
2. GET /api/settings 只回报 doubao_asr_api_key_set，不回传明文/密文；
3. 空串视为「本次未提供」，不会把已保存的密钥清掉；清除要走显式开关；
4. asr_provider 枚举接受 doubao_asr，非法值仍然 400；
5. 豆包接入点 / Resource ID / 强制断句时长可持久化，并触发 provider 重建。
"""
import json

import pytest
from fastapi import FastAPI
from fastapi.testclient import TestClient

from app.api import routes
from app.config import UserSettings
from app.services import model_service_manager as model_service_manager_module
from app.services import settings_service
from app.services.model_service_manager import model_service_manager
from app.stt.models import ASRRuntimeCapabilities
from app.utils import encryption as encryption_module
from app.utils.encryption import APIKeyEncryption

PLAIN_TOKEN = "volc-access-token-for-test"


class _FakeProvider:
    """provider 替身：只记录 close 调用，并声明在线流式能力。"""

    provider_type = "doubao_asr"
    model_name = "bigmodel"

    def __init__(self):
        self.closed = 0

    async def close(self):
        self.closed += 1

    async def warmup(self):
        return None

    async def get_runtime_capabilities(self, recording_mode="single"):
        return ASRRuntimeCapabilities(
            model_service_supported=True,
            model_service_mode="online",
            streaming_supported=True,
            streaming_available=True,
            streaming_backend="doubao_sauc",
        )


class _NoopBridgeTarget:
    """流式桥接目标替身：settings 测试不关心 Qwen 子进程。"""

    async def start(self, wait_until_ready: bool = False) -> dict:
        return {}

    async def stop(self) -> dict:
        return {}

    async def status(self) -> dict:
        return {
            "managed": False,
            "running": False,
            "loading": False,
            "ready": False,
            "pid": None,
            "base_url": None,
            "error": None,
            "health": None,
        }


@pytest.fixture
def settings_env(monkeypatch, temp_dir):
    isolated_settings = UserSettings(temp_dir / "settings.json")
    monkeypatch.setattr(routes, "user_settings", isolated_settings)
    monkeypatch.setattr(routes, "streaming_session_manager", None)
    monkeypatch.setattr(routes, "asr_provider", _FakeProvider())
    # 模型服务管理器是单例：模型运行时键变更会触发 stop()，需要它已绑定 provider
    monkeypatch.setattr(model_service_manager, "_asr_provider", _FakeProvider())
    # 桥接目标是 Qwen 本地子进程托管：这里替换成空实现，避免测试去探测/清理真实进程
    monkeypatch.setattr(
        model_service_manager_module,
        "get_streaming_bridge_target",
        lambda: _NoopBridgeTarget(),
    )
    monkeypatch.setattr(settings_service, "create_asr_provider", lambda config: _FakeProvider())
    # 加密密钥文件也隔离，避免污染真实 data/.encryption_key
    monkeypatch.setattr(
        encryption_module,
        "_encryption",
        APIKeyEncryption(str(temp_dir / ".encryption_key")),
    )

    app = FastAPI()
    app.include_router(routes.router)
    return TestClient(app), isolated_settings


def test_put_doubao_key_is_encrypted_at_rest(settings_env):
    client, user_settings = settings_env

    response = client.put(
        "/api/settings",
        json={
            "asr_provider": "doubao_asr",
            "doubao_asr_api_key": PLAIN_TOKEN,
        },
    )

    assert response.status_code == 200
    body = response.json()
    assert body["doubao_asr_api_key_set"] is True
    # 明文与密文都不得出现在响应里
    assert "doubao_asr_api_key" not in body
    assert "doubao_asr_api_key_encrypted" not in body

    persisted = json.loads(user_settings.settings_path.read_text(encoding="utf-8"))
    assert "doubao_asr_api_key" not in persisted
    assert persisted["doubao_asr_api_key_encrypted"] != PLAIN_TOKEN
    assert PLAIN_TOKEN not in user_settings.settings_path.read_text(encoding="utf-8")

    # 密文必须能解回原文（provider 重建时要用）
    assert encryption_module.get_encryption().decrypt(
        persisted["doubao_asr_api_key_encrypted"]
    ) == PLAIN_TOKEN


def test_get_settings_masks_doubao_secret(settings_env):
    client, user_settings = settings_env
    client.put("/api/settings", json={"doubao_asr_api_key": PLAIN_TOKEN})

    response = client.get("/api/settings")

    assert response.status_code == 200
    body = response.json()
    assert body["doubao_asr_api_key_set"] is True
    assert "doubao_asr_api_key_encrypted" not in body
    assert "doubao_asr_api_key" not in body


def test_get_settings_reports_not_configured(settings_env):
    client, _ = settings_env

    body = client.get("/api/settings").json()

    assert body["doubao_asr_api_key_set"] is False


def test_empty_key_keeps_existing_secret(settings_env):
    """空串 = 本次未提供：整份设置快照提交（含空表单）不能清掉已保存的密钥。"""
    client, user_settings = settings_env
    client.put("/api/settings", json={"doubao_asr_api_key": PLAIN_TOKEN})
    stored = user_settings.get("doubao_asr_api_key_encrypted")

    response = client.put(
        "/api/settings",
        json={"doubao_asr_api_key": "", "doubao_asr_end_window_size": 600},
    )

    assert response.status_code == 200
    assert user_settings.get("doubao_asr_api_key_encrypted") == stored
    assert response.json()["doubao_asr_api_key_set"] is True
    assert user_settings.get("doubao_asr_end_window_size") == 600


def test_explicit_clear_removes_secret(settings_env):
    client, user_settings = settings_env
    client.put("/api/settings", json={"doubao_asr_api_key": PLAIN_TOKEN})

    response = client.put("/api/settings", json={"doubao_asr_api_key_clear": True})

    assert response.status_code == 200
    assert user_settings.get("doubao_asr_api_key_encrypted") == ""
    assert response.json()["doubao_asr_api_key_set"] is False


def test_doubao_endpoint_and_resource_id_persisted(settings_env):
    client, user_settings = settings_env

    response = client.put(
        "/api/settings",
        json={
            "asr_provider": "doubao_asr",
            "doubao_asr_resource_id": "volc.bigasr.sauc.duration",
            "doubao_asr_endpoint": "wss://openspeech.bytedance.com/api/v3/sauc/bigmodel",
        },
    )

    assert response.status_code == 200
    assert user_settings.get("doubao_asr_resource_id") == "volc.bigasr.sauc.duration"
    assert user_settings.get("asr_provider") == "doubao_asr"


def test_invalid_provider_still_rejected(settings_env):
    client, user_settings = settings_env

    response = client.put("/api/settings", json={"asr_provider": "doubao"})

    assert response.status_code == 400
    assert user_settings.get("asr_provider") == "whisper"


def test_invalid_end_window_size_rejected(settings_env):
    client, user_settings = settings_env

    response = client.put("/api/settings", json={"doubao_asr_end_window_size": -1})

    assert response.status_code == 400
    assert "doubao_asr_end_window_size" in response.json()["detail"]
    assert user_settings.get("doubao_asr_end_window_size") == 0


def test_doubao_secret_change_triggers_provider_rebuild(settings_env, monkeypatch):
    """凭据变化必须重建 provider，否则运行时会一直用旧 token。"""
    client, _ = settings_env
    rebuilt = []

    def fake_create(config):
        rebuilt.append(config)
        return _FakeProvider()

    monkeypatch.setattr(settings_service, "create_asr_provider", fake_create)

    response = client.put("/api/settings", json={"doubao_asr_api_key": PLAIN_TOKEN})

    assert response.status_code == 200
    assert len(rebuilt) == 1
    # 重建后的 provider 会写回运行时状态宿主
    assert isinstance(routes.asr_provider, _FakeProvider)
