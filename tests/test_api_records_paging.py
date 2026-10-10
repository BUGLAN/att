"""GET /api/records 分页行为与记录索引回归测试。

覆盖 task-9 第 4 项验收：
- 默认 limit 从 100 提到 200（上限 1000），响应体仍是「裸数组」；
- 新增 offset 查询参数，能取到后续记录，且顺序与未分页、与重构前 manager 扫描一致；
- /api/records/search 的 page_size 上限从 100 提到 200；
- 进程内索引 `total` 与真实 JSON 文件数一致，且在索引异常时接口仍能回落 manager 扫描。
测试全部使用隔离的临时数据目录，不触碰真实 data/。
"""
import asyncio
from datetime import datetime, timedelta

import pytest
from fastapi import FastAPI
from fastapi.testclient import TestClient

from app.api import routes
from app.records.manager import RecordManager
from app.records.models import RecordStatus
from app.repositories.record_index import get_record_index
from app.services import records_service


def _seed_records(manager: RecordManager, total: int) -> None:
    """预置 total 条记录，并显式拉开 created_at 保证排序确定。"""
    base_time = datetime(2026, 1, 1, 0, 0, 0)
    for index in range(total):
        record = asyncio.run(manager.create_record(
            audio_path="",
            duration=float(index),
            status=RecordStatus.TRANSCRIBED,
            raw_text=f"第 {index} 条",
        ))
        record.created_at = base_time + timedelta(seconds=index)
        asyncio.run(manager._save_json(record))


@pytest.fixture
def records_env(monkeypatch, temp_dir):
    """构造只挂载 api 路由的测试应用 + 隔离 RecordManager。"""
    manager = RecordManager(temp_dir)
    asyncio.run(manager.initialize())
    monkeypatch.setattr(routes, "record_manager", manager)
    monkeypatch.setattr(routes, "streaming_session_manager", None)

    test_app = FastAPI()
    test_app.include_router(routes.router)
    return TestClient(test_app), manager


def test_default_limit_is_200(records_env):
    """不传 limit 时默认取 200 条（重构前是 100 条）。"""
    client, manager = records_env
    _seed_records(manager, 205)

    payload = client.get("/api/records").json()

    assert len(payload) == 200
    assert payload[0]["raw_text"] == "第 204 条"
    assert payload[-1]["raw_text"] == "第 5 条"


def test_limit_upper_bound_is_1000(records_env):
    """limit 上限保持 1000：超过上限由 FastAPI 返回 422。"""
    client, manager = records_env
    _seed_records(manager, 3)

    assert client.get("/api/records", params={"limit": 1000}).status_code == 200
    assert client.get("/api/records", params={"limit": 1001}).status_code == 422


def test_offset_returns_follow_up_records_in_same_order(records_env):
    """offset 能取到后续记录，且与未分页顺序、重构前 manager 扫描顺序一致。"""
    client, manager = records_env
    _seed_records(manager, 7)

    full = client.get("/api/records", params={"limit": 1000}).json()
    first_page = client.get("/api/records", params={"limit": 3, "offset": 0}).json()
    second_page = client.get("/api/records", params={"limit": 3, "offset": 3}).json()

    assert [item["id"] for item in first_page + second_page] == [
        item["id"] for item in full[:6]
    ]
    manager_order = [record.id for record in asyncio.run(manager.list_records(limit=1000))]
    assert [item["id"] for item in full] == manager_order


def test_offset_beyond_total_returns_empty_array(records_env):
    """offset 超过总数时返回空数组而不是报错。"""
    client, manager = records_env
    _seed_records(manager, 3)

    response = client.get("/api/records", params={"offset": 100})

    assert response.status_code == 200
    assert response.json() == []


def test_search_page_size_limit_raised_to_200(records_env):
    """/api/records/search 的 page_size 上限从 100 提到 200。"""
    client, manager = records_env
    _seed_records(manager, 5)

    accepted = client.get("/api/records/search", params={"page_size": 200})
    rejected = client.get("/api/records/search", params={"page_size": 201})

    assert accepted.status_code == 200
    assert accepted.json()["page_size"] == 200
    assert rejected.status_code == 422


def test_record_index_total_matches_real_file_count(records_env):
    """索引的 total 必须等于真实 JSON 文件数（派生清单可从源文件 100% 重建）。"""
    client, manager = records_env
    _seed_records(manager, 6)

    index = get_record_index(manager.data_path)
    items, total = asyncio.run(index.page(limit=1000, offset=0))

    assert total == len(list(manager.records_path.glob("*.json"))) == 6
    assert len(items) == 6
    assert {item["id"] for item in items} == {
        record.id for record in asyncio.run(manager.list_records(limit=1000))
    }


def test_record_index_manifest_is_rebuildable(records_env):
    """派生清单落 data/.index/records.json；删掉后仍能由源 JSON 重建。"""
    client, manager = records_env
    _seed_records(manager, 4)

    index = get_record_index(manager.data_path)
    asyncio.run(index.page(limit=10, offset=0))
    manifest_path = manager.data_path / ".index" / "records.json"
    assert manifest_path.exists()

    # 删除派生清单 + 让内存索引失效，重新装载必须无损
    expected_ids = [item["id"] for item in client.get("/api/records").json()]
    manifest_path.unlink()
    index.invalidate()
    rebuilt = client.get("/api/records").json()

    assert [item["id"] for item in rebuilt] == expected_ids


def test_records_list_falls_back_to_manager_when_index_broken(records_env, monkeypatch):
    """索引异常时必须回落 manager 全目录扫描并照常 200，绝不允许因为索引导致 500。"""
    client, manager = records_env
    _seed_records(manager, 4)

    class _BrokenIndex:
        async def refresh(self):
            raise RuntimeError("模拟索引损坏")

        async def page(self, **kwargs):
            raise RuntimeError("模拟索引损坏")

        def invalidate(self, record_id=None):
            raise RuntimeError("模拟索引损坏")

    monkeypatch.setattr(
        "app.services.records_service.get_record_index",
        lambda data_path: _BrokenIndex(),
    )

    # 记录回落路径是否真的被走到（只断言响应码无法区分「索引恰好也返回 4 条」）
    fallback_calls = []
    original_fallback = records_service._list_records_via_manager

    async def spying_fallback(**kwargs):
        fallback_calls.append(kwargs)
        return await original_fallback(**kwargs)

    monkeypatch.setattr(records_service, "_list_records_via_manager", spying_fallback)

    response = client.get("/api/records")

    assert response.status_code == 200
    assert len(response.json()) == 4
    assert len(fallback_calls) == 1
    assert fallback_calls[0]["limit"] == 200 and fallback_calls[0]["offset"] == 0


def test_status_filter_and_favorite_only_use_index(records_env):
    """status 过滤与 favorite_only 过滤都必须走索引且结果正确。"""
    client, manager = records_env
    _seed_records(manager, 5)
    favorite = asyncio.run(manager.create_record(
        audio_path="",
        duration=9.0,
        status=RecordStatus.OPTIMIZED,
        raw_text="收藏记录",
        is_favorite=True,
    ))

    index = get_record_index(manager.data_path)
    _, total_optimized = asyncio.run(
        index.page(limit=100, offset=0, status=RecordStatus.OPTIMIZED)
    )
    items_favorite, total_favorite = asyncio.run(
        index.page(limit=100, offset=0, favorite_only=True)
    )

    assert total_optimized == 1
    assert total_favorite == 1
    assert items_favorite[0]["id"] == favorite.id

    filtered = client.get("/api/records", params={"status": "transcribed"}).json()
    assert len(filtered) == 5
    assert all(item["status"] == "transcribed" for item in filtered)
