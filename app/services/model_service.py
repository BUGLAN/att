"""模型服务编排：model-service / streaming-service 的启停、状态与健康检查响应。

从 `app/api/routes.py` 下沉：

- `GET/POST /api/model-service*` 与 `GET/POST /api/streaming/service*`（兼容别名）
  统一由这里转发给 `app.services.model_service_manager`；
- `GET /api/health` 的 30 行状态拼装也收敛到这里（路由只负责返回结果）。

健康检查的字段集合是前端/Electron 探活契约的一部分，字段名与含义保持不变。
"""

from __future__ import annotations

from typing import Callable, Optional

from app.config import get_runtime_environment
from app.services.model_service_manager import model_service_manager
from app.services.websocket import manager as ws_manager
from app.utils.logging import get_logger

logger = get_logger(__name__)


async def get_status() -> dict:
    """模型服务当前状态。"""
    return await model_service_manager.status()


async def start(wait_until_ready: bool = True) -> dict:
    """启动托管模型服务。"""
    return await model_service_manager.start(wait_until_ready=wait_until_ready)


async def stop() -> dict:
    """停止托管模型服务。"""
    return await model_service_manager.stop()


async def build_health_payload(
    *,
    app_settings,
    user_settings,
    asr_provider,
    probe_runtime: bool,
    streaming_manager_getter: Callable[[], object],
) -> dict:
    """组装 GET /api/health 的响应体（字段与重构前完全一致）。"""
    configured_provider = user_settings.get("asr_provider", app_settings.asr_provider)

    if configured_provider == "qwen_asr":
        configured_model = user_settings.get("qwen_asr_model", app_settings.qwen_asr_model)
    elif configured_provider == "doubao_asr":
        # 在线 provider 没有可展示的本地模型名，固定回报服务端模型标识
        configured_model = "bigmodel"
    else:
        configured_model = user_settings.get("whisper_model", app_settings.whisper_model)

    streaming_available = False
    streaming_error: Optional[str] = None
    streaming_backend: Optional[str] = None
    service_status = await model_service_manager.status()

    if probe_runtime and configured_provider in ("qwen_asr", "doubao_asr"):
        try:
            provider = streaming_manager_getter()._get_provider()
            streaming_status = await provider.check_availability()
            streaming_available = bool(streaming_status.get("available", False))
            streaming_backend = streaming_status.get("backend")
        except Exception as error:
            streaming_error = str(error)
    elif configured_provider == "qwen_asr" and service_status.get("ready"):
        streaming_available = bool(service_status.get("streaming_available"))
        streaming_backend = app_settings.qwen_streaming_backend
        streaming_error = service_status.get("streaming_error")
    elif configured_provider == "doubao_asr":
        # 在线识别不需要「开启模型」，可用性直接来自 provider 的运行时能力
        streaming_available = bool(service_status.get("streaming_available"))
        streaming_backend = service_status.get("streaming_backend") or "doubao_sauc"
        streaming_error = service_status.get("streaming_error")

    return {
        "status": "healthy",
        "runtime_environment": get_runtime_environment(),
        "asr_provider": asr_provider.provider_type if asr_provider else configured_provider,
        "asr_model": asr_provider.model_name if asr_provider else configured_model,
        "deepseek_configured": bool(app_settings.deepseek_api_key),
        "ws_clients": ws_manager.client_count,
        "qwen_streaming_backend": streaming_backend,
        "qwen_streaming_available": streaming_available,
        "qwen_streaming_error": streaming_error,
        "qwen_streaming_service_managed": service_status.get("managed", False),
        "qwen_streaming_service_running": service_status.get("running", False),
        "qwen_streaming_service_loading": service_status.get("loading", False),
        "qwen_streaming_service_ready": service_status.get("ready", False),
        "qwen_streaming_service_pid": service_status.get("pid"),
        "qwen_streaming_service_url": service_status.get("base_url"),
        "qwen_streaming_service_error": service_status.get("error"),
        "model_service_managed": service_status.get("managed", False),
        "model_service_running": service_status.get("running", False),
        "model_service_loading": service_status.get("loading", False),
        "model_service_ready": service_status.get("ready", False),
        "model_service_error": service_status.get("error"),
        "model_service_supported": service_status.get("model_service_supported", True),
        "model_service_mode": service_status.get("model_service_mode"),
        "streaming_supported": service_status.get("streaming_supported", False),
        "streaming_backend": service_status.get("streaming_backend"),
        "streaming_available": service_status.get("streaming_available", False),
        "streaming_error": service_status.get("streaming_error"),
    }
