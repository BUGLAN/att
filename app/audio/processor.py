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
from app.records.models import AudioMetadata, AudioChunk
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
    CHUNK_DURATION = 30  # seconds (optimal for faster-whisper)
    OVERLAP_DURATION = 1  # second overlap to avoid word boundary issues
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

    async def convert_to_wav(
        self,
        file_content: bytes,
        filename: str
    ) -> tuple[Path, AudioMetadata]:
        """
        Convert any supported format to WAV for whisper processing
        Output: 16kHz mono WAV (optimal for whisper)

        Args:
            file_content: Raw audio file bytes
            filename: Original filename (used for naming)

        Returns:
            Tuple of (output_path, metadata)
        """
        extension = Path(filename).suffix.lower().lstrip('.')

        # 对于 webm/ogg，需要先保存文件再转换
        if extension in {'webm', 'ogg'}:
            temp_path = self.storage_path / f"temp_convert_{uuid.uuid4().hex[:8]}.{extension}"
            async with aiofiles.open(temp_path, 'wb') as f:
                await f.write(file_content)

            try:
                # 从文件读取
                audio = await asyncio.to_thread(
                    AudioSegment.from_file,
                    str(temp_path)
                )

                # 转换
                audio = audio.set_frame_rate(self.TARGET_SAMPLE_RATE)
                audio = audio.set_channels(self.TARGET_CHANNELS)

                # 导出
                base_name = Path(filename).stem
                unique_id = str(uuid.uuid4())[:8]
                output_filename = f"{base_name}_{unique_id}.wav"
                output_path = self.storage_path / output_filename

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
                    "Webm/Ogg converted to WAV",
                    output_path=str(output_path),
                    duration=metadata.duration
                )

                return output_path, metadata

            finally:
                if temp_path.exists():
                    temp_path.unlink()

        # 其他格式直接从内存转换
        audio = await asyncio.to_thread(
            AudioSegment.from_file,
            io.BytesIO(file_content)
        )

        # Convert to optimal format
        audio = audio.set_frame_rate(self.TARGET_SAMPLE_RATE)
        audio = audio.set_channels(self.TARGET_CHANNELS)

        # Generate unique filename
        base_name = Path(filename).stem
        unique_id = str(uuid.uuid4())[:8]
        output_filename = f"{base_name}_{unique_id}.wav"
        output_path = self.storage_path / output_filename

        # Export to WAV
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
            "Audio converted to WAV",
            output_path=str(output_path),
            duration=metadata.duration
        )

        return output_path, metadata

    async def chunk_audio(
        self,
        wav_path: Path,
        chunk_duration: Optional[float] = None
    ) -> List[AudioChunk]:
        """
        Split audio into chunks for parallel processing

        Strategy:
        - Default chunk: 30 seconds (balance between accuracy and speed)
        - Overlap: 1 second to avoid word boundary issues

        Args:
            wav_path: Path to WAV file
            chunk_duration: Duration per chunk in seconds (default: 30)

        Returns:
            List of AudioChunk objects
        """
        chunk_duration = chunk_duration or self.CHUNK_DURATION

        # Load audio in thread pool
        audio = await asyncio.to_thread(
            AudioSegment.from_wav,
            str(wav_path)
        )

        total_duration = len(audio) / 1000.0  # ms to seconds
        chunks = []
        chunk_id_base = wav_path.stem

        # Calculate number of chunks with overlap
        effective_duration = chunk_duration - self.OVERLAP_DURATION
        num_chunks = max(1, int(total_duration // effective_duration) + 1)

        for i in range(num_chunks):
            start_ms = i * effective_duration * 1000
            end_ms = min(
                (i + 1) * chunk_duration * 1000,
                len(audio)
            )

            if start_ms >= len(audio):
                break

            chunk_audio = audio[start_ms:end_ms]

            # Skip very short chunks at the end
            if len(chunk_audio) < 500:  # Less than 0.5 seconds
                continue

            chunk_filename = f"{chunk_id_base}_chunk_{i}.wav"
            chunk_path = self.storage_path / chunk_filename

            await asyncio.to_thread(
                chunk_audio.export,
                str(chunk_path),
                format="wav"
            )

            chunks.append(AudioChunk(
                chunk_id=f"{chunk_id_base}_chunk_{i}",
                file_path=str(chunk_path),
                start_time=start_ms / 1000.0,
                end_time=end_ms / 1000.0,
                duration=len(chunk_audio) / 1000.0,
                sequence=i
            ))

        logger.info(
            "Audio chunked",
            total_duration=total_duration,
            num_chunks=len(chunks),
            chunk_duration=chunk_duration
        )

        return chunks

    async def save_permanent(
        self,
        file_content: bytes,
        filename: str
    ) -> Path:
        """
        Save audio file to permanent storage

        Args:
            file_content: Raw audio file bytes
            filename: Original filename

        Returns:
            Path to saved file
        """
        unique_id = str(uuid.uuid4())[:8]
        safe_filename = f"{unique_id}_{filename}"
        output_path = self.permanent_path / safe_filename

        async with aiofiles.open(output_path, 'wb') as f:
            await f.write(file_content)

        logger.info("Audio saved to permanent storage", path=str(output_path))
        return output_path

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

    async def cleanup_temp_files(self, chunk_ids: List[str]) -> None:
        """
        Clean up temporary audio files after processing

        Args:
            chunk_ids: List of chunk IDs to clean up
        """
        for chunk_id in chunk_ids:
            chunk_path = self.storage_path / f"{chunk_id}.wav"
            if chunk_path.exists():
                chunk_path.unlink()
                logger.debug("Cleaned up temp file", path=str(chunk_path))

    async def cleanup_file(self, file_path: Path) -> None:
        """Clean up a single file"""
        if file_path.exists():
            file_path.unlink()
            logger.debug("Cleaned up file", path=str(file_path))
