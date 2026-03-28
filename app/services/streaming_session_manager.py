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
from app.stt.providers.qwen_streaming_runtime_provider import QwenStreamingProvider
from app.utils.logging import get_logger

logger = get_logger(__name__)

STREAM_SAMPLE_RATE = 16000
STREAM_CHANNELS = 1
STREAM_SAMPLE_WIDTH = 2


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
    """管理 Qwen 真流式会话"""

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

    def _get_provider(self) -> QwenStreamingProvider:
        if self._provider is not None:
            return self._provider

        config = get_asr_provider_config()
        if config.provider != ASRProviderType.QWEN_ASR:
            raise RuntimeError("当前 ASR provider 不是 qwen_asr，无法启用真流式")

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
            if session.wav_path.exists():
                session.wav_path.unlink()
            raise ValueError("未识别到有效语音文本")

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
            try:
                optimize_result = await self.optimizer.optimize(
                    final_text,
                    preset_id=user_settings.get("default_preset_id")
                )
                record = await self.record_manager.update_record(
                    record.id,
                    optimized_text=optimize_result.optimized_text,
                    status=RecordStatus.OPTIMIZED
                )
                if record:
                    payload["record"] = record.to_dict()
                    payload["optimized_text"] = optimize_result.optimized_text
            except Exception as error:
                logger.warning("流式录音自动优化失败", record_id=record.id, error=str(error))

        logger.info(
            "完成流式录音会话",
            session_id=session_id,
            record_id=payload["record"]["id"],
            duration=duration
        )
        return payload

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
