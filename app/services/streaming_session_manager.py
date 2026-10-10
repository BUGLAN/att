"""
流式录音会话管理
"""
import asyncio
from dataclasses import dataclass
from datetime import datetime
from pathlib import Path
import uuid
import wave

import numpy as np

from app.config import settings, user_settings
from app.llm.optimizer import TextOptimizer
from app.records.manager import RecordManager
from app.records.models import RecordStatus
from app.stt.models import ASRProviderType
from app.stt.factory import get_asr_provider_config
from app.stt.providers.doubao_asr_provider import DoubaoStreamingProvider
from app.stt.providers.qwen_streaming_runtime_provider import QwenStreamingProvider
from app.services.websocket import manager as ws_manager
from app.utils.logging import get_logger

logger = get_logger(__name__)

STREAM_SAMPLE_RATE = 16000
STREAM_CHANNELS = 1
STREAM_SAMPLE_WIDTH = 2


def _log_background_task_error(task: "asyncio.Task") -> None:
    """后台优化任务的兜底回调：任务自身已捕获业务异常，这里只兜真正的意外错误。"""
    if task.cancelled():
        return
    error = task.exception()
    if error is not None:
        logger.warning("流式录音后台优化任务异常退出", error=str(error))


@dataclass
class ManagedStreamingSession:
    """受管理的流式会话"""
    session_id: str
    asr_session: object
    wav_path: Path
    wav_writer: wave.Wave_write
    created_at: datetime
    language: str = "zh"
    last_text: str = ""
    sample_count: int = 0


class StreamingSessionManager:
    """管理真流式会话（按当前 ASR provider 分派：Qwen 本地/远端桥接 或 豆包在线）"""

    def __init__(
        self,
        record_manager: RecordManager,
        optimizer: TextOptimizer
    ):
        self.record_manager = record_manager
        self.optimizer = optimizer
        self._provider = None
        self._sessions: dict[str, ManagedStreamingSession] = {}
        self._lock = asyncio.Lock()

    def _get_provider(self):
        if self._provider is not None:
            return self._provider

        config = get_asr_provider_config()

        if config.provider == ASRProviderType.DOUBAO_ASR:
            self._provider = DoubaoStreamingProvider(
                api_key=config.doubao_asr.get("api_key", ""),
                resource_id=config.doubao_asr.get("resource_id", ""),
                endpoint=config.doubao_asr.get("endpoint", ""),
                connect_timeout_seconds=config.doubao_asr.get("connect_timeout_seconds", 10.0),
                chunk_ms=config.doubao_asr.get("chunk_ms", 200),
                end_window_size=config.doubao_asr.get("end_window_size", 0),
                force_to_speech_time=config.doubao_asr.get("force_to_speech_time", 0),
            )
            return self._provider

        if config.provider != ASRProviderType.QWEN_ASR:
            raise RuntimeError(
                f"当前 ASR provider 为 {config.provider.value}，不支持真流式录音；"
                "请在设置中切换为 Qwen3-ASR 或豆包语音"
            )

        self._provider = QwenStreamingProvider(
            model_name=config.qwen_asr["model_name"],
            cache_dir=config.qwen_asr["cache_dir"],
            device=config.qwen_asr.get("device", "auto"),
            max_new_tokens=config.qwen_asr.get("max_new_tokens", 256),
            streaming_backend=config.qwen_asr.get("streaming_backend", "auto"),
            streaming_base_url=config.qwen_asr.get("streaming_base_url", ""),
            streaming_timeout_seconds=config.qwen_asr.get("streaming_timeout_seconds", 30.0),
        )
        return self._provider

    def _create_wav_writer(self, wav_path: Path) -> wave.Wave_write:
        wav_path.parent.mkdir(parents=True, exist_ok=True)
        writer = wave.open(str(wav_path), "wb")
        writer.setnchannels(STREAM_CHANNELS)
        writer.setsampwidth(STREAM_SAMPLE_WIDTH)
        writer.setframerate(STREAM_SAMPLE_RATE)
        return writer

    async def create_session(
        self,
        language: str = "zh",
        context: str = ""
    ) -> dict:
        provider = self._get_provider()
        asr_session = await provider.create_session(language=language, context=context)
        session_id = str(uuid.uuid4())
        wav_path = self.record_manager.get_audio_file_path(session_id)
        wav_writer = self._create_wav_writer(wav_path)

        session = ManagedStreamingSession(
            session_id=session_id,
            asr_session=asr_session,
            wav_path=wav_path,
            wav_writer=wav_writer,
            created_at=datetime.now(),
            language=language or "zh"
        )

        async with self._lock:
            self._sessions[session_id] = session

        logger.info("创建流式录音会话", session_id=session_id, wav_path=str(wav_path))
        return {
            "session_id": session_id,
            "language": session.language,
            "sample_rate": STREAM_SAMPLE_RATE
        }

    async def append_audio(
        self,
        session_id: str,
        payload: bytes
    ) -> dict:
        async with self._lock:
            session = self._sessions.get(session_id)

        if not session:
            raise ValueError("流式会话不存在或已过期")

        if not payload:
            return {
                "text": session.last_text,
                "language": session.language,
                "duration": session.sample_count / STREAM_SAMPLE_RATE
            }

        pcm16 = np.frombuffer(payload, dtype=np.int16)
        session.wav_writer.writeframes(payload)
        session.sample_count += int(pcm16.shape[0])

        update = await asyncio.to_thread(session.asr_session.append_audio, pcm16)
        session.last_text = update.text
        if update.language:
            session.language = update.language

        return {
            "text": session.last_text,
            "language": session.language,
            "duration": session.sample_count / STREAM_SAMPLE_RATE
        }

    async def finish_session(self, session_id: str) -> dict:
        async with self._lock:
            session = self._sessions.pop(session_id, None)

        if not session:
            raise ValueError("流式会话不存在或已结束")

        try:
            update = await asyncio.to_thread(session.asr_session.finish)
            final_text = update.text.strip()
            if update.language:
                session.language = update.language
        finally:
            session.wav_writer.close()
            session_close = getattr(session.asr_session, "close", None)
            if callable(session_close):
                await asyncio.to_thread(session_close)

        duration = session.sample_count / STREAM_SAMPLE_RATE
        if not final_text:
            # 空文本是正常业务结果（极短点击 / 没有有效语音），不走错误通道：
            # 音频不留档，回 record=null 让前端做「未识别到内容」的轻提示。
            if session.wav_path.exists():
                session.wav_path.unlink()
            logger.info(
                "流式录音未识别到有效语音文本",
                session_id=session_id,
                duration=duration
            )
            return {
                "record": None,
                "raw_text": "",
                "language": session.language,
                "duration": duration
            }

        record = await self.record_manager.create_record(
            audio_path=str(session.wav_path),
            duration=duration,
            status=RecordStatus.TRANSCRIBED,
            raw_text=final_text
        )

        payload = {
            "record": record.to_dict(),
            "raw_text": final_text,
            "language": session.language,
            "duration": duration
        }

        if user_settings.get("auto_optimize", False):
            # 优化必须留在收尾关键路径之外：LLM provider 无超时（SDK 默认 600s），
            # 内联 await 会让前端在「正在完成流式转写」卡最长 10 分钟。
            # 这里先立即返回 transcribed 记录，优化放后台任务执行，
            # 完成后经全局 WS 广播 optimization_completed（与单段录音路径一致）。
            optimize_task = asyncio.create_task(
                self._optimize_in_background(record.id, final_text)
            )
            optimize_task.add_done_callback(_log_background_task_error)

        logger.info(
            "完成流式录音会话",
            session_id=session_id,
            record_id=payload["record"]["id"],
            duration=duration
        )
        return payload

    async def _optimize_in_background(self, record_id: str, final_text: str) -> None:
        """后台执行流式录音的自动优化，完成后广播 optimization_completed。"""
        try:
            optimize_result = await self.optimizer.optimize(
                final_text,
                preset_id=user_settings.get("default_preset_id")
            )
            record = await self.record_manager.update_record(
                record_id,
                optimized_text=optimize_result.optimized_text,
                status=RecordStatus.OPTIMIZED
            )
            if record:
                await ws_manager.broadcast({
                    "type": "optimization_completed",
                    "data": {
                        "record_id": record.id,
                        "optimized_text": optimize_result.optimized_text
                    },
                    "timestamp": datetime.now().isoformat()
                })
            logger.info("流式录音自动优化完成", record_id=record_id)
        except Exception as error:
            logger.warning("流式录音自动优化失败", record_id=record_id, error=str(error))
            # 与单段模式对齐：失败把记录复位为 transcribed 并广播失败事件
            try:
                await self.record_manager.update_record(
                    record_id,
                    status=RecordStatus.TRANSCRIBED
                )
            except Exception as reset_error:
                logger.warning(
                    "流式录音优化失败后复位记录状态失败",
                    record_id=record_id,
                    error=str(reset_error)
                )
            try:
                await ws_manager.broadcast({
                    "type": "optimization_failed",
                    "data": {
                        "record_id": record_id,
                        "error": str(error),
                        "status": RecordStatus.TRANSCRIBED.value,
                        # 自动优化开启时前端不在 transcribed 阶段粘贴，
                        # 失败广播带上原文，前端兜底粘贴，保证总有输出
                        "text": final_text
                    },
                    "timestamp": datetime.now().isoformat()
                })
            except Exception as broadcast_error:
                logger.warning(
                    "推送流式优化失败事件失败",
                    record_id=record_id,
                    error=str(broadcast_error)
                )

    async def cancel_session(self, session_id: str) -> None:
        async with self._lock:
            session = self._sessions.pop(session_id, None)

        if not session:
            return

        try:
            session_cancel = getattr(session.asr_session, "cancel", None)
            if callable(session_cancel):
                await asyncio.to_thread(session_cancel)
            session.wav_writer.close()
        finally:
            if session.wav_path.exists():
                session.wav_path.unlink()

        logger.info("取消流式录音会话", session_id=session_id)

    async def close(self) -> None:
        async with self._lock:
            sessions = list(self._sessions.values())
            self._sessions.clear()

        for session in sessions:
            try:
                session_cancel = getattr(session.asr_session, "cancel", None)
                if callable(session_cancel):
                    await asyncio.to_thread(session_cancel)
                session.wav_writer.close()
            finally:
                if session.wav_path.exists():
                    session.wav_path.unlink()

        if self._provider is not None:
            await self._provider.close()
            self._provider = None
