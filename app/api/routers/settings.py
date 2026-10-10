"""用户设置 + 健康检查 + 模型服务路由（settings + model-service）。

从 `app/api/routes.py` 拆分而来：

- 设置归一化/校验/provider 重建的编排在 `app/services/settings_service.py`；
- health 与 model-service/streaming-service 的状态拼装在 `app/services/model_service.py`；
- 本模块只做「解析请求 → 调 service → 组装响应」。

兼容别名 `GET/POST /api/streaming/service*` 与 `GET/POST /api/model-service*` 行为一致。
"""

from __future__ import annotations

from fastapi import APIRouter, Depends, HTTPException
from pydantic import BaseModel

from app.api.deps import runtime_state
from app.services import model_service, settings_service
from app.services.model_service_manager import model_service_manager
from app.utils.logging import get_logger

logger = get_logger(__name__)

router = APIRouter(prefix="/api", tags=["api", "settings", "model-service"])


# ==================== Model Service Request Model ====================

class StreamingServiceRequest(BaseModel):
    """Request model for controlling the streaming model service."""
    wait_until_ready: bool = True


# ==================== Health Check ====================

@router.get("/health")
async def health_check(probe_runtime: bool = False, state=Depends(runtime_state)) -> dict:
    """Health check endpoint（字段集合为探活契约，保持不变）。"""
    return await model_service.build_health_payload(
        app_settings=state.settings,
        user_settings=state.user_settings,
        asr_provider=state.asr_provider,
        probe_runtime=probe_runtime,
        streaming_manager_getter=state.get_streaming_session_manager,
    )


# ==================== Streaming Service Endpoints ====================

@router.get("/model-service")
async def get_model_service_status() -> dict:
    """Get the managed model service status."""
    return await model_service.get_status()


@router.post("/model-service/start")
async def start_model_service(request: StreamingServiceRequest) -> dict:
    """Start the managed model service."""
    try:
        return await model_service.start(wait_until_ready=request.wait_until_ready)
    except Exception as error:
        raise HTTPException(status_code=500, detail=str(error)) from error


@router.post("/model-service/stop")
async def stop_model_service() -> dict:
    """Stop the managed model service."""
    try:
        return await model_service.stop()
    except Exception as error:
        raise HTTPException(status_code=500, detail=str(error)) from error


@router.get("/streaming/service")
async def get_streaming_service_status() -> dict:
    """Compatibility alias for the old streaming service status endpoint."""
    return await get_model_service_status()


@router.post("/streaming/service/start")
async def start_streaming_service(request: StreamingServiceRequest) -> dict:
    """Compatibility alias for the old streaming service start endpoint."""
    return await start_model_service(request)


@router.post("/streaming/service/stop")
async def stop_streaming_service() -> dict:
    """Compatibility alias for the old streaming service stop endpoint."""
    return await stop_model_service()


# ==================== Settings Endpoints ====================

@router.get("/settings")
async def get_settings(state=Depends(runtime_state)) -> dict:
    """Get user settings（默认纯读：只有必须初始化的键才会单次落盘）。"""
    return settings_service.build_settings_response(user_settings=state.user_settings)


@router.put("/settings")
async def update_settings(updates: dict, state=Depends(runtime_state)) -> dict:
    """Update user settings（类型校验 + 未知键静默丢弃）。"""
    try:
        result = await settings_service.apply_settings_update(
            updates=updates,
            user_settings=state.user_settings,
            asr_provider=state.asr_provider,
            model_service_manager=model_service_manager,
            close_streaming_manager=state.close_streaming_session_manager,
        )
    except settings_service.SettingsValidationError as error:
        raise HTTPException(status_code=error.status_code, detail=error.message) from error

    if result.provider_rebuilt:
        # 重建后的 provider 必须写回运行时状态宿主（app.api.routes.asr_provider）
        state.asr_provider = result.asr_provider

    return result.payload
