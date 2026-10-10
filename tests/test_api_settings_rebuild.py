"""settings_service 编排回归测试（原 update_settings 102 行下沉后的行为护栏）。

覆盖重构后必须保持不变的四条路径：
1. 模型运行时键变更 → 先 stop 模型服务 + 关闭流式会话管理器，再重建 provider；
2. provider 重建成功后必须**写回运行时状态宿主** `app.api.routes.asr_provider`；
3. provider 重建失败 → 500 中文错误，且此时设置已经落盘（与原实现一致）；
4. 非模型键 / 仅 recording_mode 的变更不得触发 provider 重建。

测试全部使用隔离的临时 settings.json，并把模型服务管理器的方法替换成假实现，
绝不触碰真实 data/ 与正在运行的模型服务。
"""
import pytest
from fastapi import FastAPI
from fastapi.testclient import TestClient

from app.api import routes
from app.config import UserSettings
from app.services import settings_service
from app.services.model_service_manager import model_service_manager


class _FakeProvider:
    """假 ASR provider：只记录 close 是否被调用。"""

    def __init__(self, name: str):
        self.name = name
        self.closed = False

    async def close(self) -> None:
        self.closed = True


@pytest.fixture
def rebuild_env(monkeypatch, temp_dir):
    """构造隔离的设置更新环境（含假 provider 与假模型服务管理器）。"""
    isolated_settings = UserSettings(temp_dir / "settings.json")
    old_provider = _FakeProvider("old")
    new_provider = _FakeProvider("new")
    calls = {"stop": 0, "configure": []}

    async def fake_stop():
        calls["stop"] += 1
        return {"status": "stopped"}

    def fake_configure(provider):
        calls["configure"].append(provider)

    monkeypatch.setattr(routes, "user_settings", isolated_settings)
    monkeypatch.setattr(routes, "asr_provider", old_provider)
    monkeypatch.setattr(routes, "streaming_session_manager", None)
    monkeypatch.setattr(model_service_manager, "stop", fake_stop)
    monkeypatch.setattr(model_service_manager, "configure", fake_configure)
    monkeypatch.setattr(settings_service, "create_asr_provider", lambda config: new_provider)
    monkeypatch.setattr(settings_service, "get_asr_provider_config", lambda: {})

    test_app = FastAPI()
    test_app.include_router(routes.router)
    return TestClient(test_app), isolated_settings, old_provider, new_provider, calls


def test_model_key_change_rebuilds_provider_and_writes_back_state(rebuild_env):
    """qwen_asr_model 变更：停服务 → 关闭旧 provider → 重建并写回运行时状态。"""
    client, isolated_settings, old_provider, new_provider, calls = rebuild_env

    response = client.put("/api/settings", json={"qwen_asr_model": "Qwen3-ASR-1.7B"})

    assert response.status_code == 200
    assert isolated_settings.get("qwen_asr_model") == "Qwen3-ASR-1.7B"
    assert calls["stop"] == 1
    assert old_provider.closed is True
    assert calls["configure"] == [new_provider]
    # 关键断言：重建后的 provider 必须写回聚合层状态宿主，否则子路由仍在用旧实例
    assert routes.asr_provider is new_provider


def test_rebuild_failure_returns_500_and_settings_already_persisted(rebuild_env, monkeypatch):
    """重建失败 → 500 中文说明；落在磁盘上的设置不回滚（与原实现一致）。"""
    client, isolated_settings, old_provider, new_provider, calls = rebuild_env

    def failing_factory(config):
        raise RuntimeError("模拟 provider 初始化失败")

    monkeypatch.setattr(settings_service, "create_asr_provider", failing_factory)

    response = client.put("/api/settings", json={"whisper_model": "medium"})

    assert response.status_code == 500
    assert "重新加载 ASR provider 失败" in response.json()["detail"]
    assert isolated_settings.get("whisper_model") == "medium"
    # 未成功重建：运行时状态必须保持旧 provider
    assert routes.asr_provider is old_provider
    assert calls["configure"] == []


def test_non_model_key_does_not_touch_model_service(rebuild_env):
    """普通设置键（auto_optimize）只落盘，不停模型服务、不重建 provider。"""
    client, isolated_settings, old_provider, _new_provider, calls = rebuild_env

    response = client.put("/api/settings", json={"auto_optimize": True})

    assert response.status_code == 200
    assert response.json()["auto_optimize"] is True
    assert calls["stop"] == 0
    assert calls["configure"] == []
    assert routes.asr_provider is old_provider
    assert old_provider.closed is False


def test_recording_mode_resets_runtime_without_rebuilding_provider(rebuild_env):
    """recording_mode 只在模型运行时键集合里：停服务但不重建 provider。"""
    client, isolated_settings, old_provider, _new_provider, calls = rebuild_env

    response = client.put("/api/settings", json={"recording_mode": "multi"})

    assert response.status_code == 200
    assert isolated_settings.get("recording_mode") == "multi"
    assert calls["stop"] == 1
    assert calls["configure"] == []
    assert old_provider.closed is False
    assert routes.asr_provider is old_provider


def test_invalid_settings_rejected_before_persisting(rebuild_env):
    """非法设置仍然 400 且不落盘、不停服务（校验先于写入）。"""
    client, isolated_settings, old_provider, _new_provider, calls = rebuild_env

    response = client.put("/api/settings", json={"qwen_asr_max_new_tokens": "abc"})

    assert response.status_code == 400
    assert "设置项校验失败" in response.json()["detail"]
    assert isolated_settings.get("qwen_asr_max_new_tokens") == 256
    assert calls["stop"] == 0
