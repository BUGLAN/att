from datetime import datetime
from types import SimpleNamespace

from app.llm.config_manager import LLMConfigManager
from app.llm.gateway import LLMGateway
from app.llm.models import LLMConfig, LLMProvider


def make_config(api_key_encrypted: str) -> LLMConfig:
    return LLMConfig(
        id="llm-test",
        provider=LLMProvider.DEEPSEEK,
        name="deepseek",
        api_key_encrypted=api_key_encrypted,
        api_base="https://api.deepseek.com/v1",
        model="deepseek-chat",
        temperature=0.3,
        max_tokens=4096,
        extra_params={},
        is_default=True,
        created_at=datetime.now(),
    )


def test_gateway_skips_config_with_missing_api_key(monkeypatch):
    config = make_config("")
    gateway = LLMGateway()

    monkeypatch.setattr(
        "app.llm.gateway.get_llm_config_manager",
        lambda: SimpleNamespace(
            get_by_id=lambda config_id: config if config_id == config.id else None,
            decrypt_key=lambda config_id: ""
        )
    )

    assert gateway.get_provider(config.id) is None


def test_decrypt_key_returns_none_for_invalid_ciphertext(monkeypatch, tmp_path):
    manager = LLMConfigManager(data_dir=str(tmp_path))
    config = make_config("not-a-valid-fernet-token")
    manager._configs[config.id] = config

    class BrokenEncryption:
        def decrypt(self, encrypted: str) -> str:
            raise ValueError("invalid token")

    monkeypatch.setattr(
        "app.llm.config_manager.get_encryption",
        lambda: BrokenEncryption()
    )

    assert manager.decrypt_key(config.id) is None
