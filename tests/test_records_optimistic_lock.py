"""乐观锁（revision）验收：并发写不得静默丢更新。

对应报告 §3.5③ —— JSON 文件存储没有事务，用「版本号 + 冲突检测」
把静默丢数据变成明确报错。
"""
import asyncio
import shutil
import sys
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))

from app.records.manager import RecordManager  # noqa: E402
from app.records.models import Record, RecordStatus  # noqa: E402


@pytest.fixture()
def manager(tmp_path):
    mgr = RecordManager(tmp_path)
    asyncio.get_event_loop_policy().new_event_loop().run_until_complete(mgr.initialize())
    return mgr


def run(coro):
    return asyncio.new_event_loop().run_until_complete(coro)


def test_revision_increments_on_each_write(manager):
    record = run(manager.create_record(audio_path="a.wav", duration=1.0))
    assert record.revision == 0

    updated = run(manager.update_record(record.id, raw_text="hi"))
    assert updated.revision == 1

    updated2 = run(manager.update_record(record.id, optimized_text="hi!"))
    assert updated2.revision == 2


def test_stale_revision_is_rejected(manager):
    """携带过期 revision 的写入必须被拒绝（返回 None），而不是覆盖别人的改动。"""
    record = run(manager.create_record(audio_path="a.wav", duration=1.0))
    stale = record.revision  # 0

    # 另一个写者先改了一次 → 服务端 revision 变成 1
    run(manager.update_record(record.id, raw_text="first writer"))

    # 拿着旧 revision 的写者必须失败
    result = run(manager.update_record(record.id, raw_text="second writer", expected_revision=stale))
    assert result is None

    # 数据必须是第一个写者的内容，没被静默覆盖
    current = run(manager.get_record(record.id))
    assert current.raw_text == "first writer"
    assert current.revision == 1


def test_matching_revision_succeeds(manager):
    record = run(manager.create_record(audio_path="a.wav", duration=1.0))
    run(manager.update_record(record.id, raw_text="v1"))
    current = run(manager.get_record(record.id))

    ok = run(manager.update_record(record.id, raw_text="v2", expected_revision=current.revision))
    assert ok is not None
    assert run(manager.get_record(record.id)).raw_text == "v2"


def test_legacy_json_without_revision_is_compatible(manager):
    """老 JSON 没有 revision 字段时必须按 0 处理，不需要批量改写 709 个文件。"""
    legacy = {
        "id": "legacy-1",
        "created_at": "2026-01-01T00:00:00",
        "updated_at": "2026-01-01T00:00:00",
        "audio_path": "",
        "duration": 1.0,
        "raw_text": "old",
        "optimized_text": "",
        "status": "transcribed",
        "processing_time": 0.1,
        "is_modified": False,
        "is_favorite": False,
    }
    (manager.records_path / "legacy-1.json").write_text(
        __import__("json").dumps(legacy, ensure_ascii=False), encoding="utf-8"
    )

    loaded = run(manager.get_record("legacy-1"))
    assert loaded.revision == 0
    assert loaded.error == ""

    updated = run(manager.update_record("legacy-1", expected_revision=0, raw_text="new"))
    assert updated is not None
    assert updated.revision == 1


def test_to_dict_includes_revision_and_error(manager):
    record = run(manager.create_record(audio_path="a.wav", duration=1.0))
    payload = record.to_dict()
    assert "revision" in payload
    assert "error" in payload
    assert payload["revision"] == 0
