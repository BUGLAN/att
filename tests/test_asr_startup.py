import unittest
from unittest.mock import AsyncMock, patch

from app.main import maybe_preload_asr_provider


class _FakeProvider:
    provider_type = "whisper"
    model_name = "large-v3"


class MaybePreloadAsrProviderTests(unittest.IsolatedAsyncioTestCase):
    async def test_skips_warmup_when_preload_disabled(self):
        provider = _FakeProvider()
        provider.warmup = AsyncMock()

        with patch("app.main.settings.asr_preload_on_startup", False):
            await maybe_preload_asr_provider(provider)

        provider.warmup.assert_not_awaited()

    async def test_continues_when_warmup_fails(self):
        provider = _FakeProvider()
        provider.warmup = AsyncMock(side_effect=RuntimeError("broken model cache"))

        with patch("app.main.settings.asr_preload_on_startup", True):
            await maybe_preload_asr_provider(provider)

        provider.warmup.assert_awaited_once()
