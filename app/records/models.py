"""
Data models for records management
"""
from dataclasses import dataclass, field
from datetime import datetime
from typing import Optional, List
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


class RecordingMode(str, Enum):
    """Recording mode options"""
    SINGLE = "single"      # 单段录音
    MULTI = "multi"        # 多段录音（静音分割）


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
    processing_time: float = 0.0  # Transcription processing time in seconds
    is_modified: bool = False  # Whether text has been manually edited

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
            "status": self.status.value,
            "processing_time": self.processing_time,
            "is_modified": self.is_modified
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
            status=RecordStatus(data.get("status", "pending")),
            processing_time=data.get("processing_time", 0.0),
            is_modified=data.get("is_modified", False)
        )


@dataclass
class AudioSegment:
    """录音片段（多段录音模式）"""
    id: str = field(default_factory=lambda: str(uuid.uuid4()))
    session_id: str = ""           # 所属会话ID
    sequence: int = 0              # 片段序号
    audio_path: str = ""           # 音频文件路径
    duration: float = 0.0          # 时长（秒）
    start_time: datetime = field(default_factory=datetime.now)
    end_time: datetime = field(default_factory=datetime.now)
    status: RecordStatus = RecordStatus.PENDING
    raw_text: str = ""             # 转写文本
    processing_time: float = 0.0

    def to_dict(self) -> dict:
        """Convert to dictionary for JSON storage"""
        return {
            "id": self.id,
            "session_id": self.session_id,
            "sequence": self.sequence,
            "audio_path": self.audio_path,
            "duration": self.duration,
            "start_time": self.start_time.isoformat(),
            "end_time": self.end_time.isoformat(),
            "status": self.status.value,
            "raw_text": self.raw_text,
            "processing_time": self.processing_time
        }

    @classmethod
    def from_dict(cls, data: dict) -> "AudioSegment":
        """Create from dictionary"""
        return cls(
            id=data["id"],
            session_id=data["session_id"],
            sequence=data["sequence"],
            audio_path=data["audio_path"],
            duration=data["duration"],
            start_time=datetime.fromisoformat(data["start_time"]),
            end_time=datetime.fromisoformat(data["end_time"]),
            status=RecordStatus(data.get("status", "pending")),
            raw_text=data.get("raw_text", ""),
            processing_time=data.get("processing_time", 0.0)
        )


@dataclass
class RecordingSession:
    """录音会话（多段模式）"""
    id: str = field(default_factory=lambda: str(uuid.uuid4()))
    mode: RecordingMode = RecordingMode.MULTI
    status: str = "recording"      # recording, paused, completed
    segments: List[AudioSegment] = field(default_factory=list)
    total_duration: float = 0.0
    merged_text: str = ""          # 合并后的完整文本
    created_at: datetime = field(default_factory=datetime.now)
    completed_at: Optional[datetime] = None

    def to_dict(self) -> dict:
        """Convert to dictionary for JSON storage"""
        return {
            "id": self.id,
            "mode": self.mode.value,
            "status": self.status,
            "segments": [s.to_dict() for s in self.segments],
            "total_duration": self.total_duration,
            "merged_text": self.merged_text,
            "created_at": self.created_at.isoformat(),
            "completed_at": self.completed_at.isoformat() if self.completed_at else None
        }

    @classmethod
    def from_dict(cls, data: dict) -> "RecordingSession":
        """Create from dictionary"""
        segments = [AudioSegment.from_dict(s) for s in data.get("segments", [])]
        completed_at = None
        if data.get("completed_at"):
            completed_at = datetime.fromisoformat(data["completed_at"])
        return cls(
            id=data["id"],
            mode=RecordingMode(data.get("mode", "multi")),
            status=data.get("status", "recording"),
            segments=segments,
            total_duration=data.get("total_duration", 0.0),
            merged_text=data.get("merged_text", ""),
            created_at=datetime.fromisoformat(data["created_at"]),
            completed_at=completed_at
        )
