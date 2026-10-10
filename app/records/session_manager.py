"""
Recording Session Manager
Manages multi-segment recording sessions
"""
import shutil
import wave
from datetime import datetime
from pathlib import Path
from typing import Optional, List
import uuid

from app.records.models import (
    AudioSegment,
    RecordingSession,
    RecordingMode,
    RecordStatus
)
from app.config import settings
from app.repositories.json_store import (
    get_async_lock,
    read_json_lenient_async,
    write_json_atomic_async,
)
from app.repositories.paths import is_safe_identifier, is_within_directory
from app.utils.logging import get_logger

logger = get_logger(__name__)


class SessionManager:
    """Manages recording sessions with JSON storage."""

    def __init__(self, data_dir: Optional[str] = None):
        self.data_dir = Path(data_dir or settings.data_path) / "sessions"
        # 会话音频（含合并结果）统一存放目录，删除时以此为安全边界
        self.audio_dir = self.data_dir.parent / "audio" / "records"
        self._ensure_data_dir()

    def _ensure_data_dir(self) -> None:
        """Ensure data directory exists."""
        self.data_dir.mkdir(parents=True, exist_ok=True)

    def _get_session_file(self, session_id: str) -> Path:
        """Get session file path（session_id 必须是单层安全文件名）。"""
        if not is_safe_identifier(session_id):
            raise ValueError(f"非法 session_id（含路径分隔符或穿越写法）：{session_id!r}")
        return self.data_dir / f"{session_id}.json"

    def _is_safe_audio_path(self, audio_path: str) -> bool:
        """音频删除范围限定在 data/audio/records/ 内。"""
        return is_within_directory(audio_path, self.audio_dir)

    async def create_session(self) -> RecordingSession:
        """Create a new recording session."""
        session = RecordingSession(
            mode=RecordingMode.MULTI,
            status="recording",
            created_at=datetime.now()
        )

        async with get_async_lock(self.data_dir):
            await self._save_session(session)

        logger.info("Created recording session", session_id=session.id)
        return session

    async def get_session(self, session_id: str) -> Optional[RecordingSession]:
        """Get session by ID."""
        try:
            file_path = self._get_session_file(session_id)
        except ValueError as error:
            logger.warning("拒绝非法 session_id", error=str(error))
            return None

        if not file_path.exists():
            return None

        data = await read_json_lenient_async(file_path, default=None)
        if data is None:
            logger.error("会话文件无法解析", session_id=session_id)
            return None

        try:
            return RecordingSession.from_dict(data)
        except Exception as e:
            logger.error(
                "Failed to load session",
                session_id=session_id,
                error=str(e)
            )
            return None

    async def _save_session(self, session: RecordingSession) -> None:
        """Save session to file（原子写；调用方需自行持有资源锁）。"""
        file_path = self._get_session_file(session.id)
        try:
            await write_json_atomic_async(file_path, session.to_dict())
        except Exception as e:
            logger.error(
                "Failed to save session",
                session_id=session.id,
                error=str(e)
            )
            raise

    async def add_segment(
        self,
        session_id: str,
        audio_path: str,
        duration: float
    ) -> AudioSegment:
        """Add a segment to the session."""
        # 读-改-写整体持锁，避免并发加片段互相覆盖
        async with get_async_lock(self.data_dir):
            session = await self.get_session(session_id)
            if not session:
                raise ValueError(f"Session not found: {session_id}")

            segment = AudioSegment(
                id=str(uuid.uuid4()),
                session_id=session_id,
                sequence=len(session.segments),
                audio_path=audio_path,
                duration=duration,
                start_time=datetime.now(),
                end_time=datetime.now(),
                status=RecordStatus.PENDING
            )

            session.segments.append(segment)
            session.total_duration += duration

            await self._save_session(session)

        logger.info(
            "Added segment to session",
            session_id=session_id,
            segment_id=segment.id,
            sequence=segment.sequence,
            duration=duration
        )

        return segment

    async def update_segment(
        self,
        session_id: str,
        segment_id: str,
        **kwargs
    ) -> Optional[AudioSegment]:
        """Update segment status and text."""
        async with get_async_lock(self.data_dir):
            session = await self.get_session(session_id)
            if not session:
                return None

            for segment in session.segments:
                if segment.id == segment_id:
                    for key, value in kwargs.items():
                        if hasattr(segment, key):
                            setattr(segment, key, value)

                    await self._save_session(session)
                    return segment

            return None

    def _build_merged_audio(self, session: RecordingSession) -> str:
        """将多段录音拼接成一份可播放的 WAV 文件（无有效片段时返回空字符串）。"""
        valid_segments = []
        for segment in sorted(session.segments, key=lambda item: item.sequence):
            audio_path = Path(segment.audio_path)
            if audio_path.exists():
                valid_segments.append(audio_path)

        if not valid_segments:
            return ""

        output_path = self.audio_dir / f"{session.id}.wav"
        output_path.parent.mkdir(parents=True, exist_ok=True)

        if len(valid_segments) == 1:
            shutil.copyfile(valid_segments[0], output_path)
            return str(output_path)

        with wave.open(str(valid_segments[0]), "rb") as first_reader:
            params = first_reader.getparams()
            format_signature = (
                first_reader.getnchannels(),
                first_reader.getsampwidth(),
                first_reader.getframerate(),
                first_reader.getcomptype(),
                first_reader.getcompname()
            )
            frames = [first_reader.readframes(first_reader.getnframes())]

        for audio_path in valid_segments[1:]:
            with wave.open(str(audio_path), "rb") as reader:
                current_signature = (
                    reader.getnchannels(),
                    reader.getsampwidth(),
                    reader.getframerate(),
                    reader.getcomptype(),
                    reader.getcompname()
                )
                if current_signature != format_signature:
                    raise ValueError(f"Segment format mismatch: {audio_path}")
                frames.append(reader.readframes(reader.getnframes()))

        with wave.open(str(output_path), "wb") as writer:
            writer.setparams(params)
            for frame in frames:
                writer.writeframes(frame)

        return str(output_path)

    async def complete_session(self, session_id: str) -> Optional[RecordingSession]:
        """Mark session as completed."""
        async with get_async_lock(self.data_dir):
            session = await self.get_session(session_id)
            if not session:
                return None

            session.status = "completed"
            session.completed_at = datetime.now()

            # Merge all segment texts
            session.merged_text = "\n\n".join(
                seg.raw_text for seg in session.segments if seg.raw_text
            )

            # 只有真的生成了合并音频才写路径：写空字符串会让音频接口把 "." 当文件读（500）
            merged_audio_path = self._build_merged_audio(session)
            if merged_audio_path:
                session.merged_audio_path = merged_audio_path

            await self._save_session(session)

        logger.info(
            "Completed recording session",
            session_id=session_id,
            total_segments=len(session.segments),
            total_duration=session.total_duration
        )

        return session

    async def delete_session(self, session_id: str) -> bool:
        """Delete a session and its audio files（音频删除限定在 data/audio/records/ 内）。"""
        try:
            file_path = self._get_session_file(session_id)
        except ValueError as error:
            logger.warning("拒绝越界删除会话", error=str(error))
            return False

        if not file_path.exists():
            return False

        async with get_async_lock(self.data_dir):
            try:
                # Delete audio files
                session = await self.get_session(session_id)
                if session:
                    for segment in session.segments:
                        self._unlink_audio_if_safe(segment.audio_path, session_id)
                    if session.merged_audio_path:
                        self._unlink_audio_if_safe(session.merged_audio_path, session_id)

                # Delete session file
                file_path.unlink()

                logger.info("Deleted session", session_id=session_id)
                return True
            except Exception as e:
                logger.error(
                    "Failed to delete session",
                    session_id=session_id,
                    error=str(e)
                )
                return False

    def _unlink_audio_if_safe(self, audio_path: str, session_id: str) -> None:
        """仅在路径位于 data/audio/records/ 内且确实是文件时才删除。"""
        if not audio_path:
            return

        if not self._is_safe_audio_path(audio_path):
            logger.warning(
                "会话音频路径越界，已跳过删除",
                session_id=session_id,
                audio_path=audio_path
            )
            return

        candidate = Path(audio_path)
        if candidate.exists() and candidate.is_file():
            candidate.unlink()


# Global instance
session_manager = SessionManager()
