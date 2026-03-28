"""
Qwen3-ASR provider 实现
"""
import asyncio
import time
from pathlib import Path
from typing import Any
import os

import numpy as np
import soundfile as sf

from app.records.models import TranscriptionResult, TranscriptionSegment
from app.stt.base import ASRProvider
from app.stt.models import ASRCapabilities, ASRProviderError
from app.utils.logging import get_logger

logger = get_logger(__name__)


class QwenASRProvider(ASRProvider):
    """Qwen3-ASR provider"""

    LANGUAGE_MAP = {
        "zh": "Chinese",
        "en": "English",
        "ja": "Japanese",
        "ko": "Korean",
        "yue": "Cantonese",
        "fr": "French",
        "de": "German",
        "es": "Spanish",
        "ru": "Russian",
        "auto": None
    }
    LANGUAGE_CODE_MAP = {
        value.lower(): key
        for key, value in LANGUAGE_MAP.items()
        if value
    }

    def __init__(
        self,
        model_name: str,
        device: str,
        dtype: str,
        cache_dir: str,
        max_new_tokens: int = 256,
        return_timestamps: bool = False,
        use_forced_aligner: bool = False,
        forced_aligner_model: str = "",
        max_audio_seconds: int = 30,
        streaming_backend: str = "auto",
        streaming_base_url: str = "",
        streaming_timeout_seconds: float = 30.0
    ):
        self._model_name = model_name
        self.device = device
        self.dtype = dtype
        self.cache_dir = cache_dir
        self.max_new_tokens = max_new_tokens
        self.return_timestamps = return_timestamps
        self.use_forced_aligner = use_forced_aligner
        self.forced_aligner_model = forced_aligner_model
        self.max_audio_seconds = max_audio_seconds
        self.streaming_backend = streaming_backend
        self.streaming_base_url = streaming_base_url
        self.streaming_timeout_seconds = streaming_timeout_seconds

        self._torch = None
        self._model = None
        self._model_class = None

        logger.info(
            "初始化 QwenASRProvider",
            model=self._model_name,
            device=self.device,
            dtype=self.dtype,
            cache_dir=self.cache_dir,
            return_timestamps=self.return_timestamps,
            use_forced_aligner=self.use_forced_aligner
        )

    @property
    def provider_type(self) -> str:
        return "qwen_asr"

    @property
    def model_name(self) -> str:
        return self._model_name

    def capabilities(self) -> ASRCapabilities:
        return ASRCapabilities(
            word_timestamps=self.return_timestamps,
            multi_language=True
        )

    def _load_dependencies(self) -> None:
        if self._model_class is not None and self._torch is not None:
            return

        try:
            import torch
            from qwen_asr import Qwen3ASRModel
        except ImportError as error:
            raise ASRProviderError(
                "Qwen3-ASR 依赖未安装，请先执行 `pip install qwen-asr>=0.0.6`"
            ) from error

        self._torch = torch
        self._model_class = Qwen3ASRModel

    def _resolve_device(self) -> str:
        self._load_dependencies()

        if self.device != "auto":
            return self.device

        if self._torch.cuda.is_available():
            return "cuda:0"

        return "cpu"

    def _resolve_dtype(self):
        self._load_dependencies()
        normalized_dtype = (self.dtype or "auto").lower()
        resolved_device = self._resolve_device()

        if normalized_dtype == "auto":
            if resolved_device.startswith("cuda"):
                return self._torch.bfloat16
            return self._torch.float32

        mapping = {
            "float16": self._torch.float16,
            "bfloat16": self._torch.bfloat16,
            "float32": self._torch.float32
        }
        return mapping.get(normalized_dtype, self._torch.float32)

    def _build_model_kwargs(self) -> dict[str, Any]:
        model_kwargs = {
            "dtype": self._resolve_dtype(),
            "device_map": self._resolve_device(),
            "cache_dir": self.cache_dir
        }

        if self.use_forced_aligner and self.forced_aligner_model:
            model_kwargs["forced_aligner_name_or_path"] = self._resolve_local_model_path(
                self.forced_aligner_model
            )

        return model_kwargs

    def _looks_like_local_model_dir(self, path: Path) -> bool:
        """判断目录是否像一个可直接加载的本地模型目录"""
        return path.is_dir() and (path / "config.json").exists()

    def _resolve_local_model_path(self, model_name_or_path: str) -> str:
        """优先解析项目内已下载好的本地模型目录，避免重复联网下载。"""
        if not model_name_or_path:
            return model_name_or_path

        direct_path = Path(model_name_or_path)
        if self._looks_like_local_model_dir(direct_path):
            return str(direct_path)

        short_name = model_name_or_path.replace("\\", "/").split("/")[-1]
        candidate = Path(self.cache_dir) / short_name
        if self._looks_like_local_model_dir(candidate):
            return str(candidate)

        return model_name_or_path

    def _load_model(self):
        self._load_dependencies()

        if self._model is None:
            Path(self.cache_dir).mkdir(parents=True, exist_ok=True)
            os.environ.setdefault("HF_HUB_DISABLE_SYMLINKS_WARNING", "1")
            resolved_model_path = self._resolve_local_model_path(self._model_name)
            is_local_model = resolved_model_path != self._model_name
            logger.info(
                "加载 Qwen3-ASR 模型",
                model=self._model_name,
                resolved_model_path=resolved_model_path,
                local_model=is_local_model,
                device=self._resolve_device(),
                dtype=self.dtype,
                cache_dir=self.cache_dir
            )
            start_time = time.time()
            self._model = self._model_class.from_pretrained(
                resolved_model_path,
                local_files_only=is_local_model,
                **self._build_model_kwargs()
            )
            logger.info(
                "Qwen3-ASR 模型加载完成",
                model=resolved_model_path,
                load_time=f"{time.time() - start_time:.2f}s"
            )

        return self._model

    def _normalize_language(self, language: str | None) -> str | None:
        if not language:
            return None

        normalized_language = language.lower()
        if normalized_language in self.LANGUAGE_MAP:
            return self.LANGUAGE_MAP[normalized_language]

        return language

    def _normalize_detected_language(self, language: str | None, fallback_language: str) -> str:
        if not language:
            return fallback_language

        normalized_language = self.LANGUAGE_CODE_MAP.get(language.lower())
        if normalized_language:
            return normalized_language

        return language.lower()

    def _read_duration(self, audio_path: str) -> float:
        return float(sf.info(audio_path).duration)

    def _load_audio_input(self, audio_path: str) -> tuple[np.ndarray, int]:
        """使用 soundfile 读取音频，绕过 qwen-asr 内部的 librosa 依赖路径。"""
        audio, sample_rate = sf.read(audio_path, dtype="float32", always_2d=False)

        if getattr(audio, "ndim", 1) > 1:
            audio = np.mean(audio, axis=1)

        return audio, int(sample_rate)

    def _extract_timestamp_value(self, item: Any, *keys: str, default: float = 0.0) -> float:
        for key in keys:
            if isinstance(item, dict) and key in item:
                return float(item[key])
            value = getattr(item, key, None)
            if value is not None:
                return float(value)
        return default

    def _extract_text_value(self, item: Any, *keys: str) -> str:
        for key in keys:
            if isinstance(item, dict) and key in item:
                return str(item[key]).strip()
            value = getattr(item, key, None)
            if value is not None:
                return str(value).strip()
        return ""

    def _build_segments_from_timestamps(
        self,
        timestamps,
        language: str
    ) -> list[TranscriptionSegment]:
        segments = []

        for item in timestamps or []:
            text = self._extract_text_value(item, "text", "token", "word")
            start = self._extract_timestamp_value(item, "start", "start_time", "begin")
            end = self._extract_timestamp_value(item, "end", "end_time", "stop")

            if not text:
                continue

            segments.append(TranscriptionSegment(
                start=start,
                end=end,
                text=text,
                confidence=1.0,
                language=language
            ))

        return segments

    def _get_result_field(self, result: Any, field_name: str, default=None):
        if isinstance(result, dict):
            return result.get(field_name, default)
        return getattr(result, field_name, default)

    def _transcribe_sync(
        self,
        model,
        audio_path: str,
        language: str,
        return_timestamps: bool
    ) -> tuple[list[TranscriptionSegment], str, float, float]:
        start_time = time.time()
        normalized_language = self._normalize_language(language)

        audio_input = self._load_audio_input(audio_path)

        results = model.transcribe(
            audio=audio_input,
            language=normalized_language,
            return_time_stamps=return_timestamps
        )

        if not isinstance(results, list):
            results = [results]

        if not results:
            raise ASRProviderError("Qwen3-ASR 未返回任何结果")

        result = results[0]
        detected_language = self._normalize_detected_language(
            self._get_result_field(result, "language"),
            fallback_language=language
        )
        text = self._extract_text_value(result, "text")
        timestamps = self._get_result_field(result, "time_stamps", [])
        segments = self._build_segments_from_timestamps(timestamps, detected_language)

        duration = self._read_duration(audio_path)
        if not segments and text:
            segments = [
                TranscriptionSegment(
                    start=0.0,
                    end=duration,
                    text=text,
                    confidence=1.0,
                    language=detected_language
                )
            ]

        return segments, detected_language, duration, time.time() - start_time

    async def transcribe(
        self,
        audio_path: str | Path,
        language: str = "zh",
        **kwargs
    ) -> TranscriptionResult:
        model = self._load_model()
        requested_timestamps = kwargs.get("return_time_stamps", self.return_timestamps)

        logger.info(
            "开始 Qwen3-ASR 转写",
            audio_path=str(audio_path),
            language=language,
            model=self._model_name,
            return_timestamps=requested_timestamps
        )

        segments, detected_language, duration, processing_time = await asyncio.to_thread(
            self._transcribe_sync,
            model,
            str(audio_path),
            language,
            requested_timestamps
        )

        logger.info(
            "Qwen3-ASR 转写完成",
            audio_path=str(audio_path),
            duration=duration,
            processing_time=f"{processing_time:.2f}s",
            segments=len(segments)
        )

        return TranscriptionResult(
            segments=segments,
            language=detected_language,
            language_probability=1.0,
            duration=duration,
            processing_time=processing_time
        )

    async def warmup(self) -> None:
        await asyncio.to_thread(self._load_model)

    async def close(self) -> None:
        if self._model is not None:
            del self._model
            self._model = None
            logger.info("Qwen3-ASR 模型已卸载")
