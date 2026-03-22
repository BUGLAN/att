"""
FastAPI REST API endpoints
"""
from fastapi import (
    APIRouter,
    UploadFile,
    File,
    HTTPException,
    Query,
    BackgroundTasks
)
from fastapi.responses import PlainTextResponse, FileResponse
from typing import Optional, List
from pathlib import Path
import uuid

from app.config import settings
from app.audio.processor import AudioProcessor
from app.stt.transcriber import WhisperTranscriber
from app.llm.optimizer import TextOptimizer
from app.records.manager import RecordManager
from app.records.models import Record, RecordStatus, ExportFormat
from app.utils.logging import get_logger

logger = get_logger(__name__)

router = APIRouter(prefix="/api", tags=["api"])

# Global instances (initialized in main.py)
audio_processor: Optional[AudioProcessor] = None
transcriber: Optional[WhisperTranscriber] = None
optimizer: Optional[TextOptimizer] = None
record_manager: Optional[RecordManager] = None


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
    record_manager: RecordManager
) -> None:
    """
    Background task for audio transcription.

    Args:
        record_id: Record ID to update
        audio_path: Path to audio file
        transcriber: Transcriber instance (passed explicitly to avoid global issues)
        record_manager: Record manager instance
    """
    try:
        # Update status to transcribing
        await record_manager.update_record(
            record_id,
            status=RecordStatus.TRANSCRIBING
        )

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
            status=RecordStatus.TRANSCRIBED
        )

        logger.info(
            "Background transcription complete",
            record_id=record_id,
            duration=result.duration,
            processing_time=f"{result.processing_time:.2f}s"
        )

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
            record_manager
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

    return {
        "record_id": record.id,
        "status": record.status.value,
        "raw_text": record.raw_text if record.status == RecordStatus.TRANSCRIBED else None,
        "duration": record.duration
    }


@router.put("/records/{record_id}", response_model=dict)
async def update_record(
    record_id: str,
    raw_text: Optional[str] = None,
    optimized_text: Optional[str] = None
) -> dict:
    """Update record text (for editable input)"""
    record = await record_manager.update_record(
        record_id,
        raw_text=raw_text,
        optimized_text=optimized_text
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
        "deepseek_configured": bool(settings.deepseek_api_key)
    }
