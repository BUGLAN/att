"""
Whisper provider 瀹炵幇
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
    """faster-whisper 灏佽"""

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
            logger.info("妫€娴嬪埌 Apple Silicon锛屽垏鎹㈠埌 Core ML 鍏煎妯″紡")

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

    def _looks_like_local_model_dir(self, path: Path) -> bool:
        return path.is_dir() and (path / "config.json").exists() and (path / "tokenizer.json").exists()

    def _resolve_model_source(self) -> str:
        direct_path = Path(self.model_size)
        if self._looks_like_local_model_dir(direct_path):
            return str(direct_path)

        download_root = Path(self.download_root)
        if self._looks_like_local_model_dir(download_root):
            return str(download_root)

        short_name = self.model_size.replace("\\", "/").split("/")[-1]
        candidates = [
            download_root / short_name,
            download_root / f"faster-whisper-{short_name}",
            download_root / f"Systran--faster-whisper-{short_name}",
            download_root / f"models--Systran--faster-whisper-{short_name}"
        ]
        for candidate in candidates:
            if self._looks_like_local_model_dir(candidate):
                return str(candidate)

        snapshot_root = download_root / f"models--Systran--faster-whisper-{short_name}" / "snapshots"
        if snapshot_root.is_dir():
            for snapshot_dir in sorted(snapshot_root.iterdir(), reverse=True):
                if self._looks_like_local_model_dir(snapshot_dir):
                    return str(snapshot_dir)

        return self.model_size

    def _load_model(self) -> WhisperModel:
        if self._model is None:
            resolved_model_source = self._resolve_model_source()
            is_local_model = resolved_model_source != self.model_size
            logger.info(
                "鍔犺浇 Whisper 妯″瀷",
                model=self.model_size,
                resolved_model_source=resolved_model_source,
                local_model=is_local_model
            )
            start_time = time.time()

            if self.device == "coreml":
                self._model = WhisperModel(
                    resolved_model_source,
                    device="cpu",
                    compute_type="int8",
                    download_root=self.download_root if not is_local_model else None
                )
            else:
                self._model = WhisperModel(
                    resolved_model_source,
                    device=self.device,
                    compute_type=self.compute_type,
                    download_root=self.download_root if not is_local_model else None
                )

            logger.info(
                "Whisper 妯″瀷鍔犺浇瀹屾垚",
                model=resolved_model_source,
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
            "Whisper 杞啓瀹屾垚",
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
