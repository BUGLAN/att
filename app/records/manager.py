"""
Record manager for JSON-based storage
"""
import json
import aiofiles
from pathlib import Path
from datetime import datetime
from typing import Optional, List
import logging

from .models import Record, RecordStatus

logger = logging.getLogger(__name__)


class RecordManager:
    """Manages records using JSON files"""

    def __init__(self, data_path: Path):
        self.data_path = data_path
        self.records_path = data_path / "records"
        self.audio_path = data_path / "audio" / "records"

    async def initialize(self) -> None:
        """Ensure directories exist"""
        self.records_path.mkdir(parents=True, exist_ok=True)
        self.audio_path.mkdir(parents=True, exist_ok=True)
        logger.info(f"RecordManager initialized: records={self.records_path}, audio={self.audio_path}")

    def _get_record_file(self, record_id: str) -> Path:
        """Get JSON file path for a record"""
        return self.records_path / f"{record_id}.json"

    async def _load_json(self, record_id: str) -> Optional[dict]:
        """Load JSON data for a record"""
        file_path = self._get_record_file(record_id)
        if not file_path.exists():
            return None
        async with aiofiles.open(file_path, 'r', encoding='utf-8') as f:
            content = await f.read()
            return json.loads(content)

    async def _save_json(self, record: Record) -> None:
        """Save record to JSON file"""
        file_path = self._get_record_file(record.id)
        record.updated_at = datetime.now()
        async with aiofiles.open(file_path, 'w', encoding='utf-8') as f:
            await f.write(json.dumps(record.to_dict(), ensure_ascii=False, indent=2))

    async def create_record(
        self,
        audio_path: str,
        duration: float,
        status: RecordStatus = RecordStatus.PENDING
    ) -> Record:
        """Create a new record"""
        record = Record(
            audio_path=audio_path,
            duration=duration,
            status=status
        )
        await self._save_json(record)
        logger.info(f"Created record: {record.id}")
        return record

    async def get_record(self, record_id: str) -> Optional[Record]:
        """Get a single record by ID"""
        data = await self._load_json(record_id)
        if data:
            return Record.from_dict(data)
        return None

    async def list_records(
        self,
        limit: int = 100,
        status: Optional[RecordStatus] = None
    ) -> List[Record]:
        """List all records, sorted by created_at descending"""
        records = []
        for json_file in self.records_path.glob("*.json"):
            try:
                data = await self._load_json(json_file.stem)
                if data:
                    record = Record.from_dict(data)
                    if status is None or record.status == status:
                        records.append(record)
            except Exception as e:
                logger.error(f"Error loading record {json_file}: {e}")

        # Sort by created_at descending
        records.sort(key=lambda r: r.created_at, reverse=True)
        return records[:limit]

    async def update_record(
        self,
        record_id: str,
        raw_text: Optional[str] = None,
        optimized_text: Optional[str] = None,
        status: Optional[RecordStatus] = None,
        processing_time: Optional[float] = None
    ) -> Optional[Record]:
        """Update a record"""
        record = await self.get_record(record_id)
        if not record:
            return None

        if raw_text is not None:
            record.raw_text = raw_text
        if optimized_text is not None:
            record.optimized_text = optimized_text
        if status is not None:
            record.status = status
        if processing_time is not None:
            record.processing_time = processing_time

        await self._save_json(record)
        return record

    async def delete_record(self, record_id: str) -> bool:
        """Delete a record and its associated audio file"""
        record = await self.get_record(record_id)
        if not record:
            return False

        # Delete audio file
        if record.audio_path:
            audio_file = Path(record.audio_path)
            if audio_file.exists():
                audio_file.unlink()
                logger.info(f"Deleted audio file: {audio_file}")

        # Delete JSON file
        json_file = self._get_record_file(record_id)
        if json_file.exists():
            json_file.unlink()
            logger.info(f"Deleted record: {record_id}")

        return True

    def get_audio_file_path(self, record_id: str) -> Path:
        """Get the expected audio file path for a record"""
        return self.audio_path / f"{record_id}.wav"

    def get_audio_url(self, record_id: str) -> str:
        """Get the API URL for audio playback"""
        return f"/api/records/{record_id}/audio"
