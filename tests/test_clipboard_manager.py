import asyncio
import shutil
import uuid
from pathlib import Path

from app.clipboard.manager import ClipboardManager
from app.clipboard.models import ClipboardItemType


def test_clipboard_manager_create_search_and_update():
    async def run_test():
        test_root = Path(__file__).resolve().parent.parent / 'data' / 'test-clipboard' / str(uuid.uuid4())
        test_root.mkdir(parents=True, exist_ok=True)

        try:
            manager = ClipboardManager(test_root)
            await manager.initialize()

            item = await manager.create_item(
                content='https://example.com/docs',
                source='test'
            )

            assert item.item_type == ClipboardItemType.LINK

            items = await manager.list_items(query='example')
            assert len(items) == 1
            assert items[0].id == item.id

            updated = await manager.update_item(
                item.id,
                is_favorite=True,
                is_pinned=True
            )

            assert updated.is_favorite is True
            assert updated.is_pinned is True

            stats = await manager.get_stats()
            assert stats['total'] == 1
            assert stats['favorites'] == 1
            assert stats['pinned'] == 1
        finally:
            shutil.rmtree(test_root, ignore_errors=True)

    asyncio.run(run_test())


def test_clipboard_manager_resolves_image_items():
    async def run_test():
        test_root = Path(__file__).resolve().parent.parent / 'data' / 'test-clipboard' / str(uuid.uuid4())
        test_root.mkdir(parents=True, exist_ok=True)

        try:
            manager = ClipboardManager(test_root)
            await manager.initialize()

            image_path = test_root / 'clipboard-image.png'
            image_path.write_bytes(b'fake-image')

            item = await manager.create_item(
                content='image:test-image',
                source='test',
                item_type=ClipboardItemType.IMAGE,
                title='测试图片',
                summary='64 x 32',
                metadata={
                    'image_path': str(image_path),
                    'file_name': '测试图片.png'
                }
            )

            resolved_path = await manager.get_item_image_path(item.id)
            assert resolved_path == image_path

            image_items = await manager.list_items(item_type=ClipboardItemType.IMAGE)
            assert len(image_items) == 1
            assert image_items[0].id == item.id
        finally:
            shutil.rmtree(test_root, ignore_errors=True)

    asyncio.run(run_test())
