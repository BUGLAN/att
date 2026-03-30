import asyncio

from app.wordbook.manager import WordbookManager
from app.wordbook.models import WordbookEntryType


def test_wordbook_manager_capture_list_and_stats(temp_dir):
    async def run_test():
        manager = WordbookManager(temp_dir)
        await manager.initialize()

        first = await manager.capture_entry(
            source_text='accurate',
            normalized_text='accurate',
            language='en',
            entry_type=WordbookEntryType.WORD,
            translation='准确的；精确的',
            explanations=['adj. correct and exact in every detail'],
            source_app='chrome.exe',
            source_context='The report is accurate and concise.'
        )

        second = await manager.capture_entry(
            source_text='accurate',
            normalized_text='accurate',
            language='en',
            entry_type=WordbookEntryType.WORD,
            translation='准确的；精确的',
            explanations=['adj. correct and exact in every detail'],
            source_app='chrome.exe',
            source_context='The report is accurate and concise.'
        )

        assert second.id == first.id
        assert second.query_count == 2

        items = await manager.list_entries(query='accurate')
        assert len(items) == 1
        assert items[0].translation == '准确的；精确的'

        updated = await manager.update_entry(second.id, is_favorite=True)
        assert updated is not None
        assert updated.is_favorite is True

        stats = await manager.get_stats()
        assert stats['total'] == 1
        assert stats['favorites'] == 1
        assert stats['today'] == 1

    asyncio.run(run_test())
