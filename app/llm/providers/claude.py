"""
Claude Provider
Anthropic API implementation
"""
from typing import List
from anthropic import AsyncAnthropic

from app.llm.base import BaseLLMProvider
from app.llm.models import LLMConfig, LLMProvider, get_provider_models


class ClaudeProvider(BaseLLMProvider):
    """Claude API provider (Anthropic)."""

    def __init__(self, config: LLMConfig, decrypted_key: str):
        super().__init__(config, decrypted_key)
        self.client = AsyncAnthropic(
            api_key=self.api_key,
            base_url=config.api_base
        )

    async def optimize(self, text: str, prompt_template: str) -> str:
        """Optimize text using Claude API."""
        prompt = self.render_prompt(prompt_template, text)

        response = await self.client.messages.create(
            model=self.config.model,
            max_tokens=self.config.max_tokens,
            temperature=self.config.temperature,
            messages=[{"role": "user", "content": prompt}]
        )

        return response.content[0].text

    async def test_connection(self) -> dict:
        """Test Claude API connection."""
        try:
            await self.client.messages.create(
                model=self.config.model,
                max_tokens=5,
                messages=[{"role": "user", "content": "Hi"}]
            )
            return {"success": True, "error": None}
        except Exception as exc:
            return {"success": False, "error": str(exc)}

    def get_available_models(self) -> List[str]:
        """Get available Claude models."""
        return get_provider_models(LLMProvider.CLAUDE)
