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

            image_dir = test_root / 'clipboard' / 'images'
            image_dir.mkdir(parents=True, exist_ok=True)
            image_path = image_dir / 'clipboard-image.png'
            image_path.write_bytes(b'fake-image')

            relative_item = await manager.create_item(
                content='image:test-image',
                source='test',
                item_type=ClipboardItemType.IMAGE,
                title='测试图片',
                summary='64 x 32',
                metadata={
                    'image_path': 'images/clipboard-image.png',
                    'file_name': '测试图片.png'
                }
            )

            relative_resolved_path = await manager.get_item_image_path(relative_item.id)
            assert relative_resolved_path == image_path

            legacy_item = await manager.create_item(
                content='image:test-image-legacy',
                source='test',
                item_type=ClipboardItemType.IMAGE,
                title='旧图片记录',
                summary='64 x 32',
                metadata={
                    'image_path': 'E:\\repo\\att\\data\\clipboard\\images\\clipboard-image.png',
                    'stored_file_name': 'clipboard-image.png',
                    'file_name': '旧图片记录.png'
                }
            )

            legacy_resolved_path = await manager.get_item_image_path(legacy_item.id)
            assert legacy_resolved_path == image_path

            image_items = await manager.list_items(item_type=ClipboardItemType.IMAGE)
            assert len(image_items) == 2
            assert {image_item.id for image_item in image_items} == {
                relative_item.id,
                legacy_item.id
            }
        finally:
            shutil.rmtree(test_root, ignore_errors=True)

    asyncio.run(run_test())


def test_clipboard_manager_supports_pinned_filter():
    async def run_test():
        test_root = Path(__file__).resolve().parent.parent / 'data' / 'test-clipboard' / str(uuid.uuid4())
        test_root.mkdir(parents=True, exist_ok=True)

        try:
            manager = ClipboardManager(test_root)
            await manager.initialize()

            pinned_item = await manager.create_item(
                content='pinned text',
                source='test'
            )
            normal_item = await manager.create_item(
                content='normal text',
                source='test'
            )

            await manager.update_item(pinned_item.id, is_pinned=True)

            pinned_items = await manager.list_items(pinned_only=True)
            assert len(pinned_items) == 1
            assert pinned_items[0].id == pinned_item.id
            assert pinned_items[0].id != normal_item.id
        finally:
            shutil.rmtree(test_root, ignore_errors=True)

    asyncio.run(run_test())
