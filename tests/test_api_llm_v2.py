"""LLM v2 API 契约测试（/api/llm/*）。

锁死的点：
- 全链路响应不得出现密钥明文/密文（AGENTS.md §3.4）；
- 验证类端点返回 200 + {ok, error_code}（验证失败是业务结果，不是 5xx）；
- 删除保护：默认模型/含默认模型的档案返回 409；
- verify-draft 的草稿密钥不落盘（store 数据始终不含草稿 key）。
"""
import pytest
from fastapi import FastAPI
from fastapi.testclient import TestClient

from app.api.routers import llm as llm_router_module
from app.api.routers.llm import router
from app.llm.gateway import VerifyOutcome
from app.llm.models import ModelItemCreate, ProfileCreate
from app.llm.store import LLMStore, reset_llm_store


class _FakeEncryption:
    def encrypt(self, value: str) -> str:
        return "enc:" + value

    def decrypt(self, value: str) -> str:
        if not value.startswith("enc:"):
            raise ValueError("invalid token")
        return value[len("enc:"):]


class _StubGateway:
    """替身网关：记录验证入参，返回脚本化结果。"""

    def __init__(self):
        self.calls = []
        self.outcome = VerifyOutcome(ok=True, latency_ms=123, remote_models=["deepseek-flash"])

    async def verify_profile(self, **kwargs):
        self.calls.append(("profile", kwargs))
        return self.outcome

    async def verify_model(self, **kwargs):
        self.calls.append(("model", kwargs))
        return self.outcome

    async def list_remote_models(self, **kwargs):
        self.calls.append(("models", kwargs))
        return ["deepseek-flash", "deepseek-v4-pro"], None


@pytest.fixture(autouse=True)
def fake_encryption(monkeypatch):
    import app.llm.migration as migration_module
    import app.llm.store as store_module

    monkeypatch.setattr(store_module, "get_encryption", _FakeEncryption)
    monkeypatch.setattr(migration_module, "get_encryption", _FakeEncryption)


@pytest.fixture
def client(tmp_path, monkeypatch):
    reset_llm_store()
    store = LLMStore(tmp_path)
    gateway = _StubGateway()
    monkeypatch.setattr(llm_router_module, "get_store", lambda: store)
    monkeypatch.setattr(llm_router_module, "get_llm_gateway", lambda: gateway)
    app = FastAPI()
    app.include_router(router)
    return TestClient(app), store, gateway


def test_provider_catalog_exposes_registry(client):
    c, _store, _gw = client
    resp = c.get("/api/llm/providers")

    assert resp.status_code == 200
    providers = resp.json()["providers"]
    ids = {p["id"] for p in providers}
    assert {"deepseek", "openai", "anthropic", "ollama", "custom"} <= ids
    for p in providers:
        assert "models" not in p, "目录绝不允许再带硬编码模型清单（v1 的腐烂根源）"


def test_profile_model_crud_flow(client):
    c, store, _gw = client

    # 建档案
    resp = c.post("/api/llm/profiles", json={"provider": "deepseek", "name": "DeepSeek 官方", "api_key": "sk-secret-9999"})
    assert resp.status_code == 200
    profile = resp.json()
    assert "api_key_encrypted" not in profile, "响应不得携带任何密钥字段（含密文）"
    assert "sk-secret-9999" not in resp.text, "响应绝不允许出现密钥明文"

    # 批量建模型
    resp = c.post("/api/llm/models", json={"items": [
        {"profile_id": profile["id"], "model": "deepseek-flash", "set_default": True},
        {"profile_id": profile["id"], "model": "deepseek-v4-pro"},
    ]})
    assert resp.status_code == 200
    created = resp.json()["created"]
    assert created[0]["model"] == "deepseek/deepseek-flash"

    # state 聚合视图
    state = c.get("/api/llm/state").json()
    assert state["default_model_id"] == created[0]["id"]
    assert len(state["models"]) == 2

    # activate 换默认
    resp = c.post(f"/api/llm/models/{created[1]['id']}/activate")
    assert resp.status_code == 200
    assert c.get("/api/llm/state").json()["default_model_id"] == created[1]["id"]

    # 删除默认模型 → 409；切走后可删
    assert c.delete(f"/api/llm/models/{created[1]['id']}").status_code == 409
    c.post(f"/api/llm/models/{created[0]['id']}/activate")
    assert c.delete(f"/api/llm/models/{created[1]['id']}").status_code == 200

    # 删除含默认模型的档案 → 409
    assert c.delete(f"/api/llm/profiles/{profile['id']}").status_code == 409


def test_verify_model_endpoint_persists_status(client):
    c, store, gateway = client
    profile = store.create_profile(ProfileCreate(provider="deepseek", name="D", api_key="sk-k-12345678"))
    entry = store.create_models([ModelItemCreate(profile_id=profile.id, model="deepseek-flash", set_default=True)])["created"][0]

    resp = c.post(f"/api/llm/models/{entry.id}/verify")

    assert resp.status_code == 200
    assert resp.json()["ok"] is True
    assert resp.json()["latency_ms"] == 123
    persisted = store.get_model(entry.id)
    assert persisted.status.state == "ok", "验证结果必须持久化到模型状态"

    # 网关翻脸后再次验证：仍返回 200，但 ok=False + error_code
    gateway.outcome = VerifyOutcome(ok=False, error_code="timeout", message="端点超时")
    resp = c.post(f"/api/llm/models/{entry.id}/verify")
    assert resp.status_code == 200
    body = resp.json()
    assert body["ok"] is False and body["error_code"] == "timeout"
    assert store.get_model(entry.id).status.state == "failed"


def test_verify_draft_uses_memory_only_and_does_not_persist(client):
    c, store, gateway = client
    resp = c.post("/api/llm/verify-draft", json={
        "provider": "deepseek", "api_key": "sk-draft-key-1", "model_name": "deepseek-flash",
    })

    assert resp.status_code == 200 and resp.json()["ok"] is True
    kind, kwargs = gateway.calls[-1]
    assert kind == "model"
    assert kwargs["api_key"] == "sk-draft-key-1", "草稿密钥必须真的被用来验证"
    assert store.list_profiles() == [], "草稿验证绝不落盘"


def test_remote_models_unsupported_provider_returns_409(client):
    c, store, _gw = client
    profile = store.create_profile(ProfileCreate(provider="anthropic", name="Anthropic", api_key="sk-a-12345678"))

    resp = c.post(f"/api/llm/profiles/{profile.id}/remote-models")

    assert resp.status_code == 409
    assert resp.json()["detail"]["manual_only"] is True


def test_llm_settings_binding_update(client):
    c, store, _gw = client
    profile = store.create_profile(ProfileCreate(provider="deepseek", name="D", api_key="sk-k-12345678"))
    models = store.create_models([
        ModelItemCreate(profile_id=profile.id, model="deepseek-flash", set_default=True),
        ModelItemCreate(profile_id=profile.id, model="deepseek-v4-pro"),
    ])["created"]

    resp = c.put("/api/llm/settings", json={"default_model_id": models[1].id, "fallback_model_id": models[0].id})
    assert resp.status_code == 200
    assert store.get_default_model().id == models[1].id
    assert store.get_fallback_model().id == models[0].id

    # 清除备用绑定（显式 null）
    resp = c.put("/api/llm/settings", json={"fallback_model_id": None})
    assert resp.status_code == 200
    assert store.get_fallback_model() is None

    # 不存在的模型 → 404
    assert c.put("/api/llm/settings", json={"default_model_id": "mdl-none"}).status_code == 404
