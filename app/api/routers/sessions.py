"""多段录音会话 + WebSocket 路由（sessions + streaming(ws)）。

从 `app/api/routes.py` 拆分而来：

- 会话 CRUD/合并的编排保留在路由层（都是「调用 session_manager + 组装响应」的薄逻辑），
  后台分段转写下沉到 `app/services/transcription_service.py`；
- WebSocket 协议（/api/ws 与 /api/asr/stream 的帧格式、断开释放语义）逐行保持原样，
  这是与渲染进程的实时契约。
"""

from __future__ import annotations

import json
import uuid
from datetime import datetime

from fastapi import (
    APIRouter,
    BackgroundTasks,
    Depends,
    File,
    HTTPException,
    Request,
    UploadFile,
    WebSocket,
    WebSocketDisconnect,
)

from app.api.deps import runtime_state
from app.api.uploads import read_upload_within_limit
from app.records.models import RecordStatus
from app.records.session_manager import session_manager
from app.services import records_service, transcription_service
from app.services.websocket import manager as ws_manager
from app.utils.logging import get_logger

logger = get_logger(__name__)

router = APIRouter(prefix="/api", tags=["api", "sessions", "streaming"])


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
    request: Request,
    file: UploadFile = File(...),
    background_tasks: BackgroundTasks = None,
    state=Depends(runtime_state),
) -> dict:
    """Upload a segment to the session."""
    # Verify session exists
    session = await session_manager.get_session(session_id)
    if not session:
        raise HTTPException(status_code=404, detail="Session not found")

    # Read and validate audio（同样按 max_upload_size 分块限流）
    content = await read_upload_within_limit(request, file, state.settings.max_upload_size)
    try:
        await state.audio_processor.validate_audio(content, file.filename)
    except ValueError as e:
        raise HTTPException(status_code=400, detail=str(e))

    # Save segment audio
    segment_id = str(uuid.uuid4())
    audio_path, wav_metadata = await state.audio_processor.save_as_wav_for_record(
        content,
        f"segment_{segment_id}.webm",
        segment_id,
        state.settings.audio_records_path
    )

    # Add segment to session
    segment = await session_manager.add_segment(
        session_id,
        str(audio_path),
        wav_metadata.duration
    )

    # Schedule background transcription
    if background_tasks and state.asr_provider:
        background_tasks.add_task(
            transcription_service.transcribe_segment_background,
            session_id,
            segment.id,
            str(audio_path),
            state.asr_provider,
            session_manager,
        )

    return {
        "segment_id": segment.id,
        "sequence": segment.sequence,
        "duration": wav_metadata.duration,
        "status": segment.status.value
    }


@router.post("/sessions/{session_id}/merge")
async def merge_session(session_id: str, state=Depends(runtime_state)) -> dict:
    """Merge all segments into a single record."""
    session = await session_manager.complete_session(session_id)
    if not session:
        raise HTTPException(status_code=404, detail="Session not found")

    merged_text = session.merged_text
    processing_time = 0.0

    if session.merged_audio_path and state.asr_provider:
        try:
            result = await state.asr_provider.transcribe(
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
    record = await state.record_manager.create_record(
        audio_path=session.merged_audio_path,
        duration=session.total_duration,
        status=RecordStatus.TRANSCRIBED,
        raw_text=merged_text,
        processing_time=processing_time
    )
    records_service.invalidate_record_index(state.record_manager, record.id)

    optimized_text = None
    if state.user_settings.get("auto_optimize", False) and merged_text:
        try:
            await state.record_manager.update_record(
                record.id,
                status=RecordStatus.OPTIMIZING
            )
            optimize_result = await state.optimizer.optimize(
                merged_text,
                preset_id=state.user_settings.get("default_preset_id")
            )
            updated_record = await state.record_manager.update_record(
                record.id,
                optimized_text=optimize_result.optimized_text,
                status=RecordStatus.OPTIMIZED
            )
            if updated_record:
                record = updated_record
                optimized_text = optimize_result.optimized_text
        except Exception as error:
            await state.record_manager.update_record(
                record.id,
                status=RecordStatus.TRANSCRIBED
            )
            logger.warning(
                "多段录音自动优化失败",
                session_id=session_id,
                record_id=record.id,
                error=str(error)
            )
        records_service.invalidate_record_index(state.record_manager, record.id)

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


# ==================== WebSocket Endpoints ====================

@router.websocket("/ws")
async def websocket_endpoint(websocket: WebSocket):
    """
    WebSocket endpoint for real-time communication.

    Args:
        websocket: WebSocket connection
    """
    client_id = str(uuid.uuid4())

    await ws_manager.connect(websocket, client_id)

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
    # 预置为 None：get_streaming_session_manager() 抛异常时，
    # 下面的 except/finally 分支不会因为 manager 未绑定而 NameError
    manager = None

    try:
        # 通过聚合层模块取管理器：既有测试会替换
        # app.api.routes.get_streaming_session_manager
        manager = runtime_state().get_streaming_session_manager()
        while True:
            message = await websocket.receive()
            message_type = message.get("type")

            if message_type == "websocket.disconnect":
                # 客户端断开：先跳出循环，由 finally 统一释放会话（避免 session 泄漏）
                logger.info("流式 ASR WebSocket 客户端断开", session_id=stream_session_id)
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
                        "timestamp": _now_iso()
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
                        "timestamp": _now_iso()
                    })
                    continue

                if command == "stream_cancel":
                    if stream_session_id:
                        await manager.cancel_session(stream_session_id)
                        stream_session_id = None
                    await websocket.send_json({
                        "type": "stream_cancelled",
                        "timestamp": _now_iso()
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
                    "timestamp": _now_iso()
                })

    except WebSocketDisconnect:
        logger.info("流式 ASR WebSocket 连接中断", session_id=stream_session_id)
    except Exception as error:
        logger.error("Streaming ASR WebSocket error", error=str(error), session_id=stream_session_id)
        # 客户端可能已断开：向已关闭的 socket 发送/关闭都会抛异常，这里必须各自兜住
        try:
            await websocket.send_json({
                "type": "stream_error",
                "data": {"error": str(error)},
                "timestamp": _now_iso()
            })
        except Exception as send_error:
            logger.warning("流式错误信息发送失败（连接可能已关闭）", error=str(send_error))
        try:
            await websocket.close()
        except Exception as close_error:
            logger.warning("流式 WebSocket 关闭失败", error=str(close_error))
    finally:
        # 统一释放点：无论是断开 break 退出、正常结束还是异常退出，
        # 只要还有活动会话就调用 cancel_session，避免会话与临时 wav 泄漏
        if stream_session_id and manager is not None:
            try:
                await manager.cancel_session(stream_session_id)
                logger.info("已释放流式会话", session_id=stream_session_id)
            except Exception as cancel_error:
                logger.warning(
                    "释放流式会话失败",
                    session_id=stream_session_id,
                    error=str(cancel_error)
                )
            stream_session_id = None


async def _handle_websocket_message(client_id: str, data: dict) -> None:
    """
    Handle incoming WebSocket messages.

    Args:
        client_id: Sender client ID
        data: Message data
    """
    message_type = data.get("type")

    if message_type == "status_update":
        await ws_manager.update_status(
            recording=data.get("recording", False),
            transcribing=data.get("transcribing", False),
            record_id=data.get("record_id"),
            duration=data.get("duration", 0.0)
        )
        return

    if message_type == "ping":
        await ws_manager.send_to_client(client_id, {"type": "pong"})
        return

    logger.warning(
        "Unknown WebSocket message type",
        client_id=client_id,
        message_type=message_type
    )


def _now_iso() -> str:
    """WebSocket 事件时间戳统一格式。"""
    return datetime.now().isoformat()
