"""
DeepSeek Provider
OpenAI-compatible API implementation
"""
from typing import List
from openai import AsyncOpenAI

from app.llm.base import BaseLLMProvider
from app.llm.models import LLMConfig, LLMProvider, get_provider_models
from app.llm.security import redact_secrets


class DeepSeekProvider(BaseLLMProvider):
    """DeepSeek API provider (OpenAI-compatible)."""

    def __init__(self, config: LLMConfig, decrypted_key: str):
        super().__init__(config, decrypted_key)
        self.client = AsyncOpenAI(
            api_key=self.api_key,
            base_url=config.api_base or "https://api.deepseek.com/v1"
        )

    async def optimize(self, text: str, prompt_template: str) -> str:
        """Optimize text using DeepSeek API."""
        prompt = self.render_prompt(prompt_template, text)

        response = await self.client.chat.completions.create(
            model=self.config.model,
            messages=[{"role": "user", "content": prompt}],
            temperature=self.config.temperature,
            max_tokens=self.config.max_tokens
        )

        return response.choices[0].message.content

    async def test_connection(self) -> dict:
        """Test DeepSeek API connection."""
        try:
            await self.client.chat.completions.create(
                model=self.config.model,
                messages=[{"role": "user", "content": "Hi"}],
                max_tokens=5
            )
            return {"success": True, "error": None}
        except Exception as exc:
            # 上游鉴权失败时会在错误体里回显密钥，返回给前端前必须脱敏（AGENTS.md §3.4）
            return {"success": False, "error": redact_secrets(exc, [self.api_key])}

    def get_available_models(self) -> List[str]:
        """Get available DeepSeek models."""
        return get_provider_models(LLMProvider.DEEPSEEK)
