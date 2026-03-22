"""
Data models for the application
"""
from dataclasses import dataclass, field
from datetime import datetime
from typing import Optional
from enum import Enum
import uuid


class FragmentStatus(str, Enum):
    """Fragment processing status"""
    PENDING = "pending"
    TRANSCRIBING = "transcribing"
    TRANSCRIBED = "transcribed"
    OPTIMIZING = "optimizing"
    OPTIMIZED = "optimized"
    FAILED = "failed"


class ExportFormat(str, Enum):
    """Export format options"""
    TXT = "txt"
    JSON = "json"
    SRT = "srt"


@dataclass
class AudioMetadata:
    """Audio file metadata"""
    filename: str
    format: str
    duration: float  # seconds
    sample_rate: int
    channels: int
    file_size: int  # bytes


@dataclass
class AudioChunk:
    """Represents a chunked audio segment"""
    chunk_id: str
    file_path: str
    start_time: float  # seconds
    end_time: float
    duration: float
    sequence: int


@dataclass
class TranscriptionSegment:
    """Single transcription segment from whisper"""
    start: float
    end: float
    text: str
    confidence: float = 1.0
    language: str = "zh"


@dataclass
class TranscriptionResult:
    """Complete transcription result"""
    segments: list[TranscriptionSegment]
    language: str
    language_probability: float
    duration: float
    processing_time: float

    @property
    def text(self) -> str:
        """Get full text from segments"""
        return " ".join(seg.text for seg in self.segments)


@dataclass
class Fragment:
    """Fragment data model for database"""
    id: str = field(default_factory=lambda: str(uuid.uuid4()))
    session_id: str = ""
    audio_path: str = ""
    raw_text: str = ""
    optimized_text: str = ""
    start_time: float = 0.0
    end_time: float = 0.0
    created_at: datetime = field(default_factory=datetime.now)
    updated_at: datetime = field(default_factory=datetime.now)
    status: FragmentStatus = FragmentStatus.PENDING
    metadata: dict = field(default_factory=dict)

    def to_dict(self) -> dict:
        """Convert to dictionary for database storage"""
        return {
            "id": self.id,
            "session_id": self.session_id,
            "audio_path": self.audio_path,
            "raw_text": self.raw_text,
            "optimized_text": self.optimized_text,
            "start_time": self.start_time,
            "end_time": self.end_time,
            "created_at": self.created_at.isoformat(),
            "updated_at": self.updated_at.isoformat(),
            "status": self.status.value,
            "metadata": self.metadata
        }

    @classmethod
    def from_dict(cls, data: dict) -> "Fragment":
        """Create from dictionary"""
        return cls(
            id=data["id"],
            session_id=data["session_id"],
            audio_path=data["audio_path"],
            raw_text=data["raw_text"],
            optimized_text=data["optimized_text"],
            start_time=data["start_time"],
            end_time=data["end_time"],
            created_at=datetime.fromisoformat(data["created_at"]),
            updated_at=datetime.fromisoformat(data["updated_at"]),
            status=FragmentStatus(data["status"]),
            metadata=data.get("metadata", {})
        )


@dataclass
class Session:
    """Processing session model"""
    id: str = field(default_factory=lambda: str(uuid.uuid4()))
    name: str = ""
    created_at: datetime = field(default_factory=datetime.now)
    updated_at: datetime = field(default_factory=datetime.now)
    total_duration: float = 0.0
    fragment_count: int = 0
    status: str = "active"

    def to_dict(self) -> dict:
        return {
            "id": self.id,
            "name": self.name,
            "created_at": self.created_at.isoformat(),
            "updated_at": self.updated_at.isoformat(),
            "total_duration": self.total_duration,
            "fragment_count": self.fragment_count,
            "status": self.status
        }
