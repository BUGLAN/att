"""API 契约锚点：路由拆分前后「(HTTP 方法, URL 路径)」集合必须完全一致。

本文件是 `app/api/routes.py` 拆分重构的安全网（前端契约一字不改是本任务的最高优先级）：

1. `BUSINESS_ENDPOINTS` 是**重构前**从 `app.main:app` 实测得到的 58 条业务端点
   （56 条 HTTP + 2 条 WebSocket，见任务 task-9），已硬编码为基线；
   task-13（W3-C2 Agent 对话）**新增** `POST /api/ai-actions/chat` 一条，
   因此基线为 58 + 1 = 59 条；原有 58 条一条未减，新增端点已在集合内用注释标明；
2. FastAPI 自动生成的文档路由（/docs、/redoc、/openapi.json 及其 HEAD）与根入口 "/"
   单独登记，避免它们被悄悄塞进「忽略名单」而掩盖端点变更；
3. `API_ROUTER_ENDPOINTS` 锁定聚合层 `app.api.routes.router` 暴露的 42 条端点，
   保证拆分出的子路由全部被 include，且没有重复挂载
   （chat 挂在 `app.ai_actions.routes`，不经聚合层，故该集合仍为 42 条）；
4. 末尾用例锁定 `GET /api/records` 的「裸数组」响应形态
   （新增 `offset`、默认 `limit` 提到 200 的分页行为见 tests/test_api_records_paging.py，
   本文件保持「重构前即可跑绿」，只固化拆分前后都必须成立的契约）。
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

# 业务端点基线：58 条（重构前） + 1 条（task-13 新增 Agent 对话） = 59 条，原有 58 条不得增删改。
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
    ("GET", "/api/wordbook"),
    ("GET", "/api/wordbook/stats"),
    ("GET", "/api/wordbook/{entry_id}"),
    # ===== task-13 新增（Agent 对话，设置页「Agent 对话」分组使用）=====
    ("POST", "/api/ai-actions/chat"),
    # ===== 以下为重构前既有 58 条基线，一条都不能少 =====
    ("POST", "/api/ai-actions/explain"),
    ("POST", "/api/ai-actions/rewrite"),
    ("POST", "/api/ai-actions/summarize"),
    ("POST", "/api/ai-actions/translate"),
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
    ("POST", "/api/wordbook"),
    ("PUT", "/api/clipboard/items/{item_id}"),
    ("PUT", "/api/llm/configs/{config_id}"),
    ("PUT", "/api/llm/configs/{config_id}/default"),
    ("PUT", "/api/prompts/default"),
    ("PUT", "/api/prompts/{preset_id}"),
    ("PUT", "/api/records/{record_id}"),
    ("PUT", "/api/settings"),
    ("PUT", "/api/wordbook/{entry_id}"),
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


def test_baseline_size_is_59():
    """基线自身自检：59 条业务端点（58 条重构前基线 + 1 条 task-13 新增），含 2 个 WebSocket。"""
    assert len(BUSINESS_ENDPOINTS) == 59
    assert len([item for item in BUSINESS_ENDPOINTS if item[0] == "WS"]) == 2


def test_application_endpoint_set_matches_baseline():
    """app.main:app 的端点集合必须与重构前完全一致（缺失/新增都会红灯）。"""
    discovered = _collect(app)

    duplicated = sorted({item for item in discovered if discovered.count(item) > 1})
    assert duplicated == [], f"同一 (方法, 路径) 被重复挂载: {duplicated}"

    expected = BUSINESS_ENDPOINTS | DOC_ENDPOINTS | ROOT_ENDPOINTS
    missing = sorted(expected - set(discovered))
    added = sorted(set(discovered) - expected)

    assert missing == [], f"重构后丢失端点（前端契约破坏）: {missing}"
    assert added == [], f"重构后新增未登记端点: {added}"


def test_business_endpoints_are_still_59_after_split():
    """业务端点（排除文档路由与根入口）仍恰好是 59 条（58 条基线一条不少 + chat 1 条）。"""
    business = set(_collect(app)) - DOC_ENDPOINTS - ROOT_ENDPOINTS

    assert business == BUSINESS_ENDPOINTS
    assert len(business) == 59
    # 新增端点显式点名，避免以后有人把它连带删掉
    assert ("POST", "/api/ai-actions/chat") in business


def test_api_aggregator_router_exposes_all_42_endpoints():
    """app.api.routes.router（被 main.py include 的聚合层）必须恰好暴露 42 条端点。"""
    entries = _collect(routes.router)

    duplicated = sorted({item for item in entries if entries.count(item) > 1})
    assert duplicated == [], f"聚合层重复挂载端点: {duplicated}"
    assert set(entries) == API_ROUTER_ENDPOINTS
    assert len(entries) == 42


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
