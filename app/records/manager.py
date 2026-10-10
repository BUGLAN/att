"""
Record manager for JSON-based storage
"""
import logging
from datetime import datetime
from pathlib import Path
from typing import List, Optional

from app.repositories.json_store import (
    get_async_lock,
    read_json_lenient_async,
    write_json_atomic_async,
)
from app.repositories.paths import is_safe_identifier, is_within_directory

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
        """Get JSON file path for a record

        record_id 只允许单层安全文件名：含分隔符（含 %5C 解码后的反斜杠）或穿越写法
        一律拒绝，避免 `DELETE /api/records/..%5C..%5Ctmp%5Cx` 之类的越界删除。
        """
        if not is_safe_identifier(record_id):
            raise ValueError(f"非法 record_id（含路径分隔符或穿越写法）：{record_id!r}")
        return self.records_path / f"{record_id}.json"

    def _is_safe_audio_path(self, audio_path: str) -> bool:
        """判断音频路径是否位于本管理器的 data/audio/records/ 内。"""
        return is_within_directory(audio_path, self.audio_path)

    async def _load_json(self, record_id: str) -> Optional[dict]:
        """Load JSON data for a record（非法 ID 或损坏文件按不存在处理）"""
        try:
            file_path = self._get_record_file(record_id)
        except ValueError as error:
            logger.warning(f"拒绝非法 record_id: {error}")
            return None

        if not file_path.exists():
            return None

        data = await read_json_lenient_async(file_path, default=None)
        if data is None:
            logger.warning(f"记录文件无法解析，按不存在处理: {file_path}")
            return None
        return data

    async def _save_json(self, record: Record) -> None:
        """Save record to JSON file（原子写，调用方需自行持有资源锁）"""
        file_path = self._get_record_file(record.id)
        record.updated_at = datetime.now()
        await write_json_atomic_async(file_path, record.to_dict())

    async def create_record(
        self,
        audio_path: str,
        duration: float,
        status: RecordStatus = RecordStatus.PENDING,
        raw_text: str = "",
        optimized_text: str = "",
        processing_time: float = 0.0,
        is_modified: bool = False,
        is_favorite: bool = False
    ) -> Record:
        """Create a new record"""
        record = Record(
            audio_path=audio_path,
            duration=duration,
            raw_text=raw_text,
            optimized_text=optimized_text,
            status=status,
            processing_time=processing_time,
            is_modified=is_modified,
            is_favorite=is_favorite
        )
        async with get_async_lock(self.records_path):
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
        processing_time: Optional[float] = None,
        is_modified: Optional[bool] = None,
        is_favorite: Optional[bool] = None,
        expected_revision: Optional[int] = None
    ) -> Optional[Record]:
        """Update a record

        :param expected_revision: 可选的乐观锁版本号。传入时会与当前记录的
            ``revision`` 比对，不一致则返回 ``None``（调用方应转成 409）；
            传入 ``None``（默认）表示不校验，保持既有行为不变。
        """
        # 读-改-写整体持锁：避免并发更新互相覆盖（静默丢更新）
        async with get_async_lock(self.records_path):
            record = await self.get_record(record_id)
            if not record:
                return None

            # 乐观锁：即使有第二个写者（如 Docker 容器绕过单写者锁），
            # 也只会得到明确的冲突，而不是静默丢更新。
            if expected_revision is not None and record.revision != expected_revision:
                # 注意：本模块用的是标准 logging（不是 structlog），
                # 只支持 %-风格的位置参数，不能传关键字参数，否则会抛 TypeError。
                logger.warning(
                    "记录版本冲突，已拒绝更新: record_id=%s expected=%s actual=%s",
                    record_id,
                    expected_revision,
                    record.revision,
                )
                return None

            if raw_text is not None:
                record.raw_text = raw_text
            if optimized_text is not None:
                record.optimized_text = optimized_text
            if status is not None:
                record.status = status
            if processing_time is not None:
                record.processing_time = processing_time
            if is_modified is not None:
                record.is_modified = is_modified
            if is_favorite is not None:
                record.is_favorite = is_favorite

            # 每次成功写入自增版本号
            record.revision += 1

            await self._save_json(record)
            return record

    async def set_error_message(self, record_id: str, message: str) -> None:
        """写入/清除记录的错误信息（走原子写 + 资源锁）。

        用途：优化或转写失败时把原因持久化到记录 JSON。
        旧实现只通过 502/WebSocket 传递失败原因，用户重启后就看不到历史，
        这里补上「失败原因可追溯」的存储位（``Record.error`` 字段）。

        - ``message`` 为空串表示清除错误；
        - 记录不存在或 ``record_id`` 非法时静默返回，不抛异常；
        - 读-改-写整体持 ``records_path`` 资源锁，避免与其他更新互相覆盖。

        :param record_id: 记录 ID（必须是单层安全文件名）
        :param message: 错误原因文本；空串表示清除
        """
        async with get_async_lock(self.records_path):
            record = await self.get_record(record_id)
            if not record:
                logger.warning(f"设置错误信息失败，记录不存在: {record_id}")
                return

            record.error = message or ""
            await self._save_json(record)

    async def search_records(
        self,
        q: Optional[str] = None,
        search_type: str = "all",
        favorite_only: bool = False,
        limit: int = 100
    ) -> List[Record]:
        """Search records by text content and filters."""
        records = []
        for json_file in self.records_path.glob("*.json"):
            try:
                data = await self._load_json(json_file.stem)
                if data:
                    records.append(Record.from_dict(data))
            except Exception as e:
                logger.error(f"Error loading record {json_file}: {e}")

        normalized_query = (q or "").strip().lower()
        normalized_search_type = (search_type or "all").lower()
        if normalized_search_type not in {"all", "raw", "optimized"}:
            normalized_search_type = "all"

        def matches(record: Record) -> bool:
            if favorite_only and not record.is_favorite:
                return False

            if not normalized_query:
                return True

            raw_text = (record.raw_text or "").lower()
            optimized_text = (record.optimized_text or "").lower()

            if normalized_search_type == "raw":
                return normalized_query in raw_text
            if normalized_search_type == "optimized":
                return normalized_query in optimized_text
            return normalized_query in raw_text or normalized_query in optimized_text

        filtered = [record for record in records if matches(record)]
        filtered.sort(key=lambda r: r.created_at, reverse=True)
        return filtered[:limit]

    async def delete_record(self, record_id: str) -> bool:
        """Delete a record and its associated audio file

        安全约束：record_id 必须是单层安全文件名；audio_path 只有解析后落在
        data/audio/records/ 内才会被 unlink，越界路径一律跳过并记 warning。
        """
        try:
            self._get_record_file(record_id)
        except ValueError as error:
            logger.warning(f"拒绝越界删除记录: {error}")
            return False

        async with get_async_lock(self.records_path):
            record = await self.get_record(record_id)
            if not record:
                return False

            # Delete audio file
            if record.audio_path:
                if self._is_safe_audio_path(record.audio_path):
                    audio_file = Path(record.audio_path)
                    if audio_file.exists() and audio_file.is_file():
                        audio_file.unlink()
                        logger.info(f"Deleted audio file: {audio_file}")
                else:
                    logger.warning(
                        f"audio_path 越界，已跳过删除: record_id={record_id}, audio_path={record.audio_path!r}"
                    )

            # Delete JSON file
            json_file = self._get_record_file(record_id)
            if json_file.exists():
                json_file.unlink()
                logger.info(f"Deleted record: {record_id}")

            return True

    def get_audio_file_path(self, record_id: str) -> Path:
        """Get the expected audio file path for a record"""
        if not is_safe_identifier(record_id):
            raise ValueError(f"非法 record_id（含路径分隔符或穿越写法）：{record_id!r}")
        return self.audio_path / f"{record_id}.wav"

    def get_audio_url(self, record_id: str) -> str:
        """Get the API URL for audio playback"""
        return f"/api/records/{record_id}/audio"
