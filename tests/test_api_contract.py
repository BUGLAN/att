"""API 契约锚点：「(HTTP 方法, URL 路径)」集合必须与当前产品范围完全一致。

本文件锁定 `app.main:app` 实际暴露的端点集合：

1. `BUSINESS_ENDPOINTS` 是**功能减法之后**的业务端点基线（49 条：47 条 HTTP + 2 条 WebSocket）。
   2026-10 功能减法删除了统一浮层 / 生词本 / AI 动作（含 Agent 对话），
   对应的 `/api/wordbook/*` 5 条与 `/api/ai-actions/*` 5 条端点随之移除；
2. FastAPI 自动生成的文档路由（/docs、/redoc、/openapi.json 及其 HEAD）与根入口 "/"
   单独登记，避免它们被悄悄塞进「忽略名单」而掩盖端点变更；
3. `API_ROUTER_ENDPOINTS` 锁定聚合层 `app.api.routes.router` 暴露的全部端点，
   保证拆分出的子路由全部被 include，且没有重复挂载；
4. 末尾用例锁定 `GET /api/records` 的「裸数组」响应形态
   （分页行为见 tests/test_api_records_paging.py，本文件只固化端点与响应形态契约）。
"""
import asyncio
from datetime import datetime, timedelta

from fastapi import FastAPI
from fastapi.routing import APIWebSocketRoute
from fastapi.testclient import TestClient

from app.api import routes
from app.main import app
from app.records.manager import RecordManager
from app.records.models import RecordStatus

# ==================== 基线 ====================

# 业务端点基线：功能减法后共 49 条（47 条 HTTP + 2 条 WebSocket）。
# 已删除：/api/wordbook/*（生词本）5 条、/api/ai-actions/*（AI 动作 + Agent 对话）5 条。
BUSINESS_ENDPOINTS = frozenset({
    ("DELETE", "/api/clipboard/items/{item_id}"),
    ("DELETE", "/api/llm/configs/{config_id}"),
    ("DELETE", "/api/prompts/{preset_id}"),
    ("DELETE", "/api/records/{record_id}"),
    ("DELETE", "/api/sessions/{session_id}"),
    ("GET", "/api/clipboard/items"),
    ("GET", "/api/clipboard/items/{item_id}"),
    ("GET", "/api/clipboard/items/{item_id}/image"),
    ("GET", "/api/clipboard/stats"),
    ("GET", "/api/health"),
    ("GET", "/api/llm/configs"),
    ("GET", "/api/llm/configs/{config_id}"),
    ("GET", "/api/llm/providers"),
    ("GET", "/api/llm/providers/{provider}/models"),
    ("GET", "/api/model-service"),
    ("GET", "/api/prompts"),
    ("GET", "/api/prompts/{preset_id}"),
    ("GET", "/api/records"),
    ("GET", "/api/records/search"),
    ("GET", "/api/records/{record_id}"),
    ("GET", "/api/records/{record_id}/audio"),
    ("GET", "/api/records/{record_id}/export"),
    ("GET", "/api/records/{record_id}/status"),
    ("GET", "/api/sessions/{session_id}"),
    ("GET", "/api/settings"),
    ("GET", "/api/streaming/service"),
    ("POST", "/api/audio/upload"),
    ("POST", "/api/clipboard/items"),
    ("POST", "/api/llm/configs"),
    ("POST", "/api/llm/configs/{config_id}/test"),
    ("POST", "/api/model-service/start"),
    ("POST", "/api/model-service/stop"),
    ("POST", "/api/prompts"),
    ("POST", "/api/prompts/{preset_id}/preview"),
    ("POST", "/api/records/{record_id}/optimize"),
    ("POST", "/api/sessions"),
    ("POST", "/api/sessions/{session_id}/merge"),
    ("POST", "/api/sessions/{session_id}/segments"),
    ("POST", "/api/streaming/service/start"),
    ("POST", "/api/streaming/service/stop"),
    ("PUT", "/api/clipboard/items/{item_id}"),
    ("PUT", "/api/llm/configs/{config_id}"),
    ("PUT", "/api/llm/configs/{config_id}/default"),
    ("PUT", "/api/prompts/default"),
    ("PUT", "/api/prompts/{preset_id}"),
    ("PUT", "/api/records/{record_id}"),
    ("PUT", "/api/settings"),
    ("WS", "/api/asr/stream"),
    ("WS", "/api/ws"),
})

# 拆分后由 app/api/routes.py（薄聚合层）暴露的 42 条端点：
# 音频+记录 10、会话 5、WebSocket 2、设置 2、健康检查+模型服务 7、提示词 7、LLM 9。
API_ROUTER_ENDPOINTS = frozenset({
    ("DELETE", "/api/llm/configs/{config_id}"),
    ("DELETE", "/api/prompts/{preset_id}"),
    ("DELETE", "/api/records/{record_id}"),
    ("DELETE", "/api/sessions/{session_id}"),
    ("GET", "/api/health"),
    ("GET", "/api/llm/configs"),
    ("GET", "/api/llm/configs/{config_id}"),
    ("GET", "/api/llm/providers"),
    ("GET", "/api/llm/providers/{provider}/models"),
    ("GET", "/api/model-service"),
    ("GET", "/api/prompts"),
    ("GET", "/api/prompts/{preset_id}"),
    ("GET", "/api/records"),
    ("GET", "/api/records/search"),
    ("GET", "/api/records/{record_id}"),
    ("GET", "/api/records/{record_id}/audio"),
    ("GET", "/api/records/{record_id}/export"),
    ("GET", "/api/records/{record_id}/status"),
    ("GET", "/api/sessions/{session_id}"),
    ("GET", "/api/settings"),
    ("GET", "/api/streaming/service"),
    ("POST", "/api/audio/upload"),
    ("POST", "/api/llm/configs"),
    ("POST", "/api/llm/configs/{config_id}/test"),
    ("POST", "/api/model-service/start"),
    ("POST", "/api/model-service/stop"),
    ("POST", "/api/prompts"),
    ("POST", "/api/prompts/{preset_id}/preview"),
    ("POST", "/api/records/{record_id}/optimize"),
    ("POST", "/api/sessions"),
    ("POST", "/api/sessions/{session_id}/merge"),
    ("POST", "/api/sessions/{session_id}/segments"),
    ("POST", "/api/streaming/service/start"),
    ("POST", "/api/streaming/service/stop"),
    ("PUT", "/api/llm/configs/{config_id}"),
    ("PUT", "/api/llm/configs/{config_id}/default"),
    ("PUT", "/api/prompts/default"),
    ("PUT", "/api/prompts/{preset_id}"),
    ("PUT", "/api/records/{record_id}"),
    ("PUT", "/api/settings"),
    ("WS", "/api/asr/stream"),
    ("WS", "/api/ws"),
})

# FastAPI 自动生成的文档路由（非业务契约，单独登记以免被当作忽略项）。
DOC_ENDPOINTS = frozenset({
    ("GET", "/docs"),
    ("HEAD", "/docs"),
    ("GET", "/docs/oauth2-redirect"),
    ("HEAD", "/docs/oauth2-redirect"),
    ("GET", "/openapi.json"),
    ("HEAD", "/openapi.json"),
    ("GET", "/redoc"),
    ("HEAD", "/redoc"),
})

# 根入口：非 /api 前缀，但同样属于对外 HTTP 契约（dist 存在时返回前端首页）。
ROOT_ENDPOINTS = frozenset({("GET", "/")})

# GET /api/records 单条记录的**基线字段**（重构前 RecordsLibraryPage 依赖的 11 个键）。
# 断言用「必须包含」而非「完全相等」：后续任务（task-10 等）可能追加字段（例如 error），
# 那是各自任务负责的契约变更；本文件只保证路由拆分不会丢失任何原有字段。
RECORD_FIELDS = frozenset({
    "id", "created_at", "updated_at", "audio_path", "duration",
    "raw_text", "optimized_text", "status", "processing_time",
    "is_modified", "is_favorite",
})


def _collect(route_holder) -> list[tuple[str, str]]:
    """枚举路由对象上的全部端点，WebSocket 用伪方法 "WS" 标记。"""
    entries: list[tuple[str, str]] = []
    for route in route_holder.routes:
        path = getattr(route, "path", None)
        if path is None:
            continue
        if isinstance(route, APIWebSocketRoute):
            entries.append(("WS", path))
            continue
        for method in sorted(getattr(route, "methods", None) or set()):
            entries.append((method, path))
    return entries


# ==================== 端点集合契约 ====================


def test_baseline_size_is_49():
    """基线自身自检：49 条业务端点（47 条 HTTP + 2 个 WebSocket）。"""
    assert len(BUSINESS_ENDPOINTS) == 49
    assert len([item for item in BUSINESS_ENDPOINTS if item[0] == "WS"]) == 2


def test_application_endpoint_set_matches_baseline():
    """app.main:app 的端点集合必须与基线完全一致（缺失/新增都会红灯）。"""
    discovered = _collect(app)

    duplicated = sorted({item for item in discovered if discovered.count(item) > 1})
    assert duplicated == [], f"同一 (方法, 路径) 被重复挂载: {duplicated}"

    expected = BUSINESS_ENDPOINTS | DOC_ENDPOINTS | ROOT_ENDPOINTS
    missing = sorted(expected - set(discovered))
    added = sorted(set(discovered) - expected)

    assert missing == [], f"缺失端点（契约破坏）: {missing}"
    assert added == [], f"新增未登记端点: {added}"


def test_business_endpoints_are_still_49_after_subtraction():
    """业务端点（排除文档路由与根入口）仍恰好是 49 条。"""
    business = set(_collect(app)) - DOC_ENDPOINTS - ROOT_ENDPOINTS

    assert business == BUSINESS_ENDPOINTS
    assert len(business) == 49
    # 已下线功能（生词本 / AI 动作）的端点必须彻底消失
    assert not any(path.startswith("/api/wordbook") for _, path in business)
    assert not any(path.startswith("/api/ai-actions") for _, path in business)


def test_api_aggregator_router_exposes_all_registered_endpoints():
    """app.api.routes.router（聚合层）必须恰好暴露 API_ROUTER_ENDPOINTS 登记的全部端点。"""
    entries = _collect(routes.router)

    duplicated = sorted({item for item in entries if entries.count(item) > 1})
    assert duplicated == [], f"聚合层重复挂载端点: {duplicated}"
    assert set(entries) == API_ROUTER_ENDPOINTS
    assert len(entries) == len(API_ROUTER_ENDPOINTS)


# ==================== GET /api/records 响应形态契约 ====================


def _build_records_client(monkeypatch, temp_dir, total: int = 5):
    """构造只挂载 api 路由的测试应用，并预置 total 条隔离记录。"""
    manager = RecordManager(temp_dir)
    asyncio.run(manager.initialize())

    base_time = datetime(2026, 1, 1, 0, 0, 0)
    for index in range(total):
        record = asyncio.run(manager.create_record(
            audio_path="",
            duration=float(index),
            status=RecordStatus.TRANSCRIBED,
            raw_text=f"第 {index} 条",
        ))
        # 显式拉开 created_at，保证排序确定（列表按创建时间倒序）
        record.created_at = base_time + timedelta(seconds=index)
        asyncio.run(manager._save_json(record))

    monkeypatch.setattr(routes, "record_manager", manager)
    monkeypatch.setattr(routes, "streaming_session_manager", None)

    test_app = FastAPI()
    test_app.include_router(routes.router)
    return TestClient(test_app), manager


def test_list_records_response_is_bare_array(monkeypatch, temp_dir):
    """GET /api/records 必须仍是记录数组，不能被包装成 {items,total}。"""
    client, _ = _build_records_client(monkeypatch, temp_dir, total=5)

    response = client.get("/api/records")

    assert response.status_code == 200
    payload = response.json()
    assert isinstance(payload, list), f"响应体被包装了: {type(payload)!r}"
    assert len(payload) == 5
    # 原有 11 个字段一个都不能少（新增字段由对应任务自行负责）
    assert RECORD_FIELDS <= set(payload[0])
