"""
LLM Configuration Manager
Manages LLM provider configurations with encrypted storage
"""
from datetime import datetime
from pathlib import Path
from typing import Optional, List
import uuid

from app.llm.models import (
    LLMConfig,
    LLMConfigCreate,
    LLMConfigUpdate,
    LLMProvider,
    get_default_model,
    get_default_base_url
)
from app.repositories.json_store import (
    ensure_dict,
    get_thread_lock,
    read_json_lenient,
    write_json_atomic,
)
from app.utils.encryption import get_encryption
from app.config import settings
from app.utils.logging import get_logger

logger = get_logger(__name__)


class LLMConfigManager:
    """Manages LLM configurations with encrypted storage."""

    def __init__(self, data_dir: Optional[str] = None):
        self.data_dir = Path(data_dir or settings.data_path)
        self.config_file = self.data_dir / "llm_configs.json"
        self._configs: dict[str, LLMConfig] = {}
        self._default_config_id: Optional[str] = None
        # 读-改-写串行化（同一资源一把可重入线程锁）
        self._lock = get_thread_lock(self.config_file)
        self._load_configs()

    def _load_configs(self) -> None:
        """Load configurations from file（顶层形状非法或无文件时按空配置处理）。"""
        if not self.config_file.exists():
            logger.info("No LLM configs file found, starting fresh")
            return

        try:
            raw_data = read_json_lenient(self.config_file, default=None)
            data = ensure_dict(raw_data, logger=logger, source=self.config_file)

            for config_data in data.get("configs") or []:
                config = LLMConfig(**config_data)
                self._configs[config.id] = config
                if config.is_default:
                    self._default_config_id = config.id

            logger.info("Loaded LLM configs", count=len(self._configs))

        except Exception as e:
            logger.error("Failed to load LLM configs", error=str(e))

    def _save_configs(self) -> None:
        """Save configurations to file（原子写，保持 indent=2 / ensure_ascii=False）。"""
        data = {
            "configs": [c.model_dump() for c in self._configs.values()],
            "default_config_id": self._default_config_id
        }

        with self._lock:
            try:
                write_json_atomic(self.config_file, data, default=str)
                logger.info("Saved LLM configs", count=len(self._configs))
            except Exception as e:
                logger.error("Failed to save LLM configs", error=str(e))
                raise

    def get_all(self) -> List[LLMConfig]:
        """Get all configurations."""
        return list(self._configs.values())

    def get_by_id(self, config_id: str) -> Optional[LLMConfig]:
        """Get configuration by ID."""
        return self._configs.get(config_id)

    def get_default(self) -> Optional[LLMConfig]:
        """Get default configuration."""
        if self._default_config_id:
            return self._configs.get(self._default_config_id)
        # Return first config if no default set
        if self._configs:
            return next(iter(self._configs.values()))
        return None

    def create(self, create_data: LLMConfigCreate) -> LLMConfig:
        """Create a new configuration."""
        with self._lock:
            encryption = get_encryption()

            # Generate ID
            config_id = f"llm-{int(datetime.now().timestamp() * 1000)}"

            # Encrypt API key
            encrypted_key = encryption.encrypt(create_data.api_key)

            # Set defaults if not provided
            model = create_data.model or get_default_model(create_data.provider)
            api_base = create_data.api_base or get_default_base_url(create_data.provider)

            config = LLMConfig(
                id=config_id,
                provider=create_data.provider,
                name=create_data.name,
                api_key_encrypted=encrypted_key,
                api_base=api_base,
                model=model,
                temperature=create_data.temperature,
                max_tokens=create_data.max_tokens,
                extra_params=create_data.extra_params,
                is_default=len(self._configs) == 0,  # First config is default
                created_at=datetime.now()
            )

            self._configs[config_id] = config

            if config.is_default:
                self._default_config_id = config_id

            self._save_configs()

            logger.info("Created LLM config", config_id=config_id, provider=config.provider)
            return config

    def update(self, config_id: str, update_data: LLMConfigUpdate) -> Optional[LLMConfig]:
        """Update an existing configuration."""
        with self._lock:
            config = self._configs.get(config_id)
            if not config:
                return None

            encryption = get_encryption()

            # Update fields
            if update_data.name is not None:
                config.name = update_data.name

            if update_data.api_key is not None:
                config.api_key_encrypted = encryption.encrypt(update_data.api_key)

            if update_data.api_base is not None:
                config.api_base = update_data.api_base

            if update_data.model is not None:
                config.model = update_data.model

            if update_data.temperature is not None:
                config.temperature = update_data.temperature

            if update_data.max_tokens is not None:
                config.max_tokens = update_data.max_tokens

            if update_data.extra_params is not None:
                config.extra_params = update_data.extra_params

            config.updated_at = datetime.now()

            self._save_configs()

            logger.info("Updated LLM config", config_id=config_id)
            return config

    def delete(self, config_id: str) -> bool:
        """Delete a configuration."""
        with self._lock:
            if config_id not in self._configs:
                return False

            was_default = self._configs[config_id].is_default
            del self._configs[config_id]

            # Set new default if needed
            if was_default and self._configs:
                new_default = next(iter(self._configs.values()))
                new_default.is_default = True
                self._default_config_id = new_default.id

            self._save_configs()

            logger.info("Deleted LLM config", config_id=config_id)
            return True

    def set_default(self, config_id: str) -> bool:
        """Set default configuration."""
        with self._lock:
            if config_id not in self._configs:
                return False

            # Remove default from others
            for config in self._configs.values():
                config.is_default = False

            # Set new default
            self._configs[config_id].is_default = True
            self._default_config_id = config_id

            self._save_configs()

            logger.info("Set default LLM config", config_id=config_id)
            return True

    def decrypt_key(self, config_id: str) -> Optional[str]:
        """Decrypt API key for a configuration."""
        config = self._configs.get(config_id)
        if not config:
            return None

        if not config.api_key_encrypted:
            return ""

        encryption = get_encryption()
        try:
            return encryption.decrypt(config.api_key_encrypted)
        except ValueError:
            logger.error("Failed to decrypt stored API key", config_id=config_id)
            return None


# Global instance
_llm_config_manager: Optional[LLMConfigManager] = None


def get_llm_config_manager() -> LLMConfigManager:
    """Get or create global LLM config manager instance."""
    global _llm_config_manager
    if _llm_config_manager is None:
        _llm_config_manager = LLMConfigManager()
    return _llm_config_manager
