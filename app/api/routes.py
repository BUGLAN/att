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
import uuid

from app.config import settings, user_settings, get_default_hotkey
from app.audio.processor import AudioProcessor
from app.stt.transcriber import WhisperTranscriber
from app.llm.optimizer import TextOptimizer
from app.records.manager import RecordManager
from app.records.models import Record, RecordStatus, ExportFormat
from app.records.session_manager import session_manager
from app.services.websocket import manager as ws_manager
from app.prompts import PromptManager, PromptPresetCreate, PromptPresetUpdate
from app.utils.logging import get_logger

logger = get_logger(__name__)

router = APIRouter(prefix="/api", tags=["api"])

# Global instances (initialized in main.py)
audio_processor: Optional[AudioProcessor] = None
transcriber: Optional[WhisperTranscriber] = None
optimizer: Optional[TextOptimizer] = None
record_manager: Optional[RecordManager] = None

# Prompt preset manager (lazy initialization)
_prompt_manager: Optional[PromptManager] = None


# Request models
class RecordUpdateRequest(BaseModel):
    """Request model for updating a record"""
    raw_text: Optional[str] = None
    optimized_text: Optional[str] = None
    is_modified: Optional[bool] = None


def get_prompt_manager() -> PromptManager:
    """Get or create prompt manager instance."""
    global _prompt_manager
    if _prompt_manager is None:
        _prompt_manager = PromptManager()
    return _prompt_manager


def validate_hotkey(hotkey: dict) -> bool:
    """
    Validate hotkey configuration.

    Args:
        hotkey: Hotkey config dict with modifiers, key, display fields

    Returns:
        True if valid, False otherwise
    """
    if not isinstance(hotkey, dict):
        return False

    modifiers = hotkey.get("modifiers", [])
    key = hotkey.get("key")

    # Validate modifiers
    valid_modifiers = {"cmd", "alt", "ctrl", "shift", "win"}
    if not isinstance(modifiers, list) or not all(m in valid_modifiers for m in modifiers):
        return False

    # Must have at least one modifier
    if len(modifiers) < 1:
        return False

    # If key is present, validate it
    if key is not None:
        if not isinstance(key, str) or len(key) != 1:
            return False
        valid_keys = set("abcdefghijklmnopqrstuvwxyz0123456789")
        if key.lower() not in valid_keys:
            return False

    # For modifier-only combinations: need at least 2 modifiers
    if key is None and len(modifiers) < 2:
        return False

    return True


def init_processors(
    audio: AudioProcessor,
    stt: WhisperTranscriber,
    llm: TextOptimizer,
    records: RecordManager
) -> None:
    """Initialize global processor instances"""
    global audio_processor, transcriber, optimizer, record_manager
    audio_processor = audio
    transcriber = stt
    optimizer = llm
    record_manager = records


# ==================== Background Tasks ====================

async def _transcribe_audio_background(
    record_id: str,
    audio_path: str,
    transcriber: WhisperTranscriber,
    record_manager: RecordManager,
    optimizer: TextOptimizer,
    user_settings
) -> None:
    """
    Background task for audio transcription.

    Args:
        record_id: Record ID to update
        audio_path: Path to audio file
        transcriber: Transcriber instance (passed explicitly to avoid global issues)
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
        result = await transcriber.transcribe(audio_path)

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
            "data": {
                "record_id": record_id,
                "text": result.text,
                "duration": result.duration,
                "processing_time": result.processing_time
            },
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
                preset_id = user_settings.get("last_used_preset_id") or \
                            user_settings.get("default_preset_id")
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
                    "data": {
                        "record_id": record_id,
                        "optimized_text": optimize_result.optimized_text
                    },
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
            transcriber,
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

    # Return raw_text for completed states (transcribed, optimizing, optimized)
    completed_statuses = [
        RecordStatus.TRANSCRIBED,
        RecordStatus.OPTIMIZING,
        RecordStatus.OPTIMIZED
    ]

    return {
        "record_id": record.id,
        "status": record.status.value,
        "raw_text": record.raw_text if record.status in completed_statuses else None,
        "duration": record.duration,
        "processing_time": record.processing_time
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
        is_modified=request.is_modified
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
async def health_check() -> dict:
    """Health check endpoint"""
    return {
        "status": "healthy",
        "whisper_model": settings.whisper_model,
        "deepseek_configured": bool(settings.deepseek_api_key),
        "ws_clients": ws_manager.client_count
    }


# ==================== Settings Endpoints ====================

@router.get("/settings")
async def get_settings() -> dict:
    """Get user settings"""
    return user_settings.get_all()


@router.put("/settings")
async def update_settings(updates: dict) -> dict:
    """Update user settings"""
    # Filter to only allowed keys
    allowed_keys = {
        "whisper_model", "whisper_device",
        "pause_audio_on_recording", "auto_optimize", "auto_copy_to_clipboard",
        "hotkey",
        # Recording mode settings
        "recording_mode", "silence_threshold", "silence_level",
        # Prompt preset settings
        "default_preset_id", "last_used_preset_id"
    }
    filtered = {k: v for k, v in updates.items() if k in allowed_keys}

    # Validate hotkey if provided
    if "hotkey" in filtered and not validate_hotkey(filtered["hotkey"]):
        raise HTTPException(status_code=400, detail="Invalid hotkey configuration")

    if not filtered:
        raise HTTPException(status_code=400, detail="No valid settings provided")

    user_settings.update(filtered)
    return user_settings.get_all()


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

    elif message_type == "hotkey_update":
        # 快捷键配置更新（来自 web 客户端）
        hotkey_config = data.get("hotkey")
        client_type = ws_manager.get_client_type(client_id)

        logger.info(
            "Received hotkey_update",
            client_id=client_id,
            client_type=client_type,
            hotkey=hotkey_config
        )

        if validate_hotkey(hotkey_config):
            # 保存设置
            user_settings.set("hotkey", hotkey_config)

            # 广播给 tray 客户端
            tray_count = len(ws_manager.get_tray_clients())
            logger.info(f"Broadcasting to {tray_count} tray clients")

            await ws_manager.broadcast({
                "type": "hotkey_changed",
                "hotkey": hotkey_config,
                "timestamp": int(datetime.now().timestamp() * 1000)
            }, client_type_filter="tray")

            logger.info("Hotkey updated and broadcast", hotkey=hotkey_config)
        else:
            logger.warning("Invalid hotkey update received", hotkey=hotkey_config)

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
    presets = pm.get_all()
    return {
        "presets": [p.model_dump() for p in presets],
        "default_preset_id": pm._default_preset_id,
        "last_used_preset_id": pm._last_used_preset_id
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
async def preview_prompt(preset_id: str, text: str) -> dict:
    """Preview prompt effect with test text."""
    pm = get_prompt_manager()
    rendered = pm.render_prompt(preset_id, text)
    if rendered is None:
        raise HTTPException(status_code=404, detail="Preset not found")
    return {
        "preset_id": preset_id,
        "rendered_prompt": rendered
    }


@router.put("/prompts/default")
async def set_default_prompt(preset_id: str) -> dict:
    """Set the default prompt preset."""
    pm = get_prompt_manager()
    success = pm.set_default(preset_id)
    if not success:
        raise HTTPException(status_code=404, detail="Preset not found")

    # Also update user settings
    user_settings.set("default_preset_id", preset_id)

    return {
        "status": "updated",
        "default_preset_id": preset_id
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
    if background_tasks and transcriber:
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

    # Create a record from merged text
    record = await record_manager.create_record(
        audio_path="",  # No single audio file for merged session
        duration=session.total_duration,
        status=RecordStatus.TRANSCRIBED,
        raw_text=session.merged_text
    )

    return {
        "session_id": session_id,
        "record_id": record.id,
        "merged_text": session.merged_text,
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
        # Update segment status
        await session_manager.update_segment(
            session_id,
            segment_id,
            status=RecordStatus.TRANSCRIBING
        )

        # Execute transcription
        result = await transcriber.transcribe(audio_path)

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

    try:
        success = await provider.test_connection()
        return {"success": success, "error": None if success else "Connection test failed"}
    except Exception as e:
        return {"success": False, "error": str(e)}


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

