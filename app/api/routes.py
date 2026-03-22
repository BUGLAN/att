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
from fastapi.responses import PlainTextResponse
from typing import Optional, List
from pathlib import Path
import uuid

from app.config import settings
from app.audio.processor import AudioProcessor
from app.stt.transcriber import WhisperTranscriber
from app.llm.optimizer import TextOptimizer
from app.fragments.manager import FragmentManager
from app.fragments.models import Fragment, FragmentStatus, Session, ExportFormat
from app.utils.logging import get_logger

logger = get_logger(__name__)

router = APIRouter(prefix="/api", tags=["api"])

# Global instances (initialized in main.py)
audio_processor: Optional[AudioProcessor] = None
transcriber: Optional[WhisperTranscriber] = None
optimizer: Optional[TextOptimizer] = None
fragment_manager: Optional[FragmentManager] = None


def init_processors(
    audio: AudioProcessor,
    stt: WhisperTranscriber,
    llm: TextOptimizer,
    fragments: FragmentManager
) -> None:
    """Initialize global processor instances"""
    global audio_processor, transcriber, optimizer, fragment_manager
    audio_processor = audio
    transcriber = stt
    optimizer = llm
    fragment_manager = fragments


# ==================== Session Endpoints ====================

@router.post("/sessions", response_model=dict)
async def create_session(name: str = "") -> dict:
    """Create a new processing session"""
    session = await fragment_manager.create_session(name=name)
    return session.to_dict()


@router.get("/sessions", response_model=List[dict])
async def list_sessions(
    status: Optional[str] = None,
    limit: int = Query(default=100, le=1000)
) -> List[dict]:
    """List all sessions"""
    sessions = await fragment_manager.list_sessions(status=status, limit=limit)
    return [s.to_dict() for s in sessions]


@router.get("/sessions/{session_id}", response_model=dict)
async def get_session(session_id: str) -> dict:
    """Get session details"""
    session = await fragment_manager.get_session(session_id)
    if not session:
        raise HTTPException(status_code=404, detail="Session not found")
    return session.to_dict()


@router.delete("/sessions/{session_id}")
async def delete_session(session_id: str) -> dict:
    """Delete a session and all its fragments"""
    success = await fragment_manager.delete_session(session_id)
    if not success:
        raise HTTPException(status_code=404, detail="Session not found")
    return {"status": "deleted", "session_id": session_id}


# ==================== Audio Upload Endpoints ====================

@router.post("/audio/upload", response_model=dict)
async def upload_audio(
    file: UploadFile = File(...),
    session_id: Optional[str] = None,
    auto_transcribe: bool = True,
    auto_optimize: bool = False
) -> dict:
    """
    Upload an audio file for processing

    Args:
        file: Audio file (wav, mp3, m4a, flac, ogg, webm)
        session_id: Optional session ID (creates new if not provided)
        auto_transcribe: Automatically transcribe after upload
        auto_optimize: Automatically optimize after transcription

    Returns:
        Upload status with session and fragment IDs
    """
    # Validate file
    if not file.filename:
        raise HTTPException(status_code=400, detail="No filename provided")

    # 处理空字符串 - 视为 None
    if not session_id or session_id.strip() == "":
        session_id = None

    # Read file content
    content = await file.read()

    try:
        # Validate audio
        metadata = await audio_processor.validate_audio(content, file.filename)
    except ValueError as e:
        raise HTTPException(status_code=400, detail=str(e))

    # Create or get session
    if not session_id:
        session = await fragment_manager.create_session(name=file.filename)
        session_id = session.id

    # Save audio permanently
    audio_path = await audio_processor.save_permanent(content, file.filename)

    # Create fragment
    fragment = await fragment_manager.create_fragment(
        session_id=session_id,
        audio_path=str(audio_path),
        start_time=0,
        end_time=metadata.duration,
        metadata={"original_filename": file.filename, "format": metadata.format}
    )

    logger.info(
        "Audio uploaded",
        fragment_id=fragment.id,
        session_id=session_id,
        duration=metadata.duration
    )

    # 同步执行转写（不使用后台任务，避免全局变量问题）
    transcription_text = ""
    transcription_status = FragmentStatus.PENDING

    if auto_transcribe:
        try:
            # 更新状态为转写中
            await fragment_manager.update_fragment(
                fragment.id,
                status=FragmentStatus.TRANSCRIBING
            )

            # 执行转写
            result = await transcriber.transcribe(str(audio_path))
            transcription_text = result.text
            transcription_status = FragmentStatus.TRANSCRIBED

            # 更新转写结果
            await fragment_manager.update_fragment(
                fragment.id,
                raw_text=transcription_text,
                status=transcription_status
            )

            # 自动优化
            if auto_optimize and transcription_text:
                transcription_status = FragmentStatus.OPTIMIZING
                await fragment_manager.update_fragment(
                    fragment.id,
                    status=transcription_status
                )

                opt_result = await optimizer.optimize(transcription_text)

                await fragment_manager.update_fragment(
                    fragment.id,
                    optimized_text=opt_result.optimized_text,
                    status=FragmentStatus.OPTIMIZED
                )
                transcription_status = FragmentStatus.OPTIMIZED

            logger.info(
                "Audio transcribed",
                fragment_id=fragment.id,
                status=transcription_status.value
            )

        except Exception as e:
            logger.error(
                "Transcription failed",
                fragment_id=fragment.id,
                error=str(e)
            )
            await fragment_manager.update_fragment(
                fragment.id,
                status=FragmentStatus.FAILED
            )
            transcription_status = FragmentStatus.FAILED

    return {
        "status": "uploaded",
        "session_id": session_id,
        "fragment_id": fragment.id,
        "duration": metadata.duration,
        "auto_transcribe": auto_transcribe,
        "auto_optimize": auto_optimize,
        "transcription_status": transcription_status.value,
        "transcription_text": transcription_text[:200] if transcription_text else ""
    }


# ==================== Fragment Endpoints ====================


# ==================== Fragment Endpoints ====================

@router.get("/fragments", response_model=List[dict])
async def list_fragments(
    session_id: Optional[str] = None,
    status: Optional[str] = None,
    limit: int = Query(default=100, le=1000)
) -> List[dict]:
    """List fragments with optional filters"""
    fragment_status = FragmentStatus(status) if status else None
    fragments = await fragment_manager.list_fragments(
        session_id=session_id,
        status=fragment_status,
        limit=limit
    )
    return [f.to_dict() for f in fragments]


@router.get("/fragments/{fragment_id}", response_model=dict)
async def get_fragment(fragment_id: str) -> dict:
    """Get fragment details"""
    fragment = await fragment_manager.get_fragment(fragment_id)
    if not fragment:
        raise HTTPException(status_code=404, detail="Fragment not found")
    return fragment.to_dict()


@router.post("/fragments/{fragment_id}/transcribe")
async def transcribe_fragment(fragment_id: str) -> dict:
    """Transcribe a fragment's audio"""
    fragment = await fragment_manager.get_fragment(fragment_id)
    if not fragment:
        raise HTTPException(status_code=404, detail="Fragment not found")

    if not fragment.audio_path:
        raise HTTPException(status_code=400, detail="No audio file for this fragment")

    try:
        # 更新状态
        await fragment_manager.update_fragment(
            fragment_id,
            status=FragmentStatus.TRANSCRIBING
        )

        # 执行转写
        result = await transcriber.transcribe(fragment.audio_path)

        # 更新结果
        await fragment_manager.update_fragment(
            fragment_id,
            raw_text=result.text,
            status=FragmentStatus.TRANSCRIBED
        )

        logger.info(
            "Fragment transcribed",
            fragment_id=fragment_id,
            text_length=len(result.text)
        )

        return {
            "status": "transcribed",
            "fragment_id": fragment_id,
            "text": result.text[:500] if result.text else ""
        }

    except Exception as e:
        logger.error("Fragment transcription failed", fragment_id=fragment_id, error=str(e))
        await fragment_manager.update_fragment(fragment_id, status=FragmentStatus.FAILED)
        raise HTTPException(status_code=500, detail=str(e))


@router.post("/fragments/{fragment_id}/optimize")
async def optimize_fragment(
    fragment_id: str,
    style: str = "formal"
) -> dict:
    """Optimize a fragment's text"""
    fragment = await fragment_manager.get_fragment(fragment_id)
    if not fragment:
        raise HTTPException(status_code=404, detail="Fragment not found")

    if not fragment.raw_text:
        raise HTTPException(status_code=400, detail="No text to optimize")

    await fragment_manager.update_fragment(
        fragment_id,
        status=FragmentStatus.OPTIMIZING
    )

    result = await optimizer.optimize(fragment.raw_text, style=style)

    await fragment_manager.update_fragment(
        fragment_id,
        optimized_text=result.optimized_text,
        status=FragmentStatus.OPTIMIZED
    )

    return {
        "fragment_id": fragment_id,
        "original_text": fragment.raw_text,
        "optimized_text": result.optimized_text,
        "processing_time": result.processing_time
    }


@router.post("/fragments/merge")
async def merge_fragments(
    fragment_ids: List[str],
    use_optimized: bool = True
) -> dict:
    """Merge multiple fragments into a single text"""
    merged_text = await fragment_manager.merge_fragments(
        fragment_ids,
        use_optimized=use_optimized
    )

    return {
        "fragment_ids": fragment_ids,
        "merged_text": merged_text,
        "use_optimized": use_optimized
    }


@router.delete("/fragments/{fragment_id}")
async def delete_fragment(fragment_id: str) -> dict:
    """Delete a fragment"""
    success = await fragment_manager.delete_fragment(fragment_id)
    if not success:
        raise HTTPException(status_code=404, detail="Fragment not found")
    return {"status": "deleted", "fragment_id": fragment_id}


# ==================== Export Endpoints ====================

@router.get("/export/{session_id}")
async def export_session(
    session_id: str,
    format: str = "txt",
    use_optimized: bool = True
) -> PlainTextResponse:
    """Export a session's content"""
    session = await fragment_manager.get_session(session_id)
    if not session:
        raise HTTPException(status_code=404, detail="Session not found")

    try:
        export_format = ExportFormat(format)
    except ValueError:
        raise HTTPException(
            status_code=400,
            detail=f"Invalid format. Use: {[f.value for f in ExportFormat]}"
        )

    content = await fragment_manager.export_session(
        session_id,
        format=export_format,
        use_optimized=use_optimized
    )

    media_type = {
        ExportFormat.TXT: "text/plain",
        ExportFormat.JSON: "application/json",
        ExportFormat.SRT: "text/plain"
    }[export_format]

    return PlainTextResponse(
        content=content,
        media_type=media_type,
        headers={
            "Content-Disposition": f"attachment; filename=session_{session_id}.{format}"
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
