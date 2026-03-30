import asyncio
import json

from app.ai_actions.models import AIActionType
from app.ai_actions.service import AIActionService
from app.wordbook.manager import WordbookManager


class StubProvider:
    def __init__(self, payload):
        self.payload = payload
        self.calls = []

    async def optimize(self, text: str, prompt_template: str) -> str:
        self.calls.append({
            'text': text,
            'prompt_template': prompt_template,
        })
        return json.dumps(self.payload, ensure_ascii=False)


class StubGateway:
    def __init__(self, provider):
        self.provider = provider

    def get_default_provider(self):
        return self.provider


def test_ai_action_service_translate_uses_gateway_and_persists_wordbook(temp_dir):
    async def run_test():
        wordbook_manager = WordbookManager(temp_dir)
        await wordbook_manager.initialize()
        provider = StubProvider({
            'language': 'en',
            'entry_type': 'word',
            'translation': '准确的；精确的',
            'explanations': ['adj. correct and exact in every detail'],
            'examples': ['The report is accurate and concise.'],
        })
        service = AIActionService(
            llm_gateway=StubGateway(provider),
            wordbook_manager=wordbook_manager,
        )

        result = await service.run_action(
            action_type=AIActionType.TRANSLATE,
            source_text='accurate',
            source_app='chrome.exe',
            source_context='The report is accurate and concise.'
        )

        assert result.translation == '准确的；精确的'
        assert result.detected_language == 'en'
        assert result.entry_type == 'word'
        assert result.saved_to_wordbook is True
        assert result.wordbook_entry_id
        assert provider.calls[0]['text'] == 'accurate'

        entries = await wordbook_manager.list_entries(query='accurate')
        assert len(entries) == 1
        assert entries[0].translation == '准确的；精确的'

    asyncio.run(run_test())


def test_ai_action_service_translate_can_skip_wordbook_persistence(temp_dir):
    async def run_test():
        wordbook_manager = WordbookManager(temp_dir)
        await wordbook_manager.initialize()
        provider = StubProvider({
            'language': 'en',
            'entry_type': 'word',
            'translation': '准确的；精确的',
            'explanations': ['adj. correct and exact in every detail'],
            'examples': [],
        })
        service = AIActionService(
            llm_gateway=StubGateway(provider),
            wordbook_manager=wordbook_manager,
        )

        result = await service.run_action(
            action_type=AIActionType.TRANSLATE,
            source_text='accurate',
            auto_add_to_wordbook=False,
        )

        assert result.saved_to_wordbook is False
        assert result.wordbook_entry_id is None

        entries = await wordbook_manager.list_entries(query='accurate')
        assert entries == []

    asyncio.run(run_test())
