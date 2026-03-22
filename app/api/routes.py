"""
FastAPI REST API endpoints
"""
from fastapi import (
    APIRouter,
    UploadFile,
    File,
    HTTPException,
    Query
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


# ==================== Audio Upload Endpoint ====================

@router.post("/audio/upload", response_model=dict)
async def upload_audio(
    file: UploadFile = File(...),
    auto_transcribe: bool = True
) -> dict:
    """
    Upload an audio file for processing

    Args:
        file: Audio file (wav, mp3, m4a, flac, ogg, webm)
        auto_transcribe: Automatically transcribe after upload

    Returns:
        Upload status with record_id
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

    # Create record
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

    # Sync transcription (not using background tasks to avoid global variable issues)
    transcription_text = ""
    transcription_status = RecordStatus.PENDING

    if auto_transcribe:
        try:
            # Update status to transcribing
            await record_manager.update_record(
                record.id,
                status=RecordStatus.TRANSCRIBING
            )

            # Execute transcription
            result = await transcriber.transcribe(str(audio_path))
            transcription_text = result.text
            transcription_status = RecordStatus.TRANSCRIBED

            # Update transcription result
            await record_manager.update_record(
                record.id,
                raw_text=transcription_text,
                status=transcription_status
            )

            logger.info(
                "Audio transcribed",
                record_id=record.id,
                status=transcription_status.value
            )

        except Exception as e:
            logger.error(
                "Transcription failed",
                record_id=record.id,
                error=str(e)
            )
            await record_manager.update_record(
                record.id,
                status=RecordStatus.FAILED
            )
            transcription_status = RecordStatus.FAILED

    return {
        "status": "uploaded",
        "record_id": record.id,
        "duration": wav_metadata.duration,
        "auto_transcribe": auto_transcribe,
        "transcription_status": transcription_status.value,
        "transcription_text": transcription_text if transcription_text else ""
    }


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
    style: str = "formal"
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
