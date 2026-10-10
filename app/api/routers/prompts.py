"""提示词预设（prompts）路由。

从 `app/api/routes.py` 拆分而来，全部端点都是「调用 PromptManager + 组装响应」，
提示词管理器的读入口与预设归一化统一复用 `app/services/settings_service.py`
（与 GET/PUT /api/settings 的归一化口径保持一致）。

预设是只读的（唯一真相源是 prompts/*.txt）：这里只保留「列出」与「设默认」
两个端点；历史上的 create/update/delete（恒 400）与无调用方的
get/preview 已在功能减法中删除。
"""

from __future__ import annotations

from fastapi import APIRouter, Depends, HTTPException
from pydantic import BaseModel

from app.api.deps import runtime_state
from app.services import settings_service

router = APIRouter(prefix="/api", tags=["api", "prompts"])


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
