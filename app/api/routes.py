"""API 聚合层：运行时状态宿主 + 子路由汇总。

拆分说明（task-9 第 2 项）：

- 原本 1600+ 行 / 42 端点的单文件路由已按资源拆到 `app/api/routers/`：
  records（音频+记录）、sessions（会话+WebSocket）、settings（设置+健康检查+模型服务）、
  prompts、llm；每个子路由自带 ``prefix="/api"``；
- 本模块只保留两件职责：
  1. **运行时状态宿主**：`audio_processor` / `asr_provider` / `optimizer` /
     `record_manager` / `streaming_session_manager` / `settings` / `user_settings`
     仍然定义在这里。`app.main` 通过 `init_processors` 注入，既有测试通过
     ``monkeypatch.setattr(app.api.routes, "record_manager", ...)`` 替换，
     子路由每次请求都从本模块读取（见 `app/api/deps.py`），因此这些语义不变；
  2. **公开符号再导出**：`router`、`init_processors`、`close_streaming_session_manager`、
     `asr_provider`、`get_prompt_manager`、`normalize_preset_settings` 等既有名字
     继续可用，避免其他模块 import 失败。
"""

from __future__ import annotations

from typing import Optional

from fastapi import APIRouter

from app.api.routers.llm import router as llm_router
from app.api.routers.prompts import router as prompts_router
from app.api.routers.records import router as records_router
from app.api.routers.sessions import router as sessions_router
from app.api.routers.settings import router as settings_router
from app.api.schemas import UserSettingsUpdate, format_validation_error
from app.audio.processor import AudioProcessor
from app.config import (
    get_runtime_environment,
    is_valid_hotkey,
    settings,
    user_settings,
)
from app.llm.optimizer import TextOptimizer
from app.prompts import PromptManager, PromptPresetCreate, PromptPresetUpdate, prompt_manager
from app.records.manager import RecordManager
from app.records.models import ExportFormat, Record, RecordStatus
from app.records.session_manager import session_manager
from app.services.model_service_manager import model_service_manager
from app.services.streaming_session_manager import StreamingSessionManager
from app.stt.base import ASRProvider
from app.stt.factory import create_asr_provider, get_asr_provider_config
from app.utils.logging import get_logger

logger = get_logger(__name__)


# ==================== 子路由汇总 ====================

router = APIRouter()
router.include_router(records_router)
router.include_router(sessions_router)
router.include_router(settings_router)
router.include_router(prompts_router)
router.include_router(llm_router)


# ==================== 运行时状态（被 app.main 注入 / 被测试替换） ====================

audio_processor: Optional[AudioProcessor] = None
asr_provider: Optional[ASRProvider] = None
optimizer: Optional[TextOptimizer] = None
record_manager: Optional[RecordManager] = None
streaming_session_manager: Optional[StreamingSessionManager] = None


def init_processors(
    audio: AudioProcessor,
    stt: ASRProvider,
    llm: TextOptimizer,
    records: RecordManager
) -> None:
    """Initialize global processor instances"""
    global audio_processor, asr_provider, optimizer, record_manager
    audio_processor = audio
    asr_provider = stt
    optimizer = llm
    record_manager = records


def get_streaming_session_manager() -> StreamingSessionManager:
    """懒加载流式会话管理器。"""
    global streaming_session_manager

    if streaming_session_manager is None:
        if record_manager is None or optimizer is None:
            raise RuntimeError("Streaming session manager is not ready")
        streaming_session_manager = StreamingSessionManager(record_manager, optimizer)

    return streaming_session_manager


async def close_streaming_session_manager() -> None:
    """关闭流式会话管理器。"""
    global streaming_session_manager

    if streaming_session_manager is not None:
        await streaming_session_manager.close()
        streaming_session_manager = None


# ==================== 既有公开符号再导出（保持向后兼容） ====================

# 请求模型（原 routes.py 顶层定义，现在随子路由迁移）
from app.api.routers.llm import get_llm_manager  # noqa: E402
from app.api.routers.prompts import (  # noqa: E402
    PromptPreviewRequest,
    SetDefaultPromptRequest,
)
from app.api.routers.records import RecordUpdateRequest  # noqa: E402
from app.api.routers.sessions import _handle_websocket_message  # noqa: E402
from app.api.routers.settings import StreamingServiceRequest  # noqa: E402
from app.api.uploads import (  # noqa: E402
    UPLOAD_CHUNK_SIZE,
    describe_size_limit,
    read_upload_within_limit,
)
from app.services.settings_service import (  # noqa: E402
    apply_settings_update,
    build_settings_response,
    get_prompt_manager,
    normalize_preset_settings,
    validate_hotkey,
)
from app.services.transcription_service import (  # noqa: E402
    build_completion_metadata,
    transcribe_audio_background,
    transcribe_segment_background,
)

# 兼容别名：原 routes.py 中的私有名字继续可用
_describe_size_limit = describe_size_limit
_read_upload_within_limit = read_upload_within_limit
_transcribe_audio_background = transcribe_audio_background
_transcribe_segment_background = transcribe_segment_background

__all__ = [
    "router",
    "init_processors",
    "get_streaming_session_manager",
    "close_streaming_session_manager",
    "audio_processor",
    "asr_provider",
    "optimizer",
    "record_manager",
    "streaming_session_manager",
    "settings",
    "user_settings",
    "RecordUpdateRequest",
    "PromptPreviewRequest",
    "SetDefaultPromptRequest",
    "StreamingServiceRequest",
    "get_prompt_manager",
    "normalize_preset_settings",
    "validate_hotkey",
    "build_settings_response",
    "apply_settings_update",
    "build_completion_metadata",
    "get_llm_manager",
    "UPLOAD_CHUNK_SIZE",
    # 兼容别名（原私有名字）
    "_handle_websocket_message",
    "_describe_size_limit",
    "_read_upload_within_limit",
    "_transcribe_audio_background",
    "_transcribe_segment_background",
    "model_service_manager",
    "create_asr_provider",
    "get_asr_provider_config",
    "session_manager",
    "prompt_manager",
    "PromptManager",
    "PromptPresetCreate",
    "PromptPresetUpdate",
    "Record",
    "RecordStatus",
    "ExportFormat",
    "UserSettingsUpdate",
    "format_validation_error",
    "get_runtime_environment",
    "is_valid_hotkey",
]
