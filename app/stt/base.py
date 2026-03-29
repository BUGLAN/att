"""
ASR 抽象基类
"""
from abc import ABC, abstractmethod
from pathlib import Path

from app.records.models import AudioChunk, TranscriptionResult
from app.stt.models import ASRCapabilities, ASRRuntimeCapabilities


class ASRProvider(ABC):
    """统一的 ASR provider 接口"""

    @property
    @abstractmethod
    def provider_type(self) -> str:
        """provider 类型标识"""

    @property
    @abstractmethod
    def model_name(self) -> str:
        """当前 provider 使用的模型名"""

    def capabilities(self) -> ASRCapabilities:
        """返回 provider 能力声明"""
        return ASRCapabilities()

    async def get_runtime_capabilities(
        self,
        recording_mode: str = "single"
    ) -> ASRRuntimeCapabilities:
        """返回当前录音模式下的运行时能力。"""
        if recording_mode == "streaming":
            if not self.capabilities().streaming:
                return ASRRuntimeCapabilities(
                    model_service_supported=False,
                    model_service_reason="当前 provider 不支持流式录音",
                    streaming_supported=False,
                    streaming_available=False,
                    streaming_reason="当前 provider 不支持流式录音",
                )

            return ASRRuntimeCapabilities(
                model_service_supported=True,
                streaming_supported=True,
                streaming_available=False,
                streaming_reason="流式运行时尚未就绪",
            )

        return ASRRuntimeCapabilities(
            model_service_supported=True,
            model_service_mode="provider",
            streaming_supported=False,
            streaming_available=False,
            streaming_reason="当前录音模式不是流式",
        )

    @abstractmethod
    async def transcribe(
        self,
        audio_path: str | Path,
        language: str = "zh",
        **kwargs
    ) -> TranscriptionResult:
        """转写单个音频文件"""

    async def transcribe_chunk(
        self,
        chunk: AudioChunk,
        language: str = "zh",
        **kwargs
    ) -> TranscriptionResult:
        """转写单个分块并修正时间戳"""
        result = await self.transcribe(
            chunk.file_path,
            language=language,
            **kwargs
        )

        for segment in result.segments:
            segment.start += chunk.start_time
            segment.end += chunk.start_time

        return result

    async def transcribe_chunks(
        self,
        chunks: list[AudioChunk],
        language: str = "zh",
        **kwargs
    ) -> TranscriptionResult:
        """顺序转写多个分块并合并结果"""
        all_segments = []
        total_processing_time = 0.0
        detected_language = language
        language_probability = 1.0
        total_duration = 0.0

        for chunk in chunks:
            result = await self.transcribe_chunk(
                chunk,
                language=language,
                **kwargs
            )
            all_segments.extend(result.segments)
            total_processing_time += result.processing_time
            total_duration = max(total_duration, chunk.end_time)

            if detected_language == language and result.language:
                detected_language = result.language
                language_probability = result.language_probability

        merged_segments = self._merge_overlapping_segments(all_segments)

        return TranscriptionResult(
            segments=merged_segments,
            language=detected_language,
            language_probability=language_probability,
            duration=total_duration,
            processing_time=total_processing_time
        )

    def _merge_overlapping_segments(
        self,
        segments,
        min_gap: float = 0.5
    ):
        """合并重叠分段"""
        if not segments:
            return segments

        sorted_segments = sorted(segments, key=lambda segment: segment.start)
        merged = [sorted_segments[0]]

        for segment in sorted_segments[1:]:
            last = merged[-1]
            if segment.start < last.end - min_gap:
                if segment.end > last.end:
                    merged[-1] = segment
            else:
                merged.append(segment)

        return merged

    async def warmup(self) -> None:
        """预热模型"""

    async def close(self) -> None:
        """释放资源"""
