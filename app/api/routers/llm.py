"""LLM v2 路由：供应商档案 + 模型条目 + 验证（docs/LLM配置模块重设计.md §6）。

约定：
- 本模块只做「解析请求 → 调 store/gateway → 组装响应」，业务规则在 app/llm/；
- 任何响应不得包含密钥明文/密文（统一走 models.profile_public_dict 等安全序列化）；
- 验证类端点返回 200 + {ok, error_code, ...}（验证失败是正常业务结果，不是传输错误）；
- v1 的 /api/llm/configs* 已随 v2 上线整体移除（前后端同版本切换，不留过渡期）。
"""

from __future__ import annotations

from datetime import datetime
from typing import Optional

from fastapi import APIRouter, HTTPException, Query

from app.llm.errors import LLMError
from app.llm.gateway import get_llm_gateway
from app.llm.models import (
    DraftVerifyRequest,
    LLMSettingsUpdate,
    ModelUpdate,
    ModelsBatchCreate,
    ProfileCreate,
    ProfileUpdate,
    VerifyStatus,
    model_public_dict,
    profile_public_dict,
)
from app.llm.registry import compose_model_string, get_provider_spec, provider_catalog
from app.llm.store import LLMStore, get_llm_store
from app.utils.logging import get_logger

logger = get_logger(__name__)

router = APIRouter(prefix="/api", tags=["api", "llm"])


def get_store() -> LLMStore:
    """测试通过 monkeypatch 本函数替换 store（沿用旧 llm.py 的可替换单例模式）。"""
    return get_llm_store()


def _failed_outcome(exc: LLMError) -> dict:
    return {"ok": False, "error_code": exc.code, "message": exc.user_message, "latency_ms": None, "remote_models": []}


def _now_status(outcome) -> VerifyStatus:
    """把网关的 VerifyOutcome 落成可持久化的 VerifyStatus。"""
    return VerifyStatus(
        state="ok" if outcome.ok else "failed",
        checked_at=datetime.now().isoformat(timespec="seconds"),
        latency_ms=outcome.latency_ms,
        error_code=None if outcome.ok else outcome.error_code,
        error=outcome.message,
    )


def _bad_request(exc: ValueError) -> HTTPException:
    return HTTPException(status_code=400, detail=str(exc))


# ==================== 服务商目录 ====================


@router.get("/llm/providers")
async def list_llm_providers() -> dict:
    """服务商目录：只含协议前缀与元信息，不含硬编码模型清单。"""
    return {"providers": provider_catalog()}


@router.get("/llm/state")
async def get_llm_state() -> dict:
    """档案 + 模型 + 默认绑定 + 损坏条目提示，一次拉全（设置页首屏用）。"""
    return get_store().overview()


# ==================== 供应商档案 ====================


@router.get("/llm/profiles")
async def list_llm_profiles() -> dict:
    return {"profiles": [profile_public_dict(p) for p in get_store().list_profiles()]}


@router.post("/llm/profiles")
async def create_llm_profile(data: ProfileCreate) -> dict:
    try:
        profile = get_store().create_profile(data)
    except ValueError as exc:
        raise _bad_request(exc)
    return profile_public_dict(profile)


@router.get("/llm/profiles/{profile_id}")
async def get_llm_profile(profile_id: str) -> dict:
    profile = get_store().get_profile(profile_id)
    if profile is None:
        raise HTTPException(status_code=404, detail="供应商档案不存在")
    return profile_public_dict(profile)


@router.put("/llm/profiles/{profile_id}")
async def update_llm_profile(profile_id: str, data: ProfileUpdate) -> dict:
    try:
        profile = get_store().update_profile(profile_id, data)
    except ValueError as exc:
        raise _bad_request(exc)
    if profile is None:
        raise HTTPException(status_code=404, detail="供应商档案不存在")
    return profile_public_dict(profile)


@router.delete("/llm/profiles/{profile_id}")
async def delete_llm_profile(profile_id: str) -> dict:
    result = get_store().delete_profile(profile_id)
    if result == "not_found":
        raise HTTPException(status_code=404, detail="供应商档案不存在")
    if result == "conflict_default":
        raise HTTPException(status_code=409, detail="该档案下包含默认优化模型，请先把默认模型切换到其他模型再删除")
    return {"status": "deleted", "profile_id": profile_id}


@router.post("/llm/profiles/{profile_id}/verify")
async def verify_llm_profile(profile_id: str) -> dict:
    """验证已存档案：鉴权 + 尽力列取上游模型。结果会持久化到档案状态。"""
    store = get_store()
    profile = store.get_profile(profile_id)
    if profile is None:
        raise HTTPException(status_code=404, detail="供应商档案不存在")
    spec = get_provider_spec(profile.provider)
    if spec is None:
        return _failed_outcome(LLMError("unsupported_provider", f"未知服务商：{profile.provider}", hint=False))
    try:
        plain_key = store.decrypt_key(profile)
    except LLMError as exc:
        store.set_profile_status(
            profile_id,
            VerifyStatus(state="failed", checked_at=datetime.now().isoformat(timespec="seconds"), error_code=exc.code, error=exc.user_message),
        )
        return _failed_outcome(exc)
    outcome = await get_llm_gateway().verify_profile(
        spec=spec,
        api_key=plain_key,
        api_base=profile.api_base,
        extra_headers=profile.extra_headers or None,
    )
    store.set_profile_status(profile_id, _now_status(outcome))
    return outcome.to_dict()


@router.post("/llm/profiles/{profile_id}/remote-models")
async def list_remote_models(profile_id: str) -> dict:
    """拉取上游模型列表；上游不支持列取时返回 409 + manual_only。"""
    store = get_store()
    profile = store.get_profile(profile_id)
    if profile is None:
        raise HTTPException(status_code=404, detail="供应商档案不存在")
    spec = get_provider_spec(profile.provider)
    if spec is None or not spec.supports_model_listing:
        raise HTTPException(status_code=409, detail={"manual_only": True, "reason": "该服务商不提供 OpenAI 兼容的模型列表接口，请手动输入模型名"})
    try:
        plain_key = store.decrypt_key(profile)
    except LLMError as exc:
        raise HTTPException(status_code=400, detail=exc.user_message)
    models, error = await get_llm_gateway().list_remote_models(
        api_key=plain_key,
        api_base=profile.api_base or spec.default_base_url,
        extra_headers=profile.extra_headers or None,
    )
    if models is None:
        raise HTTPException(status_code=409, detail={"manual_only": True, "reason": error or "模型列表拉取失败"})
    return {"profile_id": profile_id, "models": models}


# ==================== 保存前验证（草稿） ====================


@router.post("/llm/verify-draft")
async def verify_llm_draft(data: DraftVerifyRequest) -> dict:
    """保存前验证：草稿密钥只活在本次请求内存里，验证后即弃，不落任何文件。"""
    spec = get_provider_spec(data.provider)
    if spec is None:
        return _failed_outcome(LLMError("unsupported_provider", f"未知服务商：{data.provider}", hint=False))
    api_base = (data.api_base or "").strip() or None
    if spec.id == "custom" and not api_base:
        return _failed_outcome(LLMError("network_error", "自定义 OpenAI 兼容端点必须填写 API Base", hint=False))
    gateway = get_llm_gateway()
    if data.model_name:
        try:
            model_string = compose_model_string(data.provider, data.model_name, api_base)
        except ValueError as exc:
            return _failed_outcome(LLMError("model_not_found", str(exc), hint=False))
        outcome = await gateway.verify_model(
            model_string=model_string,
            api_key=data.api_key,
            api_base=api_base,
            extra_headers=data.extra_headers or None,
        )
    else:
        outcome = await gateway.verify_profile(
            spec=spec,
            api_key=data.api_key,
            api_base=api_base,
            extra_headers=data.extra_headers or None,
        )
    return outcome.to_dict()


# ==================== 模型条目 ====================


@router.get("/llm/models")
async def list_llm_models(profile_id: Optional[str] = Query(default=None)) -> dict:
    models = get_store().list_models(profile_id)
    return {"models": [model_public_dict(m) for m in models]}


@router.post("/llm/models")
async def create_llm_models(data: ModelsBatchCreate) -> dict:
    """批量创建（向导勾选多个模型一次提交）。"""
    try:
        result = get_store().create_models(data.items)
    except ValueError as exc:
        raise _bad_request(exc)
    return {
        "created": [model_public_dict(m) for m in result["created"]],
        "skipped": result["skipped"],
    }


@router.put("/llm/models/{model_id}")
async def update_llm_model(model_id: str, data: ModelUpdate) -> dict:
    try:
        entry = get_store().update_model(model_id, data)
    except ValueError as exc:
        raise _bad_request(exc)
    if entry is None:
        raise HTTPException(status_code=404, detail="模型不存在")
    return model_public_dict(entry)


@router.delete("/llm/models/{model_id}")
async def delete_llm_model(model_id: str) -> dict:
    result = get_store().delete_model(model_id)
    if result == "not_found":
        raise HTTPException(status_code=404, detail="模型不存在")
    if result == "conflict_default":
        raise HTTPException(status_code=409, detail="该模型是当前默认优化模型，请先切换默认模型再删除")
    return {"status": "deleted", "model_id": model_id}


@router.post("/llm/models/{model_id}/verify")
async def verify_llm_model(model_id: str) -> dict:
    """单模型轻量验证（约 5 token 补全）。结果持久化到模型状态。"""
    store = get_store()
    try:
        entry, profile, plain_key = store.resolve_runtime(model_id)
    except LLMError as exc:
        store.set_model_status(
            model_id,
            VerifyStatus(state="failed", checked_at=datetime.now().isoformat(timespec="seconds"), error_code=exc.code, error=exc.user_message),
        )
        return _failed_outcome(exc)
    outcome = await get_llm_gateway().verify_model(
        model_string=entry.model,
        api_key=plain_key,
        api_base=profile.api_base,
        extra_headers=profile.extra_headers or None,
    )
    store.set_model_status(model_id, _now_status(outcome))
    return outcome.to_dict()


@router.post("/llm/models/{model_id}/activate")
async def activate_llm_model(model_id: str) -> dict:
    """设为默认优化模型（即时生效，下一次优化请求即用新模型）。"""
    if not get_store().set_default_model(model_id):
        raise HTTPException(status_code=404, detail="模型不存在")
    return {"status": "updated", "default_model_id": model_id}


@router.put("/llm/settings")
async def update_llm_settings(data: LLMSettingsUpdate) -> dict:
    """更新默认/备用模型绑定。fallback 显式传 null 表示清除（失败不自动切换）。"""
    store = get_store()
    if data.default_model_id is not None and not store.set_default_model(data.default_model_id):
        raise HTTPException(status_code=404, detail="默认模型不存在")
    if "fallback_model_id" in data.model_fields_set and not store.set_fallback_model(data.fallback_model_id):
        raise HTTPException(status_code=404, detail="备用模型不存在")
    default_entry = store.get_default_model()
    fallback_entry = store.get_fallback_model()
    return {
        "status": "updated",
        "default_model_id": default_entry.id if default_entry else None,
        "fallback_model_id": fallback_entry.id if fallback_entry else None,
    }
