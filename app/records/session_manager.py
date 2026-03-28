"""
Recording Session Manager
Manages multi-segment recording sessions
"""
import json
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
from app.utils.logging import get_logger

logger = get_logger(__name__)


class SessionManager:
    """Manages recording sessions with JSON storage."""

    def __init__(self, data_dir: Optional[str] = None):
        self.data_dir = Path(data_dir or settings.data_path) / "sessions"
        self._ensure_data_dir()

    def _ensure_data_dir(self) -> None:
        """Ensure data directory exists."""
        self.data_dir.mkdir(parents=True, exist_ok=True)

    def _get_session_file(self, session_id: str) -> Path:
        """Get session file path."""
        return self.data_dir / f"{session_id}.json"

    async def create_session(self) -> RecordingSession:
        """Create a new recording session."""
        session = RecordingSession(
            mode=RecordingMode.MULTI,
            status="recording",
            created_at=datetime.now()
        )

        await self._save_session(session)

        logger.info("Created recording session", session_id=session.id)
        return session

    async def get_session(self, session_id: str) -> Optional[RecordingSession]:
        """Get session by ID."""
        file_path = self._get_session_file(session_id)
        if not file_path.exists():
            return None

        try:
            with open(file_path, "r", encoding="utf-8") as f:
                data = json.load(f)
            return RecordingSession.from_dict(data)
        except Exception as e:
            logger.error(
                "Failed to load session",
                session_id=session_id,
                error=str(e)
            )
            return None

    async def _save_session(self, session: RecordingSession) -> None:
        """Save session to file."""
        file_path = self._get_session_file(session.id)
        try:
            with open(file_path, "w", encoding="utf-8") as f:
                json.dump(session.to_dict(), f, ensure_ascii=False, indent=2)
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
        """将多段录音拼接成一份可播放的 WAV 文件。"""
        valid_segments = []
        for segment in sorted(session.segments, key=lambda item: item.sequence):
            audio_path = Path(segment.audio_path)
            if audio_path.exists():
                valid_segments.append(audio_path)

        if not valid_segments:
            return ""

        output_path = settings.audio_records_path / f"{session.id}.wav"
        output_path.parent.mkdir(parents=True, exist_ok=True)

        if len(valid_segments) == 1:
            shutil.copyfile(valid_segments[0], output_path)
            return str(output_path)

        with wave.open(str(valid_segments[0]), "rb") as first_reader:
            params = first_reader.getparams()
            frames = [first_reader.readframes(first_reader.getnframes())]

        for audio_path in valid_segments[1:]:
            with wave.open(str(audio_path), "rb") as reader:
                current_params = reader.getparams()
                if current_params[:4] != params[:4]:
                    raise ValueError(f"Segment format mismatch: {audio_path}")
                frames.append(reader.readframes(reader.getnframes()))

        with wave.open(str(output_path), "wb") as writer:
            writer.setparams(params)
            for frame in frames:
                writer.writeframes(frame)

        return str(output_path)

    async def complete_session(self, session_id: str) -> Optional[RecordingSession]:
        """Mark session as completed."""
        session = await self.get_session(session_id)
        if not session:
            return None

        session.status = "completed"
        session.completed_at = datetime.now()

        # Merge all segment texts
        session.merged_text = "\n\n".join(
            seg.raw_text for seg in session.segments if seg.raw_text
        )
        session.merged_audio_path = self._build_merged_audio(session)

        await self._save_session(session)

        logger.info(
            "Completed recording session",
            session_id=session_id,
            total_segments=len(session.segments),
            total_duration=session.total_duration
        )

        return session

    async def delete_session(self, session_id: str) -> bool:
        """Delete a session and its audio files."""
        file_path = self._get_session_file(session_id)
        if not file_path.exists():
            return False

        try:
            # Delete audio files
            session = await self.get_session(session_id)
            if session:
                for segment in session.segments:
                    audio_path = Path(segment.audio_path)
                    if audio_path.exists():
                        audio_path.unlink()
                if session.merged_audio_path:
                    merged_audio_path = Path(session.merged_audio_path)
                    if merged_audio_path.exists():
                        merged_audio_path.unlink()

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


# Global instance
session_manager = SessionManager()
