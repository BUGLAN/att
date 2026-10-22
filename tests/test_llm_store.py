"""LLMStore 回归测试（档案 + 模型两级配置）。

锁死的点：
1. 逐条容错加载：单条损坏只跳过该条（修复已知问题 #9「全有或全无」）；
2. 密钥只在档案级加密存储；解密失败抛 LLMError 而不是静默返回空；
3. 默认/备用绑定的自动修复与删除保护（默认模型不可删/不可级联删）；
4. 批量创建模型的去重与非法项跳过语义；落盘文件必须是完整合法 JSON（原子写）。
"""
import json

import pytest

from app.llm.errors import CODE_AUTH, LLMError
from app.llm.models import ModelItemCreate, ModelParams, ProfileCreate, ProfileUpdate, VerifyStatus
from app.llm.store import LLMStore, reset_llm_store


class _FakeEncryption:
    """可逆的假加密：enc: 前缀，非前缀密文视为损坏。"""

    def encrypt(self, value: str) -> str:
        return "enc:" + value

    def decrypt(self, value: str) -> str:
        if not value.startswith("enc:"):
            raise ValueError("invalid token")
        return value[len("enc:"):]


@pytest.fixture(autouse=True)
def fake_encryption(monkeypatch):
    import app.llm.migration as migration_module
    import app.llm.store as store_module

    monkeypatch.setattr(store_module, "get_encryption", _FakeEncryption)
    monkeypatch.setattr(migration_module, "get_encryption", _FakeEncryption)
    return _FakeEncryption


@pytest.fixture
def store(tmp_path):
    reset_llm_store()
    return LLMStore(tmp_path)


def _profile(store, provider="deepseek", name="DeepSeek 官方", api_key="sk-1234567890", api_base=None):
    return store.create_profile(ProfileCreate(provider=provider, name=name, api_key=api_key, api_base=api_base))


# ==================== 档案 ====================


def test_create_profile_encrypts_key_and_records_hint(store):
    profile = _profile(store)

    assert profile.api_key_encrypted == "enc:sk-1234567890", "密钥必须加密落盘，禁止明文"
    assert profile.api_key_hint.endswith("7890")
    assert "sk-1234567890" not in profile.api_key_hint


def test_create_custom_profile_requires_base(store):
    with pytest.raises(ValueError, match="API Base"):
        _profile(store, provider="custom", api_base=None)


def test_update_profile_with_new_key_resets_status(store):
    profile = _profile(store)
    store.set_profile_status(profile.id, VerifyStatus(state="ok", latency_ms=100))

    updated = store.update_profile(profile.id, ProfileUpdate(api_key="sk-new-key-9999"))

    assert updated.api_key_encrypted == "enc:sk-new-key-9999"
    assert updated.status.state == "unknown", "凭据变更后旧验证结论必须失效"


def test_decrypt_failure_raises_auth_error(store):
    profile = _profile(store)
    profile.api_key_encrypted = "not-a-valid-token"

    with pytest.raises(LLMError) as excinfo:
        store.decrypt_key(profile)

    assert excinfo.value.code == CODE_AUTH
    assert "重新填写" in excinfo.value.user_message


# ==================== 模型 ====================


def test_create_models_batch_with_dedup_and_skip(store):
    profile = _profile(store)
    result = store.create_models([
        ModelItemCreate(profile_id=profile.id, model="deepseek-flash", set_default=True),
        ModelItemCreate(profile_id=profile.id, model="deepseek-v4-pro"),
        ModelItemCreate(profile_id=profile.id, model="deepseek-flash"),  # 重复
        ModelItemCreate(profile_id="prof-none", model="whatever"),        # 档案不存在
    ])

    assert [m.model for m in result["created"]] == [
        "deepseek/deepseek-flash",
        "deepseek/deepseek-v4-pro",
    ], "裸模型名必须按档案前缀组装成完整模型串"
    assert len(result["skipped"]) == 2
    assert store.get_default_model().model == "deepseek/deepseek-flash"


def test_delete_default_model_is_conflict_until_switched(store):
    profile = _profile(store)
    created = store.create_models([
        ModelItemCreate(profile_id=profile.id, model="a"),
        ModelItemCreate(profile_id=profile.id, model="b"),
    ])
    first, second = created["created"]
    store.set_default_model(first.id)

    assert store.delete_model(first.id) == "conflict_default"
    store.set_default_model(second.id)
    assert store.delete_model(first.id) == "deleted"


def test_delete_profile_cascades_but_protects_default(store):
    profile = _profile(store)
    created = store.create_models([ModelItemCreate(profile_id=profile.id, model="a", set_default=True)])

    assert store.delete_profile(profile.id) == "conflict_default"

    other = _profile(store, provider="openai", name="OpenAI", api_key="sk-o-12345678")
    other_models = store.create_models([ModelItemCreate(profile_id=other.id, model="gpt-x", set_default=True)])
    store.set_default_model(other_models["created"][0].id)

    assert store.delete_profile(profile.id) == "deleted"
    assert store.get_model(created["created"][0].id) is None, "档案删除必须级联其下模型"


# ==================== 容错加载（已知问题 #9） ====================


def test_tolerant_load_skips_broken_entries(tmp_path):
    (tmp_path / "llm_v2.json").write_text(json.dumps({
        "version": 2,
        "profiles": [
            {"id": "prof-good", "provider": "deepseek", "name": "OK", "api_key_encrypted": "enc:sk-1234567890"},
            {"id": "prof-bad", "provider": "deepseek", "temperature": "非法字段类型"},
        ],
        "models": [
            {"id": "mdl-good", "profile_id": "prof-good", "model": "deepseek/deepseek-flash"},
            {"id": "mdl-bad", "profile_id": "prof-good", "params": {"timeout_s": "abc"}},
        ],
        "default_model_id": "mdl-good",
    }), encoding="utf-8")

    store = LLMStore(tmp_path)

    assert [p.id for p in store.list_profiles()] == ["prof-good"]
    assert [m.id for m in store.list_models()] == ["mdl-good"]
    assert len(store.skipped_entries) == 2, "损坏条目必须被点名并跳过，而不是全量消失"
    assert store.get_default_model().id == "mdl-good"


def test_tolerant_load_handles_invalid_top_shape(tmp_path):
    """顶层形状非法（比如被手写坏成 null）时按空配置处理，不能启动失败。"""
    (tmp_path / "llm_v2.json").write_text("null", encoding="utf-8")

    store = LLMStore(tmp_path)

    assert store.list_profiles() == []
    assert store.list_models() == []


def test_default_binding_repair_when_missing(tmp_path):
    (tmp_path / "llm_v2.json").write_text(json.dumps({
        "version": 2,
        "profiles": [{"id": "p1", "provider": "deepseek", "name": "D", "api_key_encrypted": "enc:k"}],
        "models": [{"id": "m1", "profile_id": "p1", "model": "deepseek/x"}],
        "default_model_id": "mdl-gone",
    }), encoding="utf-8")

    store = LLMStore(tmp_path)

    assert store.get_default_model().id == "m1", "默认绑定指向缺失模型时必须自动回落"
    # 回写修正：重新加载后仍一致
    store2 = LLMStore(tmp_path)
    assert store2.get_default_model().id == "m1"


# ==================== 运行时解析 / 落盘 ====================


def test_resolve_runtime_without_models_raises_model_not_found(store):
    with pytest.raises(LLMError) as excinfo:
        store.resolve_runtime(None)
    assert excinfo.value.code == "model_not_found"


def test_custom_model_strings_are_healed_on_load(tmp_path):
    """2026-10-22 实测踩坑回归：custom 档案下含 / 的模型 ID 曾被原样存库，
    LiteLLM 把首段误当服务商。加载期必须统一补齐 openai/ 前缀。"""
    (tmp_path / "llm_v2.json").write_text(json.dumps({
        "version": 2,
        "profiles": [
            {"id": "p-custom", "provider": "custom", "name": "本地网关", "api_base": "http://127.0.0.1:9999/v1"},
            {"id": "p-deepseek", "provider": "deepseek", "name": "DeepSeek", "api_key_encrypted": "enc:k"},
        ],
        "models": [
            {"id": "m-bad", "profile_id": "p-custom", "model": "group/deepseek-v4.1-flash"},
            {"id": "m-ok", "profile_id": "p-custom", "model": "openai/qwen3:14b"},
            {"id": "m-other", "profile_id": "p-deepseek", "model": "some-group/deepseek-flash"},
        ],
        "default_model_id": "m-bad",
    }), encoding="utf-8")

    store = LLMStore(tmp_path)
    by_id = {m.id: m for m in store.list_models()}

    assert by_id["m-bad"].model == "openai/group/deepseek-v4.1-flash", "坏串必须自愈为 openai/ + 完整 ID"
    assert by_id["m-ok"].model == "openai/qwen3:14b", "已带 openai/ 的串保持幂等"
    assert by_id["m-other"].model == "some-group/deepseek-flash", "非 custom 档案的完整串透传不受影响"
    # 修复要回写：重新加载后一致
    store2 = LLMStore(tmp_path)
    assert {m.id: m.model for m in store2.list_models()} == {m.id: m.model for m in store.list_models()}


def test_persisted_file_is_valid_json_with_encrypted_key_only(store):
    profile = _profile(store)
    store.create_models([ModelItemCreate(profile_id=profile.id, model="deepseek-flash")])

    data = json.loads(store.file.read_text(encoding="utf-8"))
    assert data["version"] == 2
    assert data["profiles"][0]["api_key_encrypted"].startswith("enc:")
    assert "api_key" not in data["profiles"][0], "明文字段绝不允许出现在落盘结构里"
