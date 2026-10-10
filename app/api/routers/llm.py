"""LLM 配置与 provider 元数据路由。

从 `app/api/routes.py` 拆分而来（原实现把 LLM 相关 import 放在文件中段，
拆分后统一提到模块顶部）。所有端点都是「调用 config manager / gateway + 组装响应」。
"""

from __future__ import annotations

from fastapi import APIRouter, HTTPException

from app.llm.config_manager import get_llm_config_manager
from app.llm.gateway import get_gateway
from app.llm.models import (
    PROVIDER_INFO,
    LLMConfigCreate,
    LLMConfigUpdate,
    LLMProvider,
    get_provider_models,
)
from app.utils.logging import get_logger

logger = get_logger(__name__)

router = APIRouter(prefix="/api", tags=["api", "llm"])

_llm_config_manager = None


def get_llm_manager():
    """Get or create LLM config manager instance."""
    global _llm_config_manager
    if _llm_config_manager is None:
        _llm_config_manager = get_llm_config_manager()
    return _llm_config_manager


@router.get("/llm/configs")
async def list_llm_configs() -> dict:
    """Get all LLM configurations."""
    cm = get_llm_manager()
    configs = cm.get_all()
    return {
        "configs": [
            {
                "id": c.id,
                "provider": c.provider.value,
                "name": c.name,
                "model": c.model,
                "api_base": c.api_base,
                "temperature": c.temperature,
                "max_tokens": c.max_tokens,
                "is_default": c.is_default,
                "created_at": c.created_at.isoformat(),
                "updated_at": c.updated_at.isoformat() if c.updated_at else None
                # Note: api_key is NOT returned for security
            }
            for c in configs
        ],
        "default_config_id": cm._default_config_id
    }


@router.get("/llm/configs/{config_id}")
async def get_llm_config(config_id: str) -> dict:
    """Get a single LLM configuration by ID."""
    cm = get_llm_manager()
    config = cm.get_by_id(config_id)
    if not config:
        raise HTTPException(status_code=404, detail="Configuration not found")

    return {
        "id": config.id,
        "provider": config.provider.value,
        "name": config.name,
        "model": config.model,
        "api_base": config.api_base,
        "temperature": config.temperature,
        "max_tokens": config.max_tokens,
        "is_default": config.is_default,
        "created_at": config.created_at.isoformat(),
        "updated_at": config.updated_at.isoformat() if config.updated_at else None
        # Note: api_key is NOT returned for security
    }


@router.post("/llm/configs")
async def create_llm_config(config_data: LLMConfigCreate) -> dict:
    """Create a new LLM configuration."""
    cm = get_llm_manager()
    try:
        config = cm.create(config_data)
        return {
            "id": config.id,
            "provider": config.provider.value,
            "name": config.name,
            "model": config.model,
            "is_default": config.is_default
        }
    except Exception as e:
        raise HTTPException(status_code=400, detail=str(e))


@router.put("/llm/configs/{config_id}")
async def update_llm_config(config_id: str, config_data: LLMConfigUpdate) -> dict:
    """Update an existing LLM configuration."""
    cm = get_llm_manager()
    try:
        config = cm.update(config_id, config_data)
        if not config:
            raise HTTPException(status_code=404, detail="Configuration not found")
        return {
            "id": config.id,
            "name": config.name,
            "model": config.model,
            "updated_at": config.updated_at.isoformat() if config.updated_at else None
        }
    except ValueError as e:
        raise HTTPException(status_code=400, detail=str(e))


@router.delete("/llm/configs/{config_id}")
async def delete_llm_config(config_id: str) -> dict:
    """Delete an LLM configuration."""
    cm = get_llm_manager()
    success = cm.delete(config_id)
    if not success:
        raise HTTPException(status_code=404, detail="Configuration not found")

    # Invalidate gateway cache
    gateway = get_gateway()
    gateway.invalidate(config_id)

    return {"status": "deleted", "config_id": config_id}


@router.put("/llm/configs/{config_id}/default")
async def set_default_llm_config(config_id: str) -> dict:
    """Set the default LLM configuration."""
    cm = get_llm_manager()
    success = cm.set_default(config_id)
    if not success:
        raise HTTPException(status_code=404, detail="Configuration not found")

    # Invalidate gateway cache
    gateway = get_gateway()
    gateway.clear_cache()

    return {"status": "updated", "default_config_id": config_id}


@router.post("/llm/configs/{config_id}/test")
async def test_llm_config(config_id: str) -> dict:
    """Test LLM configuration connection."""
    cm = get_llm_manager()
    config = cm.get_by_id(config_id)
    if not config:
        raise HTTPException(status_code=404, detail="Configuration not found")

    gateway = get_gateway()
    provider = gateway.get_provider(config_id)

    if not provider:
        return {"success": False, "error": "Failed to create provider"}

    return await provider.test_connection()


@router.get("/llm/providers")
async def list_llm_providers() -> dict:
    """Get supported LLM providers."""
    return {
        "providers": [
            {
                "id": p.value,
                "name": info["name"],
                "default_base_url": info["default_base_url"],
                "models": info["models"],
                "default_model": info["default_model"]
            }
            for p, info in PROVIDER_INFO.items()
        ]
    }


@router.get("/llm/providers/{provider}/models")
async def get_llm_provider_models(provider: str) -> dict:
    """Get available models for a provider."""
    try:
        p = LLMProvider(provider)
        models = get_provider_models(p)
        return {"provider": provider, "models": models}
    except ValueError:
        raise HTTPException(status_code=400, detail=f"Unknown provider: {provider}")
