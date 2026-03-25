"""
LLM Provider Base Class
Abstract base class for LLM provider implementations
"""
from abc import ABC, abstractmethod
from typing import List, Optional

from app.llm.models import LLMConfig


class BaseLLMProvider(ABC):
    """
    Abstract base class for LLM providers.

    All provider implementations must inherit from this class
    and implement the abstract methods.
    """

    def __init__(self, config: LLMConfig, decrypted_key: str):
        """
        Initialize provider with configuration.

        Args:
            config: LLM configuration
            decrypted_key: Decrypted API key
        """
        self.config = config
        self.api_key = decrypted_key

    @abstractmethod
    async def optimize(self, text: str, prompt_template: str) -> str:
        """
        Optimize text using the LLM.

        Args:
            text: Text to optimize
            prompt_template: Prompt template with {text} placeholder

        Returns:
            Optimized text
        """
        pass

    @abstractmethod
    async def test_connection(self) -> bool:
        """
        Test the API connection.

        Returns:
            True if connection successful, False otherwise
        """
        pass

    @abstractmethod
    def get_available_models(self) -> List[str]:
        """
        Get list of available models for this provider.

        Returns:
            List of model names
        """
        pass

    def render_prompt(self, prompt_template: str, text: str) -> str:
        """
        Render prompt template with text.

        Args:
            prompt_template: Template with {text} placeholder
            text: Text to insert

        Returns:
            Rendered prompt
        """
        try:
            return prompt_template.format(text=text)
        except KeyError:
            # Template doesn't have {text} placeholder
            return prompt_template
