"""v1 → v2 迁移器回归测试（docs/LLM配置模块重设计.md §9.1）。

锁死的点：
1. 同 key 同服务商的多条 v1 配置聚合为一个档案，模型各自成条目；
2. v1 provider 枚举正确映射到 LiteLLM 前缀（claude→anthropic、qwen→dashscope、
   doubao→custom+方舟端点）；
3. v1 默认配置映射到对应模型条目 id；
4. 单条坏数据只跳过该条；解密失败的条目保留密文并标记 decrypt_failed；
5. 迁移后 v1 文件改名为 .v1.bak（回滚锚点），再次初始化不会重复迁移。
"""
import json

import pytest

from app.llm.migration import migrate_v1_to_v2
from app.llm.models import STATUS_DECRYPT_FAILED
from app.llm.store import LLMStore, reset_llm_store


class _FakeEncryption:
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


def _write_v1(tmp_path, configs, default_config_id=None):
    payload = {"configs": configs}
    if default_config_id:
        payload["default_config_id"] = default_config_id
    (tmp_path / "llm_configs.json").write_text(json.dumps(payload, ensure_ascii=False), encoding="utf-8")


def test_migrate_aggregates_same_key_and_maps_prefixes(tmp_path):
    key = "enc:sk-same-key-1234"
    _write_v1(tmp_path, [
        {
            "id": "llm-1", "provider": "deepseek", "name": "DeepSeek 主力",
            "api_key_encrypted": key, "api_base": "https://api.deepseek.com/v1",
            "model": "deepseek-flash", "temperature": 0.3, "max_tokens": 4096,
        },
        {
            "id": "llm-2", "provider": "deepseek", "name": "DeepSeek 备用",
            "api_key_encrypted": key, "api_base": "https://api.deepseek.com/v1",
            "model": "deepseek-v4-pro",
        },
        {
            "id": "llm-3", "provider": "claude", "name": "Claude",
            "api_key_encrypted": "enc:sk-ant-9999", "model": "claude-sonnet-4-5",
        },
        {
            "id": "llm-4", "provider": "qwen", "name": "千问",
            "api_key_encrypted": "enc:sk-qwen-1", "model": "qwen-max",
        },
        {
            "id": "llm-5", "provider": "doubao", "name": "豆包",
            "api_key_encrypted": "enc:ark-1", "model": "doubao-pro-32k",
        },
    ], default_config_id="llm-2")

    report = migrate_v1_to_v2(tmp_path)

    # llm-1/llm-2 同 key 同端点 → 1 个档案 2 个模型
    deepseek_profiles = [p for p in report.profiles if p.provider == "deepseek"]
    assert len(deepseek_profiles) == 1, "同 key 同端点必须聚合为一个档案，密钥只存一份"
    assert sorted(m.model for m in report.models if m.profile_id == deepseek_profiles[0].id) == [
        "deepseek/deepseek-flash",
        "deepseek/deepseek-v4-pro",
    ]
    # 前缀映射
    by_model = {m.model: m for m in report.models}
    assert "anthropic/claude-sonnet-4-5" in by_model
    assert "dashscope/qwen-max" in by_model
    doubao = by_model.get("openai/doubao-pro-32k")
    assert doubao is not None, "v1 豆包必须走 custom（openai/ + 方舟端点）这条保证可用的路径"
    doubao_profile = next(p for p in report.profiles if p.id == doubao.profile_id)
    assert doubao_profile.api_base == "https://ark.cn-beijing.volces.com/api/v3"
    # 默认映射：llm-2 → deepseek-v4-pro
    assert report.default_model_id == by_model["deepseek/deepseek-v4-pro"].id


def test_migrate_skips_broken_entry_without_killing_rest(tmp_path):
    _write_v1(tmp_path, [
        {"id": "llm-bad", "provider": "deepseek", "name": "坏数据", "api_key_encrypted": "enc:x", "model": ""},
        {"id": "llm-good", "provider": "openai", "name": "好数据", "api_key_encrypted": "enc:sk-openai-1", "model": "gpt-5"},
    ])

    report = migrate_v1_to_v2(tmp_path)

    assert [m.model for m in report.models] == ["openai/gpt-5"]
    assert report.skipped and "llm-bad" in report.skipped[0]


def test_migrate_preserves_undecryptable_entry(tmp_path):
    _write_v1(tmp_path, [
        {"id": "llm-lock", "provider": "deepseek", "name": "换机器遗留",
         "api_key_encrypted": "原始损坏密文", "model": "deepseek-flash"},
    ])

    report = migrate_v1_to_v2(tmp_path)

    assert len(report.profiles) == 1
    profile = report.profiles[0]
    assert profile.status.state == STATUS_DECRYPT_FAILED
    assert profile.api_key_encrypted == "原始损坏密文", "解密失败必须保留密文，等用户重填 key 修复"
    assert len(report.models) == 1, "模型条目仍要迁移出来，修好 key 即可用"


def test_store_init_triggers_migration_and_renames_legacy(tmp_path):
    _write_v1(tmp_path, [
        {"id": "llm-1", "provider": "deepseek", "name": "D", "api_key_encrypted": "enc:k",
         "model": "deepseek-flash", "is_default": True},
    ])

    reset_llm_store()
    store = LLMStore(tmp_path)

    assert len(store.list_models()) == 1
    assert store.get_default_model() is not None
    assert (tmp_path / "llm_configs.json.v1.bak").exists(), "v1 文件必须改名保留回滚锚点"
    assert not (tmp_path / "llm_configs.json").exists()

    # v2 文件已生成；再次初始化走正常加载，不重复迁移
    store2 = LLMStore(tmp_path)
    assert len(store2.list_models()) == 1


def test_migrate_returns_none_without_v1_file(tmp_path):
    assert migrate_v1_to_v2(tmp_path) is None
