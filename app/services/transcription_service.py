"""转写后台任务编排：转写 → 自动优化 → 落盘 → WebSocket 通知。

从 `app/api/routes.py` 原样下沉（原 `_transcribe_audio_background` 137 行、
`_transcribe_segment_background` 89 行），行为逐行保持一致：

- 状态机：pending → transcribing → transcribed →（可选）optimizing → optimized；
- 自动优化失败必须把状态复位为 transcribed（否则记录永久卡在 optimizing）；
- 转写失败必须置 failed 并广播 error；
- 分段转写失败置 failed 并广播 segment_failed。
"""

from __future__ import annotations

from datetime import datetime

from app.llm.security import redact_secrets
from app.records.manager import RecordManager
from app.records.models import RecordStatus
from app.records.session_manager import SessionManager
from app.services.websocket import manager as ws_manager
from app.stt.base import ASRProvider
from app.utils.logging import get_logger

logger = get_logger(__name__)


def build_completion_metadata(**data) -> dict:
    """在完成事件上附带「是否有 Electron/Web 活动客户端」的元数据。"""
    return {
        **data,
        "electron_active": ws_manager.get_active_web_client() is not None,
    }


async def transcribe_audio_background(
    record_id: str,
    audio_path: str,
    asr_provider: ASRProvider,
    record_manager: RecordManager,
    optimizer,
    user_settings,
) -> None:
    """单条录音的后台转写 + 自动优化编排。

    Args:
        record_id: 需要更新的记录 ID
        audio_path: 音频文件路径
        asr_provider: ASR provider 实例（显式传入以避免全局引用问题）
        record_manager: 记录管理器
        optimizer: 文本优化器（用于自动优化）
        user_settings: 用户设置实例
    """
    try:
        # 状态：转写中
        await record_manager.update_record(
            record_id,
            status=RecordStatus.TRANSCRIBING
        )

        await ws_manager.broadcast({
            "type": "transcription_started",
            "data": {"record_id": record_id},
            "timestamp": datetime.now().isoformat()
        })

        logger.info(
            "Background transcription started",
            record_id=record_id
        )

        # 执行转写
        result = await asr_provider.transcribe(audio_path)

        await record_manager.update_record(
            record_id,
            raw_text=result.text,
            status=RecordStatus.TRANSCRIBED,
            processing_time=result.processing_time
        )

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

        # 按设置执行自动优化
        if user_settings.get("auto_optimize", False):
            try:
                logger.info("Auto-optimizing text", record_id=record_id)
                await record_manager.update_record(
                    record_id,
                    status=RecordStatus.OPTIMIZING
                )

                await ws_manager.broadcast({
                    "type": "optimization_started",
                    "data": {"record_id": record_id},
                    "timestamp": datetime.now().isoformat()
                })

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
                # 自动优化成功即清除历史错误原因
                await record_manager.set_error_message(record_id, "")

                await ws_manager.broadcast({
                    "type": "optimization_completed",
                    "data": build_completion_metadata(
                        record_id=record_id,
                        optimized_text=optimize_result.optimized_text
                    ),
                    "timestamp": datetime.now().isoformat()
                })

                if optimize_result.fallback_reason:
                    # 用户配置的 LLM 失败、回退到 .env 内置 key：优化成功但配置没生效，
                    # 自动优化是异步路径，只能靠 WebSocket 让界面提示用户
                    logger.warning(
                        "Auto-optimization degraded to builtin key",
                        record_id=record_id,
                        reason=optimize_result.fallback_reason
                    )
                    await ws_manager.broadcast({
                        "type": "optimization_degraded",
                        "data": {
                            "record_id": record_id,
                            "reason": optimize_result.fallback_reason
                        },
                        "timestamp": datetime.now().isoformat()
                    })

                logger.info("Auto-optimization complete", record_id=record_id)
            except Exception as e:
                # 与 /records/{id}/optimize 一致：失败必须把状态复位，
                # 否则自动优化失败后记录会永久停留在 optimizing
                # 错误文案先脱敏：上游鉴权失败会在错误体里回显密钥（AGENTS.md §3.4）
                safe_error = redact_secrets(e)
                await record_manager.update_record(
                    record_id,
                    status=RecordStatus.TRANSCRIBED
                )
                # 持久化失败原因，便于用户在记录详情里看到历史错误
                await record_manager.set_error_message(record_id, safe_error)
                logger.error(
                    "Auto-optimization failed, status reset to transcribed",
                    record_id=record_id,
                    error=safe_error
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

        await ws_manager.broadcast({
            "type": "error",
            "data": {
                "record_id": record_id,
                "error": str(e)
            },
            "timestamp": datetime.now().isoformat()
        })


async def transcribe_segment_background(
    session_id: str,
    segment_id: str,
    audio_path: str,
    asr_provider: ASRProvider,
    session_manager: SessionManager,
) -> None:
    """多段录音中单个片段的后台转写编排。"""
    try:
        session = await session_manager.get_session(session_id)
        sequence = None
        if session:
            for segment in session.segments:
                if segment.id == segment_id:
                    sequence = segment.sequence
                    break

        # 状态：转写中
        await session_manager.update_segment(
            session_id,
            segment_id,
            status=RecordStatus.TRANSCRIBING
        )

        # 执行转写（分段不做 VAD 过滤，保持原生分段语义）
        result = await asr_provider.transcribe(audio_path, vad_filter=False)

        await session_manager.update_segment(
            session_id,
            segment_id,
            raw_text=result.text,
            status=RecordStatus.TRANSCRIBED,
            processing_time=result.processing_time
        )

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
