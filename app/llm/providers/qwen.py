"""
Qwen Provider (Alibaba Cloud)
OpenAI-compatible API implementation
"""
from typing import List
from openai import AsyncOpenAI

from app.llm.base import BaseLLMProvider
from app.llm.models import LLMConfig, LLMProvider, get_provider_models


class QwenProvider(BaseLLMProvider):
    """Qwen API provider (Alibaba Cloud, OpenAI-compatible)."""

    def __init__(self, config: LLMConfig, decrypted_key: str):
        super().__init__(config, decrypted_key)
        self.client = AsyncOpenAI(
            api_key=self.api_key,
            base_url=config.api_base or "https://dashscope.aliyuncs.com/compatible-mode/v1"
        )

    async def optimize(self, text: str, prompt_template: str) -> str:
        """Optimize text using Qwen API."""
        prompt = self.render_prompt(prompt_template, text)

        response = await self.client.chat.completions.create(
            model=self.config.model,
            messages=[{"role": "user", "content": prompt}],
            temperature=self.config.temperature,
            max_tokens=self.config.max_tokens
        )

        return response.choices[0].message.content

    async def test_connection(self) -> bool:
        """Test Qwen API connection."""
        try:
            # Try a minimal completion
            await self.client.chat.completions.create(
                model=self.config.model,
                messages=[{"role": "user", "content": "Hi"}],
                max_tokens=10
            )
            return True
        except Exception:
            return False

    def get_available_models(self) -> List[str]:
        """Get available Qwen models."""
        return get_provider_models(LLMProvider.QWEN)
