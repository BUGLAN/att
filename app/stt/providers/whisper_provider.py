"""
Whisper provider 实现
"""
import asyncio
import platform
import time
from pathlib import Path
from typing import Optional

from faster_whisper import WhisperModel

from app.records.models import TranscriptionResult, TranscriptionSegment
from app.stt.base import ASRProvider
from app.stt.models import ASRCapabilities
from app.utils.logging import get_logger

logger = get_logger(__name__)


class WhisperProvider(ASRProvider):
    """faster-whisper 封装"""

    CHINESE_CONFIG = {
        "language": "zh",
        "task": "transcribe",
        "beam_size": 5,
        "best_of": 5,
        "temperature": 0.0,
        "condition_on_previous_text": False,
        "word_timestamps": True,
        "vad_filter": True,
        "vad_parameters": {
            "min_silence_duration_ms": 500,
            "speech_pad_ms": 200
        }
    }

    def __init__(
        self,
        model_size: str,
        device: str,
        compute_type: str,
        download_root: str
    ):
        self.model_size = model_size
        self.device = device
        self.compute_type = compute_type
        self.download_root = download_root

        if self.device == "auto" and platform.processor() == "arm":
            self.device = "coreml"
            logger.info("检测到 Apple Silicon，切换到 Core ML 兼容模式")

        self._model: Optional[WhisperModel] = None

        logger.info(
            "初始化 WhisperProvider",
            model=self.model_size,
            device=self.device,
            compute_type=self.compute_type,
            download_root=self.download_root
        )

    @property
    def provider_type(self) -> str:
        return "whisper"

    @property
    def model_name(self) -> str:
        return self.model_size

    def capabilities(self) -> ASRCapabilities:
        return ASRCapabilities(
            word_timestamps=True,
            vad=True,
            multi_language=True
        )

    def _load_model(self) -> WhisperModel:
        if self._model is None:
            logger.info("加载 Whisper 模型", model=self.model_size)
            start_time = time.time()

            if self.device == "coreml":
                self._model = WhisperModel(
                    self.model_size,
                    device="cpu",
                    compute_type="int8",
                    download_root=self.download_root
                )
            else:
                self._model = WhisperModel(
                    self.model_size,
                    device=self.device,
                    compute_type=self.compute_type,
                    download_root=self.download_root
                )

            logger.info(
                "Whisper 模型加载完成",
                model=self.model_size,
                device=self.device,
                load_time=f"{time.time() - start_time:.2f}s"
            )

        return self._model

    def _transcribe_sync(
        self,
        model: WhisperModel,
        audio_path: str,
        config: dict,
        language: str
    ) -> tuple[list[TranscriptionSegment], float, float, str, float]:
        start_time = time.time()
        segments_gen, info = model.transcribe(audio_path, **config)

        segments = []
        for segment in segments_gen:
            segments.append(TranscriptionSegment(
                start=segment.start,
                end=segment.end,
                text=segment.text.strip(),
                confidence=getattr(segment, "avg_logprob", 0.0),
                language=language
            ))

        return (
            segments,
            time.time() - start_time,
            info.duration,
            info.language,
            info.language_probability
        )

    async def transcribe(
        self,
        audio_path: str | Path,
        language: str = "zh",
        **kwargs
    ) -> TranscriptionResult:
        model = self._load_model()
        config = {**self.CHINESE_CONFIG, "language": language, **kwargs}

        logger.info(
            "开始 Whisper 转写",
            audio_path=str(audio_path),
            language=language,
            model=self.model_size
        )

        segments, processing_time, duration, detected_language, language_probability = await asyncio.to_thread(
            self._transcribe_sync,
            model,
            str(audio_path),
            config,
            language
        )

        logger.info(
            "Whisper 转写完成",
            audio_path=str(audio_path),
            duration=duration,
            processing_time=f"{processing_time:.2f}s",
            segments=len(segments)
        )

        return TranscriptionResult(
            segments=segments,
            language=detected_language,
            language_probability=language_probability,
            duration=duration,
            processing_time=processing_time
        )

    async def warmup(self) -> None:
        await asyncio.to_thread(self._load_model)

    def unload_model(self) -> None:
        if self._model is not None:
            del self._model
            self._model = None
            logger.info("Whisper 模型已卸载")

    async def close(self) -> None:
        self.unload_model()
