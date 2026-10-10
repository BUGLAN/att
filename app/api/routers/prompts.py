"""提示词预设（prompts）路由。

从 `app/api/routes.py` 拆分而来，全部端点都是「调用 PromptManager + 组装响应」，
提示词管理器的读入口与预设归一化统一复用 `app/services/settings_service.py`
（与 GET/PUT /api/settings 的归一化口径保持一致）。
"""

from __future__ import annotations

from fastapi import APIRouter, Depends, HTTPException
from pydantic import BaseModel

from app.api.deps import runtime_state
from app.prompts import PromptPresetCreate, PromptPresetUpdate
from app.services import settings_service
from app.utils.logging import get_logger

logger = get_logger(__name__)

router = APIRouter(prefix="/api", tags=["api", "prompts"])


class PromptPreviewRequest(BaseModel):
    """Request model for prompt preview."""
    text: str


class SetDefaultPromptRequest(BaseModel):
    """Request model for updating the default prompt preset."""
    preset_id: str


# ==================== Prompt Preset Endpoints ====================

@router.get("/prompts")
async def list_prompts(state=Depends(runtime_state)) -> dict:
    """Get all prompt presets."""
    pm = settings_service.get_prompt_manager()
    normalized_settings = settings_service.normalize_preset_settings(
        state.user_settings.get_all(),
        user_settings=state.user_settings,
        persist=True,
    )
    presets = pm.get_all()
    return {
        "presets": [p.model_dump() for p in presets],
        "default_preset_id": normalized_settings.get("default_preset_id"),
        "last_used_preset_id": normalized_settings.get("default_preset_id")
    }


@router.put("/prompts/default")
async def set_default_prompt(
    request: SetDefaultPromptRequest,
    state=Depends(runtime_state),
) -> dict:
    """Set the default prompt preset."""
    pm = settings_service.get_prompt_manager()
    if not pm.set_default(request.preset_id):
        raise HTTPException(status_code=404, detail="Preset not found")

    state.user_settings.update({
        "default_preset_id": request.preset_id,
        "last_used_preset_id": request.preset_id
    })

    return {
        "status": "updated",
        "default_preset_id": request.preset_id
    }


@router.get("/prompts/{preset_id}")
async def get_prompt(preset_id: str) -> dict:
    """Get a single prompt preset by ID."""
    pm = settings_service.get_prompt_manager()
    preset = pm.get_by_id(preset_id)
    if not preset:
        raise HTTPException(status_code=404, detail="Preset not found")
    return preset.model_dump()


@router.post("/prompts")
async def create_prompt(preset_data: PromptPresetCreate) -> dict:
    """Create a new custom prompt preset."""
    pm = settings_service.get_prompt_manager()
    try:
        preset = pm.create(preset_data)
        return preset.model_dump()
    except Exception as e:
        raise HTTPException(status_code=400, detail=str(e))


@router.put("/prompts/{preset_id}")
async def update_prompt(preset_id: str, preset_data: PromptPresetUpdate) -> dict:
    """Update an existing custom preset."""
    pm = settings_service.get_prompt_manager()
    try:
        preset = pm.update(preset_id, preset_data)
        if not preset:
            raise HTTPException(status_code=404, detail="Preset not found")
        return preset.model_dump()
    except ValueError as e:
        raise HTTPException(status_code=400, detail=str(e))


@router.delete("/prompts/{preset_id}")
async def delete_prompt(preset_id: str) -> dict:
    """Delete a custom prompt preset."""
    pm = settings_service.get_prompt_manager()
    try:
        success = pm.delete(preset_id)
        if not success:
            raise HTTPException(status_code=404, detail="Preset not found")
        return {"status": "deleted", "preset_id": preset_id}
    except ValueError as e:
        raise HTTPException(status_code=400, detail=str(e))


@router.post("/prompts/{preset_id}/preview")
async def preview_prompt(preset_id: str, request: PromptPreviewRequest) -> dict:
    """Preview prompt effect with test text."""
    pm = settings_service.get_prompt_manager()
    rendered = pm.render_prompt(preset_id, request.text)
    if rendered is None:
        raise HTTPException(status_code=404, detail="Preset not found")
    return {
        "preset_id": preset_id,
        "rendered_prompt": rendered
    }
