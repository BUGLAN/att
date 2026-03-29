"""
LLM Gateway
Factory for creating LLM provider instances
"""
from typing import Optional, Dict

from app.llm.base import BaseLLMProvider
from app.llm.models import LLMConfig, LLMProvider
from app.llm.config_manager import get_llm_config_manager
from app.llm.providers import (
    DeepSeekProvider,
    ClaudeProvider,
    OpenAIProvider,
    DoubaoProvider,
    QwenProvider
)
from app.utils.logging import get_logger

logger = get_logger(__name__)


# Provider class mapping
PROVIDER_CLASSES = {
    LLMProvider.DEEPSEEK: DeepSeekProvider,
    LLMProvider.CLAUDE: ClaudeProvider,
    LLMProvider.OPENAI: OpenAIProvider,
    LLMProvider.DOUBAO: DoubaoProvider,
    LLMProvider.QWEN: QwenProvider,
    LLMProvider.CUSTOM: OpenAIProvider,  # Custom uses OpenAI-compatible
}


class LLMGateway:
    """
    Gateway for creating and managing LLM provider instances.

    Provides a unified interface for accessing different LLM providers.
    """

    def __init__(self):
        self._provider_cache: Dict[str, BaseLLMProvider] = {}

    def get_provider(self, config_id: str) -> Optional[BaseLLMProvider]:
        """
        Get a provider instance for the given configuration ID.

        Args:
            config_id: LLM configuration ID

        Returns:
            Provider instance or None if not found
        """
        # Check cache
        if config_id in self._provider_cache:
            return self._provider_cache[config_id]

        # Get config
        cm = get_llm_config_manager()
        config = cm.get_by_id(config_id)
        if not config:
            logger.warning("LLM config not found", config_id=config_id)
            return None

        # Create provider
        provider = self._create_provider(config)
        if provider:
            self._provider_cache[config_id] = provider

        return provider

    def get_default_provider(self) -> Optional[BaseLLMProvider]:
        """
        Get the default provider instance.

        Returns:
            Default provider instance or None if no configs
        """
        cm = get_llm_config_manager()
        default_config = cm.get_default()

        if not default_config:
            return None

        return self.get_provider(default_config.id)

    def _create_provider(self, config: LLMConfig) -> Optional[BaseLLMProvider]:
        """
        Create a provider instance for the given configuration.

        Args:
            config: LLM configuration

        Returns:
            Provider instance or None if creation fails
        """
        cm = get_llm_config_manager()

        # Decrypt API key
        if not config.api_key_encrypted:
            logger.warning("LLM config is missing API key", config_id=config.id)
            return None

        decrypted_key = cm.decrypt_key(config.id)
        if decrypted_key is None:
            logger.error("Failed to decrypt API key", config_id=config.id)
            return None
        if not decrypted_key:
            logger.warning("LLM config decrypted to empty API key", config_id=config.id)
            return None

        # Get provider class
        provider_class = PROVIDER_CLASSES.get(config.provider)
        if not provider_class:
            logger.error("Unknown provider", provider=config.provider)
            return None

        try:
            provider = provider_class(config, decrypted_key)
            logger.debug(
                "Created provider instance",
                config_id=config.id,
                provider=config.provider.value
            )
            return provider
        except Exception as e:
            logger.error(
                "Failed to create provider",
                config_id=config.id,
                provider=config.provider.value,
                error=str(e)
            )
            return None

    def clear_cache(self) -> None:
        """Clear the provider cache."""
        self._provider_cache.clear()
        logger.debug("Cleared provider cache")

    def invalidate(self, config_id: str) -> None:
        """Invalidate a cached provider."""
        if config_id in self._provider_cache:
            del self._provider_cache[config_id]
            logger.debug("Invalidated cached provider", config_id=config_id)


# Global instance
_gateway: Optional[LLMGateway] = None


def get_gateway() -> LLMGateway:
    """Get or create global LLM gateway instance."""
    global _gateway
    if _gateway is None:
        _gateway = LLMGateway()
    return _gateway
