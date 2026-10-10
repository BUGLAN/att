"""音频上传 + 记录（records）路由。

从 `app/api/routes.py` 拆分而来（原文件 1479+ 行、42 端点），本模块只做
「解析请求 → 调 service → 组装响应」：

- 音频上传的转写编排在 `app/services/transcription_service.py`；
- 列表查询（进程内索引 + 回落扫描）在 `app/services/records_service.py`；
- 运行时实例（record_manager / audio_processor / optimizer / settings / user_settings）
  通过 `app.api.deps.runtime_state` 在请求期解析，保持既有注入与测试替换语义。

契约说明：`GET /api/records` 的响应体仍是**记录数组**（不包装成 {items,total}），
新增的只是 `offset` 查询参数与默认 `limit` 200（上限 1000）。
"""

from __future__ import annotations

import json
import uuid
from datetime import datetime
from pathlib import Path
from typing import List, Optional

from fastapi import (
    APIRouter,
    BackgroundTasks,
    Depends,
    File,
    HTTPException,
    Query,
    Request,
    UploadFile,
)
from fastapi.responses import FileResponse, PlainTextResponse
from pydantic import BaseModel

from app.api.deps import runtime_state
from app.api.uploads import read_upload_within_limit
from app.llm.security import redact_secrets
from app.records.models import ExportFormat, RecordStatus
from app.services import records_service, transcription_service
from app.services.websocket import manager as ws_manager
from app.utils.logging import get_logger

logger = get_logger(__name__)

router = APIRouter(prefix="/api", tags=["api", "records"])


class RecordUpdateRequest(BaseModel):
    """Request model for updating a record"""
    raw_text: Optional[str] = None
    optimized_text: Optional[str] = None
    is_modified: Optional[bool] = None
    is_favorite: Optional[bool] = None
    # 乐观锁：可选。传入当前 revision 时，若服务端版本已变则返回 409，
    # 避免「读-改-写」期间被他人改动导致静默丢更新。不传则保持既有行为。
    revision: Optional[int] = None


def _invalidate_index(state, record_id: Optional[str] = None) -> None:
    """写入路径 write-through：立即失效索引条目（失败不影响写入结果）。"""
    records_service.invalidate_record_index(state.record_manager, record_id)


# ==================== Audio Upload Endpoint ====================

@router.post("/audio/upload", response_model=dict)
async def upload_audio(
    request: Request,
    file: UploadFile = File(...),
    auto_transcribe: bool = True,
    background_tasks: BackgroundTasks = None,
    state=Depends(runtime_state),
) -> dict:
    """
    上传音频文件并（可选）异步转写。

    Args:
        request: HTTP 请求对象（用于读取 Content-Length 做前置大小校验）
        file: 音频文件（wav, mp3, m4a, flac, ogg, webm）
        auto_transcribe: 上传后是否自动转写（异步）
        background_tasks: FastAPI 后台任务

    Returns:
        上传结果与 record_id（转写在后台进行）
    """
    audio_processor = state.audio_processor
    record_manager = state.record_manager
    app_settings = state.settings

    # Validate file
    if not file.filename:
        raise HTTPException(status_code=400, detail="No filename provided")

    # Read file content（按 settings.max_upload_size 边读边限流，避免大文件直接 OOM）
    content = await read_upload_within_limit(request, file, app_settings.max_upload_size)

    try:
        # Validate audio
        await audio_processor.validate_audio(content, file.filename)
    except ValueError as e:
        raise HTTPException(status_code=400, detail=str(e))

    # Generate record ID
    record_id = str(uuid.uuid4())

    # Convert and save as WAV
    audio_path, wav_metadata = await audio_processor.save_as_wav_for_record(
        content,
        file.filename,
        record_id,
        app_settings.audio_records_path
    )

    # Create record with PENDING status
    record = await record_manager.create_record(
        audio_path=str(audio_path),
        duration=wav_metadata.duration,
        status=RecordStatus.PENDING
    )
    _invalidate_index(state, record.id)

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
            transcription_service.transcribe_audio_background,
            record.id,
            str(audio_path),
            state.asr_provider,
            record_manager,
            state.optimizer,
            state.user_settings
        )
        response_data["transcription_status"] = RecordStatus.TRANSCRIBING.value
        logger.info("Scheduled background transcription", record_id=record.id)

    return response_data


# ==================== Record Endpoints ====================

@router.get("/records", response_model=List[dict])
async def list_records(
    status: Optional[str] = None,
    limit: int = Query(default=200, le=1000),
    offset: int = Query(default=0, ge=0),
    state=Depends(runtime_state),
) -> List[dict]:
    """列出记录（默认 200 条，上限 1000；走进程内索引，异常时回落全目录扫描）。

    响应体保持为记录数组，前端契约不变。
    """
    record_status = RecordStatus(status) if status else None
    return await records_service.list_record_page(
        record_manager=state.record_manager,
        status=record_status,
        limit=limit,
        offset=offset,
    )


@router.get("/records/search", response_model=dict)
async def search_records(
    q: Optional[str] = None,
    search_type: str = Query(default="all", pattern="^(all|raw|optimized)$"),
    favorite_only: bool = False,
    page: int = Query(default=1, ge=1),
    page_size: int = Query(default=100, ge=1, le=200),
    state=Depends(runtime_state),
) -> dict:
    """按文本内容与收藏状态搜索记录（page_size 上限从 100 提到 200）。"""
    records = await state.record_manager.search_records(
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
async def get_record(record_id: str, state=Depends(runtime_state)) -> dict:
    """Get record details"""
    record = await state.record_manager.get_record(record_id)
    if not record:
        raise HTTPException(status_code=404, detail="Record not found")
    return record.to_dict()


@router.get("/records/{record_id}/status", response_model=dict)
async def get_record_status(record_id: str, state=Depends(runtime_state)) -> dict:
    """
    查询转写状态（供前端轮询），只返回轮询必需的最小数据。
    """
    record = await state.record_manager.get_record(record_id)
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
    request: RecordUpdateRequest,
    state=Depends(runtime_state),
) -> dict:
    """Update record text (for editable input)"""
    record = await state.record_manager.update_record(
        record_id,
        raw_text=request.raw_text,
        optimized_text=request.optimized_text,
        is_modified=request.is_modified,
        is_favorite=request.is_favorite,
        expected_revision=request.revision
    )
    if not record:
        # 区分「记录不存在」与「版本冲突」：前者 404，后者 409，
        # 让前端能提示用户「内容已被修改，请刷新后重试」而不是误报未找到。
        if request.revision is not None:
            existing = await state.record_manager.get_record(record_id)
            if existing is not None:
                raise HTTPException(
                    status_code=409,
                    detail="记录已被其他操作修改，请刷新后重试"
                )
        raise HTTPException(status_code=404, detail="Record not found")
    _invalidate_index(state, record_id)
    return record.to_dict()


@router.delete("/records/{record_id}")
async def delete_record(record_id: str, state=Depends(runtime_state)) -> dict:
    """Delete a record and its audio file"""
    success = await state.record_manager.delete_record(record_id)
    if not success:
        raise HTTPException(status_code=404, detail="Record not found")
    _invalidate_index(state, record_id)
    return {"status": "deleted", "record_id": record_id}


@router.get("/records/{record_id}/audio")
async def get_record_audio(record_id: str, state=Depends(runtime_state)) -> FileResponse:
    """Serve audio file for playback"""
    record = await state.record_manager.get_record(record_id)
    if not record:
        raise HTTPException(status_code=404, detail="Record not found")

    # 空 audio_path 必须直接 404：
    # Windows 上 Path("").exists() 为 True（"." 是目录），会走到 FileResponse('.')
    # 并抛 PermissionError → 500。真实数据中存在 36 条 audio_path="" 的记录。
    audio_path_value = (record.audio_path or "").strip()
    if not audio_path_value:
        raise HTTPException(status_code=404, detail="Audio file not found")

    audio_path = Path(audio_path_value)
    # 用 is_file() 而非 exists()：目录同样存在，但 FileResponse 无法处理目录
    if not audio_path.is_file():
        raise HTTPException(status_code=404, detail="Audio file not found")

    return FileResponse(
        str(audio_path),
        media_type="audio/wav"
    )


@router.post("/records/{record_id}/optimize")
async def optimize_record(
    record_id: str,
    style: str = "default",
    state=Depends(runtime_state),
) -> dict:
    """Optimize record text with AI"""
    record = await state.record_manager.get_record(record_id)
    if not record:
        raise HTTPException(status_code=404, detail="Record not found")

    if not record.raw_text:
        raise HTTPException(status_code=400, detail="No text to optimize")

    await state.record_manager.update_record(
        record_id,
        status=RecordStatus.OPTIMIZING
    )

    try:
        result = await state.optimizer.optimize(record.raw_text, style=style)
    except Exception as error:
        # 失败必须复位状态：否则记录会永久卡在 optimizing（真实数据已出现 2 条）
        # 错误文案先脱敏：上游鉴权失败会在错误体里回显密钥，
        # 它会流入日志、记录 JSON 与前端提示（AGENTS.md §3.4）
        safe_error = redact_secrets(error)
        reset_status = RecordStatus.TRANSCRIBED if record.raw_text else RecordStatus.FAILED
        await state.record_manager.update_record(record_id, status=reset_status)
        # 把失败原因持久化到记录（Record.error），用户在记录详情里能看到历史错误
        await state.record_manager.set_error_message(record_id, safe_error)
        _invalidate_index(state, record_id)
        logger.error(
            "文本优化失败，已复位记录状态",
            record_id=record_id,
            reset_status=reset_status.value,
            error=safe_error,
            exc_info=True
        )
        # 通过 WebSocket 把错误信息推给前端；
        # 错误详情同时写入 Record.error（持久化）并通过 HTTP 502 的 detail 返回
        try:
            await ws_manager.broadcast({
                "type": "optimization_failed",
                "data": {
                    "record_id": record_id,
                    "error": safe_error,
                    "status": reset_status.value
                },
                "timestamp": datetime.now().isoformat()
            })
        except Exception as broadcast_error:
            logger.warning(
                "推送优化失败事件失败",
                record_id=record_id,
                error=str(broadcast_error)
            )
        raise HTTPException(
            status_code=502,
            detail=f"文本优化失败：{safe_error}"
        ) from error

    await state.record_manager.update_record(
        record_id,
        optimized_text=result.optimized_text,
        status=RecordStatus.OPTIMIZED
    )
    # 优化成功即清除历史错误原因
    await state.record_manager.set_error_message(record_id, "")
    _invalidate_index(state, record_id)

    if result.fallback_reason:
        # 用户配置的 LLM 调用失败后回退到 .env 内置 key：优化本身成功了，
        # 但「配置没生效」必须让用户知道，否则设置页里的 key 配错会被静默掩盖。
        # 手动优化走同步响应（fallback_reason 字段），不额外广播，避免前端重复提示。
        logger.warning(
            "文本优化已降级：回退到内置 key 完成",
            record_id=record_id,
            reason=result.fallback_reason
        )

    return {
        "record_id": record_id,
        "original_text": record.raw_text,
        "optimized_text": result.optimized_text,
        "processing_time": result.processing_time,
        "fallback_reason": result.fallback_reason
    }


@router.get("/records/{record_id}/export")
async def export_record(
    record_id: str,
    format: str = "txt",
    use_optimized: bool = True,
    state=Depends(runtime_state),
) -> PlainTextResponse:
    """Export a single record"""
    record = await state.record_manager.get_record(record_id)
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
        content = json.dumps(record.to_dict(), ensure_ascii=False, indent=2)
        media_type = "application/json"

    return PlainTextResponse(
        content=content,
        media_type=media_type,
        headers={
            "Content-Disposition": f"attachment; filename=record_{record_id}.{format}"
        }
    )
