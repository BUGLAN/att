"""
Data models for records management
"""
from dataclasses import dataclass, field
from datetime import datetime
from typing import Optional
from enum import Enum
import uuid


class RecordStatus(str, Enum):
    """Record processing status"""
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
    segments: list
    language: str
    language_probability: float
    duration: float
    processing_time: float

    @property
    def text(self) -> str:
        """Get full text from segments"""
        return " ".join(seg.text for seg in self.segments)


@dataclass
class Record:
    """Record data model - one recording = one record"""
    id: str = field(default_factory=lambda: str(uuid.uuid4()))
    created_at: datetime = field(default_factory=datetime.now)
    updated_at: datetime = field(default_factory=datetime.now)
    audio_path: str = ""
    duration: float = 0.0
    raw_text: str = ""
    optimized_text: str = ""
    status: RecordStatus = RecordStatus.PENDING

    def to_dict(self) -> dict:
        """Convert to dictionary for JSON storage"""
        return {
            "id": self.id,
            "created_at": self.created_at.isoformat(),
            "updated_at": self.updated_at.isoformat(),
            "audio_path": self.audio_path,
            "duration": self.duration,
            "raw_text": self.raw_text,
            "optimized_text": self.optimized_text,
            "status": self.status.value
        }

    @classmethod
    def from_dict(cls, data: dict) -> "Record":
        """Create from dictionary"""
        return cls(
            id=data["id"],
            created_at=datetime.fromisoformat(data["created_at"]),
            updated_at=datetime.fromisoformat(data["updated_at"]),
            audio_path=data["audio_path"],
            duration=data["duration"],
            raw_text=data.get("raw_text", ""),
            optimized_text=data.get("optimized_text", ""),
            status=RecordStatus(data.get("status", "pending"))
        )
