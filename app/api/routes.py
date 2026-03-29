"""
FastAPI REST API endpoints
"""
from fastapi import (
    APIRouter,
    UploadFile,
    File,
    HTTPException,
    Query,
    BackgroundTasks,
    WebSocket,
    WebSocketDisconnect
)
from fastapi.responses import PlainTextResponse, FileResponse
from pydantic import BaseModel
from typing import Optional, List
from pathlib import Path
from datetime import datetime
import json
import uuid

from app.config import (
    settings,
    user_settings,
    get_runtime_environment,
    is_valid_hotkey,
    normalize_clipboard_overlay_settings,
)
from app.audio.processor import AudioProcessor
from app.stt.base import ASRProvider
from app.llm.optimizer import TextOptimizer
from app.records.manager import RecordManager
from app.records.models import Record, RecordStatus, ExportFormat
from app.records.session_manager import session_manager
from app.services.model_service_manager import model_service_manager
from app.services.streaming_session_manager import StreamingSessionManager
from app.stt.factory import create_asr_provider, get_asr_provider_config
from app.services.websocket import manager as ws_manager
from app.prompts import PromptManager, PromptPresetCreate, PromptPresetUpdate, prompt_manager
from app.utils.logging import get_logger

logger = get_logger(__name__)

router = APIRouter(prefix="/api", tags=["api"])

# Global instances (initialized in main.py)
audio_processor: Optional[AudioProcessor] = None
asr_provider: Optional[ASRProvider] = None
optimizer: Optional[TextOptimizer] = None
record_manager: Optional[RecordManager] = None
streaming_session_manager: Optional[StreamingSessionManager] = None

# Request models
class RecordUpdateRequest(BaseModel):
    """Request model for updating a record"""
    raw_text: Optional[str] = None
    optimized_text: Optional[str] = None
    is_modified: Optional[bool] = None
    is_favorite: Optional[bool] = None


class PromptPreviewRequest(BaseModel):
    """Request model for prompt preview."""
    text: str


class SetDefaultPromptRequest(BaseModel):
    """Request model for updating the default prompt preset."""
    preset_id: str


class StreamingServiceRequest(BaseModel):
    """Request model for controlling the streaming model service."""
    wait_until_ready: bool = True


def get_prompt_manager() -> PromptManager:
    """Return the shared prompt manager."""
    prompt_manager.reload_from_files()
    return prompt_manager


def normalize_preset_settings(payload: dict, persist: bool = False) -> dict:
    """Normalize prompt settings against prompts/*.txt."""
    pm = get_prompt_manager()
    preset_ids = {preset.id for preset in pm.get_all()}
    fallback_default_id = pm.get_default_id()

    resolved_default_id = payload.get("default_preset_id")
    if resolved_default_id not in preset_ids:
        resolved_default_id = fallback_default_id

    resolved_last_used_id = payload.get("last_used_preset_id")
    if resolved_last_used_id not in preset_ids:
        resolved_last_used_id = resolved_default_id

    normalized = {
        **payload,
        "default_preset_id": resolved_default_id,
        "last_used_preset_id": resolved_last_used_id
    }

    if persist:
        updates = {}
        if payload.get("default_preset_id") != resolved_default_id:
            updates["default_preset_id"] = resolved_default_id
        if payload.get("last_used_preset_id") != resolved_last_used_id:
            updates["last_used_preset_id"] = resolved_last_used_id
        if updates:
            user_settings.update(updates)

    return normalized


def validate_hotkey(hotkey: dict) -> bool:
    """
    Validate hotkey configuration.

    Args:
        hotkey: Hotkey config dict with modifiers, key, display fields

    Returns:
        True if valid, False otherwise
    """
    return is_valid_hotkey(hotkey)


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


def build_completion_metadata(**data) -> dict:
    """Attach Electron/web activity metadata to completion payloads."""
    return {
        **data,
        "electron_active": ws_manager.get_active_web_client() is not None
    }


# ==================== Background Tasks ====================

async def _transcribe_audio_background(
    record_id: str,
    audio_path: str,
    asr_provider: ASRProvider,
    record_manager: RecordManager,
    optimizer: TextOptimizer,
    user_settings
) -> None:
    """
    Background task for audio transcription.

    Args:
        record_id: Record ID to update
        audio_path: Path to audio file
        asr_provider: ASR provider 实例（显式传入以避免全局引用问题）
        record_manager: Record manager instance
        optimizer: Text optimizer instance for auto-optimization
        user_settings: User settings instance
    """
    try:
        # Update status to transcribing
        await record_manager.update_record(
            record_id,
            status=RecordStatus.TRANSCRIBING
        )

        # Notify: transcription started
        await ws_manager.broadcast({
            "type": "transcription_started",
            "data": {"record_id": record_id},
            "timestamp": datetime.now().isoformat()
        })

        logger.info(
            "Background transcription started",
            record_id=record_id
        )

        # Execute transcription
        result = await asr_provider.transcribe(audio_path)

        # Update with result
        await record_manager.update_record(
            record_id,
            raw_text=result.text,
            status=RecordStatus.TRANSCRIBED,
            processing_time=result.processing_time
        )

        # Notify: transcription completed
        await ws_manager.broadcast({
            "type": "transcription_completed",
            "data": build_completion_metadata(
                record_id=record_id,
                text=result.text,
                duration=result.duration,
                processing_time=result.processing_time
            ),
            "timestamp": datetime.now().isoformat()
        })

        logger.info(
            "Background transcription complete",
            record_id=record_id,
            duration=result.duration,
            processing_time=f"{result.processing_time:.2f}s"
        )

        # Auto-optimization if enabled
        if user_settings.get("auto_optimize", False):
            try:
                logger.info("Auto-optimizing text", record_id=record_id)
                await record_manager.update_record(
                    record_id,
                    status=RecordStatus.OPTIMIZING
                )

                # Notify: optimization started
                await ws_manager.broadcast({
                    "type": "optimization_started",
                    "data": {"record_id": record_id},
                    "timestamp": datetime.now().isoformat()
                })

                # Get preset ID from settings
                preset_id = user_settings.get("default_preset_id")
                optimize_result = await optimizer.optimize(
                    result.text,
                    preset_id=preset_id
                )

                await record_manager.update_record(
                    record_id,
                    optimized_text=optimize_result.optimized_text,
                    status=RecordStatus.OPTIMIZED
                )

                # Notify: optimization completed
                await ws_manager.broadcast({
                    "type": "optimization_completed",
                    "data": build_completion_metadata(
                        record_id=record_id,
                        optimized_text=optimize_result.optimized_text
                    ),
                    "timestamp": datetime.now().isoformat()
                })

                logger.info("Auto-optimization complete", record_id=record_id)
            except Exception as e:
                logger.error("Auto-optimization failed", record_id=record_id, error=str(e))
                # Keep TRANSCRIBED status on optimization failure

    except Exception as e:
        logger.error(
            "Background transcription failed",
            record_id=record_id,
            error=str(e)
        )
        await record_manager.update_record(
            record_id,
            status=RecordStatus.FAILED
        )

        # Notify: error
        await ws_manager.broadcast({
            "type": "error",
            "data": {
                "record_id": record_id,
                "error": str(e)
            },
            "timestamp": datetime.now().isoformat()
        })


# ==================== Audio Upload Endpoint ====================

@router.post("/audio/upload", response_model=dict)
async def upload_audio(
    file: UploadFile = File(...),
    auto_transcribe: bool = True,
    background_tasks: BackgroundTasks = None
) -> dict:
    """
    Upload an audio file for processing

    Args:
        file: Audio file (wav, mp3, m4a, flac, ogg, webm)
        auto_transcribe: Automatically transcribe after upload (async)
        background_tasks: FastAPI background tasks

    Returns:
        Upload status with record_id (transcription happens in background)
    """
    # Validate file
    if not file.filename:
        raise HTTPException(status_code=400, detail="No filename provided")

    # Read file content
    content = await file.read()

    try:
        # Validate audio
        metadata = await audio_processor.validate_audio(content, file.filename)
    except ValueError as e:
        raise HTTPException(status_code=400, detail=str(e))

    # Generate record ID
    record_id = str(uuid.uuid4())

    # Convert and save as WAV
    audio_path, wav_metadata = await audio_processor.save_as_wav_for_record(
        content,
        file.filename,
        record_id,
        settings.audio_records_path
    )

    # Create record with PENDING status
    record = await record_manager.create_record(
        audio_path=str(audio_path),
        duration=wav_metadata.duration,
        status=RecordStatus.PENDING
    )

    logger.info(
        "Audio uploaded",
        record_id=record.id,
        duration=wav_metadata.duration
    )

    # Prepare response
    response_data = {
        "status": "uploaded",
        "record_id": record.id,
        "duration": wav_metadata.duration,
        "transcription_status": RecordStatus.PENDING.value
    }

    # Schedule background transcription
    if auto_transcribe and background_tasks:
        # Pass instances explicitly to avoid global variable issues
        background_tasks.add_task(
            _transcribe_audio_background,
            record.id,
            str(audio_path),
            asr_provider,
            record_manager,
            optimizer,
            user_settings
        )
        response_data["transcription_status"] = RecordStatus.TRANSCRIBING.value
        logger.info("Scheduled background transcription", record_id=record.id)

    return response_data


# ==================== Record Endpoints ====================

@router.get("/records", response_model=List[dict])
async def list_records(
    status: Optional[str] = None,
    limit: int = Query(default=100, le=1000)
) -> List[dict]:
    """List all records"""
    record_status = RecordStatus(status) if status else None
    records = await record_manager.list_records(
        status=record_status,
        limit=limit
    )
    return [r.to_dict() for r in records]


@router.get("/records/search", response_model=dict)
async def search_records(
    q: Optional[str] = None,
    search_type: str = Query(default="all", pattern="^(all|raw|optimized)$"),
    favorite_only: bool = False,
    page: int = Query(default=1, ge=1),
    page_size: int = Query(default=100, ge=1, le=100)
) -> dict:
    """Search records by text content and favorite state."""
    records = await record_manager.search_records(
        q=q,
        search_type=search_type,
        favorite_only=favorite_only,
        limit=5000
    )
    total = len(records)
    offset = (page - 1) * page_size
    items = records[offset:offset + page_size]
    total_pages = max((total + page_size - 1) // page_size, 1)

    return {
        "items": [r.to_dict() for r in items],
        "total": total,
        "page": page,
        "page_size": page_size,
        "total_pages": total_pages
    }


@router.get("/records/{record_id}", response_model=dict)
async def get_record(record_id: str) -> dict:
    """Get record details"""
    record = await record_manager.get_record(record_id)
    if not record:
        raise HTTPException(status_code=404, detail="Record not found")
    return record.to_dict()


@router.get("/records/{record_id}/status", response_model=dict)
async def get_record_status(record_id: str) -> dict:
    """
    Get record transcription status (for polling).

    Returns minimal data for efficient polling.
    """
    record = await record_manager.get_record(record_id)
    if not record:
        raise HTTPException(status_code=404, detail="Record not found")

    include_raw_text = record.status in {
        RecordStatus.TRANSCRIBED,
        RecordStatus.OPTIMIZING,
        RecordStatus.OPTIMIZED
    }
    include_optimized_text = record.status == RecordStatus.OPTIMIZED

    return {
        "record_id": record.id,
        "status": record.status.value,
        "raw_text": record.raw_text if include_raw_text else None,
        "optimized_text": record.optimized_text if include_optimized_text else None,
        "duration": record.duration,
        "processing_time": record.processing_time,
        "is_favorite": record.is_favorite
    }


@router.put("/records/{record_id}", response_model=dict)
async def update_record(
    record_id: str,
    request: RecordUpdateRequest
) -> dict:
    """Update record text (for editable input)"""
    record = await record_manager.update_record(
        record_id,
        raw_text=request.raw_text,
        optimized_text=request.optimized_text,
        is_modified=request.is_modified,
        is_favorite=request.is_favorite
    )
    if not record:
        raise HTTPException(status_code=404, detail="Record not found")
    return record.to_dict()


@router.delete("/records/{record_id}")
async def delete_record(record_id: str) -> dict:
    """Delete a record and its audio file"""
    success = await record_manager.delete_record(record_id)
    if not success:
        raise HTTPException(status_code=404, detail="Record not found")
    return {"status": "deleted", "record_id": record_id}


@router.get("/records/{record_id}/audio")
async def get_record_audio(record_id: str) -> FileResponse:
    """Serve audio file for playback"""
    record = await record_manager.get_record(record_id)
    if not record:
        raise HTTPException(status_code=404, detail="Record not found")

    audio_path = Path(record.audio_path)
    if not audio_path.exists():
        raise HTTPException(status_code=404, detail="Audio file not found")

    return FileResponse(
        str(audio_path),
        media_type="audio/wav"
    )


@router.post("/records/{record_id}/optimize")
async def optimize_record(
    record_id: str,
    style: str = "default"
) -> dict:
    """Optimize record text with AI"""
    record = await record_manager.get_record(record_id)
    if not record:
        raise HTTPException(status_code=404, detail="Record not found")

    if not record.raw_text:
        raise HTTPException(status_code=400, detail="No text to optimize")

    await record_manager.update_record(
        record_id,
        status=RecordStatus.OPTIMIZING
    )

    result = await optimizer.optimize(record.raw_text, style=style)

    await record_manager.update_record(
        record_id,
        optimized_text=result.optimized_text,
        status=RecordStatus.OPTIMIZED
    )

    return {
        "record_id": record_id,
        "original_text": record.raw_text,
        "optimized_text": result.optimized_text,
        "processing_time": result.processing_time
    }


@router.get("/records/{record_id}/export")
async def export_record(
    record_id: str,
    format: str = "txt",
    use_optimized: bool = True
) -> PlainTextResponse:
    """Export a single record"""
    record = await record_manager.get_record(record_id)
    if not record:
        raise HTTPException(status_code=404, detail="Record not found")

    try:
        export_format = ExportFormat(format)
    except ValueError:
        raise HTTPException(
            status_code=400,
            detail=f"Invalid format. Use: {[f.value for f in ExportFormat]}"
        )

    # Get text (prefer optimized)
    text = record.optimized_text if (use_optimized and record.optimized_text) else record.raw_text

    if export_format == ExportFormat.TXT:
        content = text
        media_type = "text/plain"
    else:  # JSON
        import json
        content = json.dumps(record.to_dict(), ensure_ascii=False, indent=2)
        media_type = "application/json"

    return PlainTextResponse(
        content=content,
        media_type=media_type,
        headers={
            "Content-Disposition": f"attachment; filename=record_{record_id}.{format}"
        }
    )


# ==================== Health Check ====================

@router.get("/health")
async def health_check(probe_runtime: bool = False) -> dict:
    """Health check endpoint"""
    configured_provider = user_settings.get("asr_provider", settings.asr_provider)
    configured_model = (
        user_settings.get("qwen_asr_model", settings.qwen_asr_model)
        if configured_provider == "qwen_asr"
        else user_settings.get("whisper_model", settings.whisper_model)
    )
    streaming_available = False
    streaming_error = None
    streaming_backend = None
    service_status = await model_service_manager.status()
    if probe_runtime and configured_provider == "qwen_asr":
        try:
            provider = get_streaming_session_manager()._get_provider()
            streaming_status = await provider.check_availability()
            streaming_available = bool(streaming_status.get("available", False))
            streaming_backend = streaming_status.get("backend")
        except Exception as error:
            streaming_error = str(error)
    elif configured_provider == "qwen_asr" and service_status.get("ready"):
        streaming_available = bool(service_status.get("streaming_available"))
        streaming_backend = settings.qwen_streaming_backend
        streaming_error = service_status.get("streaming_error")

    return {
        "status": "healthy",
        "runtime_environment": get_runtime_environment(),
        "asr_provider": asr_provider.provider_type if asr_provider else configured_provider,
        "asr_model": asr_provider.model_name if asr_provider else configured_model,
        "deepseek_configured": bool(settings.deepseek_api_key),
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


# ==================== Streaming Service Endpoints ====================

@router.get("/model-service")
async def get_model_service_status() -> dict:
    """Get the managed model service status."""
    return await model_service_manager.status()


@router.post("/model-service/start")
async def start_model_service(request: StreamingServiceRequest) -> dict:
    """Start the managed model service."""
    try:
        return await model_service_manager.start(
            wait_until_ready=request.wait_until_ready
        )
    except Exception as error:
        raise HTTPException(status_code=500, detail=str(error)) from error


@router.post("/model-service/stop")
async def stop_model_service() -> dict:
    """Stop the managed model service."""
    try:
        return await model_service_manager.stop()
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
async def get_settings() -> dict:
    """Get user settings"""
    payload = normalize_preset_settings(user_settings.get_all(), persist=True)
    normalized_payload = normalize_clipboard_overlay_settings(payload)

    overlay_keys = {
        "clipboard_overlay_trigger",
        "clipboard_overlay_retrigger_action",
        "clipboard_overlay_enter_behavior",
        "clipboard_overlay_auto_focus_search",
        "clipboard_overlay_close_on_click_copy",
    }
    overlay_updates = {
        key: normalized_payload[key]
        for key in overlay_keys
        if payload.get(key) != normalized_payload.get(key)
    }
    if overlay_updates:
        user_settings.update(overlay_updates)

    return normalized_payload


@router.put("/settings")
async def update_settings(updates: dict) -> dict:
    """Update user settings"""
    global asr_provider

    # Filter to only allowed keys
    allowed_keys = {
        "asr_provider", "asr_language",
        "whisper_model", "whisper_device",
        "qwen_asr_model", "qwen_asr_device", "qwen_asr_dtype",
        "qwen_asr_max_new_tokens", "qwen_asr_return_timestamps",
        "qwen_asr_use_forced_aligner", "qwen_asr_forced_aligner_model",
        "qwen_asr_max_audio_seconds",
        "pause_audio_on_recording", "auto_optimize", "auto_copy_to_clipboard",
        "hotkey",
        "clipboard_overlay_trigger", "clipboard_overlay_retrigger_action",
        "clipboard_overlay_enter_behavior", "clipboard_overlay_auto_focus_search",
        "clipboard_overlay_close_on_click_copy",
        # Recording mode settings
        "recording_mode", "silence_threshold", "silence_level",
        # Prompt preset settings
        "default_preset_id", "last_used_preset_id",
        # Audio input device
        "audio_input_device"
    }
    filtered = {k: v for k, v in updates.items() if k in allowed_keys}

    # Validate hotkey if provided
    if "hotkey" in filtered and not validate_hotkey(filtered["hotkey"]):
        raise HTTPException(status_code=400, detail="Invalid hotkey configuration")

    if "asr_provider" in filtered and filtered["asr_provider"] not in {"whisper", "qwen_asr"}:
        raise HTTPException(status_code=400, detail="Invalid ASR provider")

    if not filtered:
        raise HTTPException(status_code=400, detail="No valid settings provided")

    merged_settings = {**user_settings.get_all(), **filtered}
    normalized_settings = normalize_clipboard_overlay_settings(normalize_preset_settings(merged_settings))
    filtered = {key: normalized_settings[key] for key in filtered.keys()}
    previous_settings = user_settings.get_all()
    user_settings.update(filtered)

    model_runtime_keys = {
        "asr_provider",
        "whisper_model",
        "whisper_device",
        "qwen_asr_model",
        "qwen_asr_device",
        "qwen_asr_dtype",
        "qwen_asr_return_timestamps",
        "qwen_asr_use_forced_aligner",
        "qwen_asr_forced_aligner_model",
        "recording_mode",
    }
    provider_rebuild_keys = model_runtime_keys - {"recording_mode"}

    changed_keys = {
        key for key, value in filtered.items()
        if previous_settings.get(key) != value
    }
    should_reset_model_runtime = bool(changed_keys & model_runtime_keys)
    should_rebuild_provider = bool(changed_keys & provider_rebuild_keys)

    if should_reset_model_runtime:
        logger.info(
            "检测到模型运行时配置变更，准备卸载当前模型服务",
            changed_keys=sorted(changed_keys & model_runtime_keys),
        )
        await model_service_manager.stop()
        await close_streaming_session_manager()

    if should_rebuild_provider:
        if asr_provider is None:
            raise HTTPException(status_code=500, detail="ASR provider is not initialized")

        try:
            await asr_provider.close()
        except Exception as error:
            logger.warning("关闭旧 ASR provider 失败", error=str(error), exc_info=True)

        try:
            asr_provider = create_asr_provider(get_asr_provider_config())
            model_service_manager.configure(asr_provider)
        except Exception as error:
            logger.error("重建 ASR provider 失败", error=str(error), exc_info=True)
            raise HTTPException(
                status_code=500,
                detail=f"重新加载 ASR provider 失败: {error}"
            ) from error

    payload = normalize_preset_settings(user_settings.get_all())
    return normalize_clipboard_overlay_settings(payload)


# ==================== WebSocket Endpoints ====================

@router.websocket("/ws/{client_type}")
async def websocket_endpoint(websocket: WebSocket, client_type: str):
    """
    WebSocket endpoint for real-time communication.

    Args:
        websocket: WebSocket connection
        client_type: Client type ("web" or "tray")
    """
    client_id = str(uuid.uuid4())

    await ws_manager.connect(websocket, client_id, client_type)

    try:
        while True:
            # 接收消息
            data = await websocket.receive_json()

            # 处理消息
            await _handle_websocket_message(client_id, data)

    except WebSocketDisconnect:
        ws_manager.disconnect(client_id)
        await ws_manager.broadcast_status()

    except Exception as e:
        logger.error(
            "WebSocket error",
            client_id=client_id,
            error=str(e)
        )
        ws_manager.disconnect(client_id)
        await ws_manager.broadcast_status()


@router.websocket("/asr/stream")
async def streaming_asr_websocket(websocket: WebSocket):
    """Qwen 真流式 ASR WebSocket。"""
    await websocket.accept()
    stream_session_id = None

    try:
        manager = get_streaming_session_manager()
        while True:
            message = await websocket.receive()
            message_type = message.get("type")

            if message_type == "websocket.disconnect":
                break

            if message.get("text") is not None:
                data = json.loads(message["text"])
                command = data.get("type")

                if command == "stream_start":
                    session = await manager.create_session(
                        language=data.get("language", "zh"),
                        context=data.get("context", "")
                    )
                    stream_session_id = session["session_id"]
                    await websocket.send_json({
                        "type": "stream_ready",
                        "data": session,
                        "timestamp": datetime.now().isoformat()
                    })
                    continue

                if command == "stream_finish":
                    if not stream_session_id:
                        raise ValueError("流式会话尚未启动")

                    result = await manager.finish_session(stream_session_id)
                    stream_session_id = None
                    await websocket.send_json({
                        "type": "stream_finished",
                        "data": result,
                        "timestamp": datetime.now().isoformat()
                    })
                    continue

                if command == "stream_cancel":
                    if stream_session_id:
                        await manager.cancel_session(stream_session_id)
                        stream_session_id = None
                    await websocket.send_json({
                        "type": "stream_cancelled",
                        "timestamp": datetime.now().isoformat()
                    })
                    continue

                if command == "ping":
                    await websocket.send_json({"type": "pong"})
                    continue

                raise ValueError(f"未知的流式命令: {command}")

            if message.get("bytes") is not None:
                if not stream_session_id:
                    raise ValueError("流式会话尚未启动")

                update = await manager.append_audio(stream_session_id, message["bytes"])
                await websocket.send_json({
                    "type": "stream_partial",
                    "data": update,
                    "timestamp": datetime.now().isoformat()
                })

    except WebSocketDisconnect:
        if stream_session_id:
            await manager.cancel_session(stream_session_id)
    except Exception as error:
        logger.error("Streaming ASR WebSocket error", error=str(error), session_id=stream_session_id)
        if stream_session_id:
            await manager.cancel_session(stream_session_id)
        await websocket.send_json({
            "type": "stream_error",
            "data": {"error": str(error)},
            "timestamp": datetime.now().isoformat()
        })
        await websocket.close()


async def _handle_websocket_message(client_id: str, data: dict) -> None:
    """
    Handle incoming WebSocket messages.

    Args:
        client_id: Sender client ID
        data: Message data
    """
    message_type = data.get("type")

    if message_type == "recording":
        # 录音控制命令（来自托盘工具）
        action = data.get("action")  # "start" | "stop"
        logger.info(
            "Received recording command",
            client_id=client_id,
            action=action
        )

        # 广播给其他客户端（web 前端）
        await ws_manager.broadcast({
            "type": "recording_command",
            "action": action,
            "source": client_id,
            "timestamp": int(datetime.now().timestamp() * 1000)
        }, exclude=client_id)

    elif message_type == "status_update":
        # 状态更新（来自 web 客户端）
        await ws_manager.update_status(
            recording=data.get("recording", False),
            transcribing=data.get("transcribing", False),
            record_id=data.get("record_id"),
            duration=data.get("duration", 0.0)
        )

    elif message_type == "web_recording":
        # 网页录音通知（来自 web 客户端）
        action = data.get("action")  # "start" | "stop"
        logger.info(
            "Received web_recording notification",
            client_id=client_id,
            action=action
        )

        # 广播给托盘工具，让其执行暂停/恢复音频
        await ws_manager.broadcast({
            "type": "web_recording_command",
            "action": action,
            "source": client_id,
            "timestamp": int(datetime.now().timestamp() * 1000)
        }, client_type_filter="tray")

    elif message_type == "settings_update":
        # 设置更新通知（来自 web 客户端）
        pause_audio = data.get("pause_audio_on_recording")
        logger.info(
            "Received settings_update",
            client_id=client_id,
            pause_audio_on_recording=pause_audio
        )

        # 广播给托盘工具，让其更新设置
        await ws_manager.broadcast({
            "type": "settings_changed",
            "pause_audio_on_recording": pause_audio,
            "timestamp": int(datetime.now().timestamp() * 1000)
        }, client_type_filter="tray")

    elif message_type == "ping":
        # 心跳
        await ws_manager.send_to_client(client_id, {"type": "pong"})

    else:
        logger.warning(
            "Unknown WebSocket message type",
            client_id=client_id,
            message_type=message_type
        )


# ==================== Prompt Preset Endpoints ====================

@router.get("/prompts")
async def list_prompts() -> dict:
    """Get all prompt presets."""
    pm = get_prompt_manager()
    normalized_settings = normalize_preset_settings(user_settings.get_all(), persist=True)
    presets = pm.get_all()
    return {
        "presets": [p.model_dump() for p in presets],
        "default_preset_id": normalized_settings.get("default_preset_id"),
        "last_used_preset_id": normalized_settings.get("default_preset_id")
    }


@router.put("/prompts/default")
async def set_default_prompt(request: SetDefaultPromptRequest) -> dict:
    """Set the default prompt preset."""
    pm = get_prompt_manager()
    if not pm.set_default(request.preset_id):
        raise HTTPException(status_code=404, detail="Preset not found")

    user_settings.update({
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
    pm = get_prompt_manager()
    preset = pm.get_by_id(preset_id)
    if not preset:
        raise HTTPException(status_code=404, detail="Preset not found")
    return preset.model_dump()


@router.post("/prompts")
async def create_prompt(preset_data: PromptPresetCreate) -> dict:
    """Create a new custom prompt preset."""
    pm = get_prompt_manager()
    try:
        preset = pm.create(preset_data)
        return preset.model_dump()
    except Exception as e:
        raise HTTPException(status_code=400, detail=str(e))


@router.put("/prompts/{preset_id}")
async def update_prompt(preset_id: str, preset_data: PromptPresetUpdate) -> dict:
    """Update an existing custom preset."""
    pm = get_prompt_manager()
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
    pm = get_prompt_manager()
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
    pm = get_prompt_manager()
    rendered = pm.render_prompt(preset_id, request.text)
    if rendered is None:
        raise HTTPException(status_code=404, detail="Preset not found")
    return {
        "preset_id": preset_id,
        "rendered_prompt": rendered
    }


# ==================== Recording Session Endpoints ====================

@router.post("/sessions")
async def create_session() -> dict:
    """Create a new multi-segment recording session."""
    session = await session_manager.create_session()
    return {
        "session_id": session.id,
        "status": session.status,
        "created_at": session.created_at.isoformat()
    }


@router.get("/sessions/{session_id}")
async def get_session(session_id: str) -> dict:
    """Get session details."""
    session = await session_manager.get_session(session_id)
    if not session:
        raise HTTPException(status_code=404, detail="Session not found")
    return session.to_dict()


@router.post("/sessions/{session_id}/segments")
async def upload_segment(
    session_id: str,
    file: UploadFile = File(...),
    background_tasks: BackgroundTasks = None
) -> dict:
    """Upload a segment to the session."""
    # Verify session exists
    session = await session_manager.get_session(session_id)
    if not session:
        raise HTTPException(status_code=404, detail="Session not found")

    # Read and validate audio
    content = await file.read()
    try:
        metadata = await audio_processor.validate_audio(content, file.filename)
    except ValueError as e:
        raise HTTPException(status_code=400, detail=str(e))

    # Save segment audio
    segment_id = str(uuid.uuid4())
    audio_path, wav_metadata = await audio_processor.save_as_wav_for_record(
        content,
        f"segment_{segment_id}.webm",
        segment_id,
        settings.audio_records_path
    )

    # Add segment to session
    segment = await session_manager.add_segment(
        session_id,
        str(audio_path),
        wav_metadata.duration
    )

    # Schedule background transcription
    if background_tasks and asr_provider:
        background_tasks.add_task(
            _transcribe_segment_background,
            session_id,
            segment.id,
            str(audio_path)
        )

    return {
        "segment_id": segment.id,
        "sequence": segment.sequence,
        "duration": wav_metadata.duration,
        "status": segment.status.value
    }


@router.post("/sessions/{session_id}/merge")
async def merge_session(session_id: str) -> dict:
    """Merge all segments into a single record."""
    session = await session_manager.complete_session(session_id)
    if not session:
        raise HTTPException(status_code=404, detail="Session not found")

    merged_text = session.merged_text
    processing_time = 0.0

    if session.merged_audio_path and asr_provider:
        try:
            result = await asr_provider.transcribe(
                session.merged_audio_path,
                vad_filter=False
            )
            merged_text = result.text or merged_text
            processing_time = result.processing_time
        except Exception as error:
            logger.warning(
                "多段录音整段转写失败，回退到分段拼接文本",
                session_id=session_id,
                error=str(error)
            )

    # Create a record from merged text
    record = await record_manager.create_record(
        audio_path=session.merged_audio_path,
        duration=session.total_duration,
        status=RecordStatus.TRANSCRIBED,
        raw_text=merged_text,
        processing_time=processing_time
    )

    optimized_text = None
    if user_settings.get("auto_optimize", False) and merged_text:
        try:
            await record_manager.update_record(
                record.id,
                status=RecordStatus.OPTIMIZING
            )
            optimize_result = await optimizer.optimize(
                merged_text,
                preset_id=user_settings.get("default_preset_id")
            )
            updated_record = await record_manager.update_record(
                record.id,
                optimized_text=optimize_result.optimized_text,
                status=RecordStatus.OPTIMIZED
            )
            if updated_record:
                record = updated_record
                optimized_text = optimize_result.optimized_text
        except Exception as error:
            await record_manager.update_record(
                record.id,
                status=RecordStatus.TRANSCRIBED
            )
            logger.warning(
                "多段录音自动优化失败",
                session_id=session_id,
                record_id=record.id,
                error=str(error)
            )

    return {
        "session_id": session_id,
        "record_id": record.id,
        "merged_text": merged_text,
        "optimized_text": optimized_text,
        "record": record.to_dict(),
        "total_duration": session.total_duration,
        "segment_count": len(session.segments)
    }


@router.delete("/sessions/{session_id}")
async def delete_session(session_id: str) -> dict:
    """Delete a session and its segments."""
    success = await session_manager.delete_session(session_id)
    if not success:
        raise HTTPException(status_code=404, detail="Session not found")
    return {"status": "deleted", "session_id": session_id}


async def _transcribe_segment_background(
    session_id: str,
    segment_id: str,
    audio_path: str
) -> None:
    """Background task for segment transcription."""
    try:
        session = await session_manager.get_session(session_id)
        sequence = None
        if session:
            for segment in session.segments:
                if segment.id == segment_id:
                    sequence = segment.sequence
                    break

        # Update segment status
        await session_manager.update_segment(
            session_id,
            segment_id,
            status=RecordStatus.TRANSCRIBING
        )

        # Execute transcription
        result = await asr_provider.transcribe(audio_path, vad_filter=False)

        # Update segment with result
        await session_manager.update_segment(
            session_id,
            segment_id,
            raw_text=result.text,
            status=RecordStatus.TRANSCRIBED,
            processing_time=result.processing_time
        )

        # Notify via WebSocket
        await ws_manager.broadcast({
            "type": "segment_transcribed",
            "data": {
                "session_id": session_id,
                "segment_id": segment_id,
                "sequence": sequence,
                "text": result.text,
                "duration": result.duration
            },
            "timestamp": datetime.now().isoformat()
        })

        logger.info(
            "Segment transcribed",
            session_id=session_id,
            segment_id=segment_id,
            duration=result.duration
        )

    except Exception as e:
        logger.error(
            "Segment transcription failed",
            session_id=session_id,
            segment_id=segment_id,
            error=str(e)
        )
        await session_manager.update_segment(
            session_id,
            segment_id,
            status=RecordStatus.FAILED
        )
        await ws_manager.broadcast({
            "type": "segment_failed",
            "data": {
                "session_id": session_id,
                "segment_id": segment_id,
                "error": str(e)
            },
            "timestamp": datetime.now().isoformat()
        })


# ==================== LLM Configuration Endpoints ====================

from app.llm.models import (
    LLMProvider, LLMConfigCreate, LLMConfigUpdate,
    PROVIDER_INFO, get_provider_models
)
from app.llm.config_manager import get_llm_config_manager
from app.llm.gateway import get_gateway

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
