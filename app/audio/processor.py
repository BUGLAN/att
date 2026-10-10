"""
Audio Processing Module
Handles: reception, validation, format conversion, chunking
"""
from pathlib import Path
from typing import Optional, List
import io
import uuid
import asyncio
import subprocess
from dataclasses import dataclass

from pydub import AudioSegment
import aiofiles

from app.config import settings
from app.records.models import AudioMetadata
from app.utils.logging import get_logger

logger = get_logger(__name__)


class AudioProcessor:
    """
    Core audio processing logic

    Supported formats: wav, mp3, m4a, flac, ogg, webm
    """

    SUPPORTED_FORMATS = {'wav', 'mp3', 'm4a', 'flac', 'ogg', 'webm'}
    MAX_FILE_SIZE = 500 * 1024 * 1024  # 500MB
    MIN_DURATION = 0.5  # 最短 0.5 秒
    TARGET_SAMPLE_RATE = 16000  # whisper optimal
    TARGET_CHANNELS = 1  # mono

    def __init__(self, storage_path: Optional[Path] = None):
        self.storage_path = storage_path or settings.audio_temp_path
        self.permanent_path = settings.audio_permanent_path

        # Ensure directories exist
        self.storage_path.mkdir(parents=True, exist_ok=True)
        self.permanent_path.mkdir(parents=True, exist_ok=True)

    async def validate_audio(
        self,
        file_content: bytes,
        filename: str
    ) -> AudioMetadata:
        """
        Validate audio file and extract metadata

        Args:
            file_content: Raw audio file bytes
            filename: Original filename

        Returns:
            AudioMetadata object

        Raises:
            ValueError: If file format not supported or file corrupted
        """
        extension = Path(filename).suffix.lower().lstrip('.')

        if extension not in self.SUPPORTED_FORMATS:
            raise ValueError(
                f"Unsupported format: {extension}. "
                f"Supported formats: {', '.join(self.SUPPORTED_FORMATS)}"
            )

        if len(file_content) > self.MAX_FILE_SIZE:
            raise ValueError(
                f"File too large: {len(file_content)} bytes. "
                f"Max: {self.MAX_FILE_SIZE} bytes"
            )

        if len(file_content) == 0:
            raise ValueError("Empty file")

        # 对于 webm/ogg 格式，faster-whisper 可以直接处理
        # 跳过 pydub 验证（pydub 无法从内存管道读取 webm）
        if extension in {'webm', 'ogg'}:
            # 尝试用 ffprobe 获取时长，失败则跳过时长检查
            duration = 0.0

            # 保存临时文件用于 ffprobe
            temp_path = self.storage_path / f"temp_{uuid.uuid4().hex[:8]}.{extension}"
            try:
                async with aiofiles.open(temp_path, 'wb') as f:
                    await f.write(file_content)

                duration = await self._get_duration_ffprobe(temp_path)

                logger.info(
                    "Webm/Ogg audio validated",
                    filename=filename,
                    duration=duration,
                    format=extension,
                    file_size=len(file_content)
                )

            except Exception as e:
                logger.warning(
                    "Webm/Ogg validation skipped",
                    filename=filename,
                    error=str(e)
                )
            finally:
                # 清理临时文件
                if 'temp_path' in dir() and temp_path.exists():
                    temp_path.unlink()

            # 检查最短时长（仅在成功获取时长时检查）
            if duration > 0 and duration < self.MIN_DURATION:
                raise ValueError(
                    f"音频时间过短（{duration:.1f}秒），至少需要 {self.MIN_DURATION} 秒"
                )

            return AudioMetadata(
                filename=filename,
                format=extension,
                duration=duration,
                sample_rate=16000,
                channels=1,
                file_size=len(file_content)
            )

        # 其他格式使用 pydub 验证
        audio = await asyncio.to_thread(
            AudioSegment.from_file,
            io.BytesIO(file_content)
        )

        metadata = AudioMetadata(
            filename=filename,
            format=extension,
            duration=len(audio) / 1000.0,  # ms to seconds
            sample_rate=audio.frame_rate,
            channels=audio.channels,
            file_size=len(file_content)
        )

        # 检查最短时长
        if metadata.duration < self.MIN_DURATION:
            raise ValueError(
                f"音频时间过短（{metadata.duration:.1f}秒），至少需要 {self.MIN_DURATION} 秒"
            )

        logger.info(
            "Audio validated",
            filename=filename,
            duration=metadata.duration,
            format=metadata.format
        )

        return metadata

    async def _get_duration_ffprobe(self, file_path: Path) -> float:
        """
        使用 ffprobe 获取音频时长

        Args:
            file_path: 音频文件路径

        Returns:
            音频时长（秒），失败返回 0.0
        """
        def run_ffprobe():
            result = subprocess.run(
                [
                    'ffprobe', '-v', 'error',
                    '-show_entries', 'format=duration',
                    '-of', 'default=noprint_wrappers=1:nokey=1',
                    str(file_path)
                ],
                capture_output=True,
                text=True
            )

            output = result.stdout.strip()

            # 检查是否为有效数字
            if not output or output == 'N/A':
                logger.warning(
                    "ffprobe returned invalid duration",
                    output=output,
                    stderr=result.stderr
                )
                return 0.0

            try:
                return float(output)
            except ValueError:
                logger.warning(
                    "ffprobe returned non-numeric value",
                    output=output
                )
                return 0.0

        return await asyncio.to_thread(run_ffprobe)

    async def save_as_wav_for_record(
        self,
        file_content: bytes,
        filename: str,
        record_id: str,
        records_path: Path
    ) -> tuple[Path, AudioMetadata]:
        """
        Convert and save audio as WAV for a specific record

        Args:
            file_content: Raw audio file bytes
            filename: Original filename
            record_id: Record ID for naming
            records_path: Path to records audio directory

        Returns:
            Tuple of (output_path, metadata)
        """
        # Ensure directory exists
        records_path.mkdir(parents=True, exist_ok=True)

        extension = Path(filename).suffix.lower().lstrip('.')

        # For webm/ogg, need to save temp file first
        if extension in {'webm', 'ogg'}:
            temp_path = self.storage_path / f"temp_record_{record_id}.{extension}"
            async with aiofiles.open(temp_path, 'wb') as f:
                await f.write(file_content)

            try:
                # Load from file
                audio = await asyncio.to_thread(
                    AudioSegment.from_file,
                    str(temp_path)
                )

                # Convert
                audio = audio.set_frame_rate(self.TARGET_SAMPLE_RATE)
                audio = audio.set_channels(self.TARGET_CHANNELS)

                # Export
                output_path = records_path / f"{record_id}.wav"

                await asyncio.to_thread(
                    audio.export,
                    str(output_path),
                    format="wav"
                )

                metadata = AudioMetadata(
                    filename=filename,
                    format="wav",
                    duration=len(audio) / 1000.0,
                    sample_rate=self.TARGET_SAMPLE_RATE,
                    channels=self.TARGET_CHANNELS,
                    file_size=output_path.stat().st_size
                )

                logger.info(
                    "Audio converted and saved for record",
                    record_id=record_id,
                    output_path=str(output_path),
                    duration=metadata.duration
                )

                return output_path, metadata

            finally:
                if temp_path.exists():
                    temp_path.unlink()

        # Other formats - convert directly from memory
        audio = await asyncio.to_thread(
            AudioSegment.from_file,
            io.BytesIO(file_content)
        )

        # Convert to optimal format
        audio = audio.set_frame_rate(self.TARGET_SAMPLE_RATE)
        audio = audio.set_channels(self.TARGET_CHANNELS)

        # Export
        output_path = records_path / f"{record_id}.wav"

        await asyncio.to_thread(
            audio.export,
            str(output_path),
            format="wav"
        )

        metadata = AudioMetadata(
            filename=filename,
            format="wav",
            duration=len(audio) / 1000.0,
            sample_rate=self.TARGET_SAMPLE_RATE,
            channels=self.TARGET_CHANNELS,
            file_size=output_path.stat().st_size
        )

        logger.info(
            "Audio converted and saved for record",
            record_id=record_id,
            output_path=str(output_path),
            duration=metadata.duration
        )

        return output_path, metadata
