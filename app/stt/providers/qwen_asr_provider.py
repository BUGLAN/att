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

from app.config import get_runtime_environment
from app.records.models import TranscriptionResult, TranscriptionSegment
from app.stt.base import ASRProvider
from app.stt.models import ASRCapabilities, ASRProviderError, ASRRuntimeCapabilities
from app.stt.providers.qwen_streaming_runtime_provider import QwenStreamingProvider
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

    async def get_runtime_capabilities(
        self,
        recording_mode: str = "single"
    ) -> ASRRuntimeCapabilities:
        if recording_mode != "streaming":
            return ASRRuntimeCapabilities(
                model_service_supported=True,
                model_service_mode="provider",
                streaming_supported=False,
                streaming_available=False,
                streaming_reason="当前录音模式不是流式",
            )

        if get_runtime_environment() == "windows":
            reason = "Windows 本地后端当前不支持流式服务，请切换为单段或多段录音。"
            return ASRRuntimeCapabilities(
                model_service_supported=False,
                model_service_mode="managed_streaming_bridge",
                model_service_reason=reason,
                streaming_supported=False,
                streaming_available=False,
                streaming_reason=reason,
            )

        backend = self.streaming_backend or "auto"
        streaming_provider = QwenStreamingProvider(
            model_name=self._model_name,
            cache_dir=self.cache_dir,
            device=self.device,
            max_new_tokens=self.max_new_tokens,
            streaming_backend=self.streaming_backend,
            streaming_base_url=self.streaming_base_url,
            streaming_timeout_seconds=self.streaming_timeout_seconds,
        )
        reason = "流式模型服务尚未启动"

        if backend == "remote_bridge" and self.streaming_base_url:
            try:
                availability = await streaming_provider.check_availability()
                return ASRRuntimeCapabilities(
                    model_service_supported=True,
                    model_service_mode="managed_streaming_bridge",
                    streaming_supported=True,
                    streaming_available=bool(availability.get("available", False)),
                    streaming_reason=None,
                    streaming_backend=availability.get("backend"),
                )
            except Exception as error:
                reason = str(error)

        return ASRRuntimeCapabilities(
            model_service_supported=True,
            model_service_mode="managed_streaming_bridge",
            streaming_supported=True,
            streaming_available=False,
            streaming_reason=reason,
            streaming_backend=streaming_provider.backend_type,
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
        """判断目录是否像一个可直接加载的本地模型目录。"""
        return path.is_dir() and (path / "config.json").exists()

    def _iter_local_model_candidates(self, model_name_or_path: str):
        direct_path = Path(model_name_or_path)
        yield direct_path

        short_name = model_name_or_path.replace("\\", "/").split("/")[-1]
        safe_short_name = short_name.replace(".", "___")
        cache_dir = Path(self.cache_dir)

        yield cache_dir / short_name
        yield cache_dir / safe_short_name
        yield cache_dir / "Qwen" / short_name
        yield cache_dir / "Qwen" / safe_short_name

        snapshot_root = cache_dir / f"models--{model_name_or_path.replace('/', '--')}" / "snapshots"
        if snapshot_root.is_dir():
            for snapshot_dir in sorted(snapshot_root.iterdir(), reverse=True):
                yield snapshot_dir

    def _resolve_local_model_path(self, model_name_or_path: str) -> str:
        """优先解析项目内已下载好的本地模型目录，避免重复联网下载。"""
        if not model_name_or_path:
            return model_name_or_path

        for candidate in self._iter_local_model_candidates(model_name_or_path):
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

    # ==================== 长文件分块（文件转写路径，设计文档 §3.3） ====================

    # 静音检测参数：20ms 窗口能量，连续 ≥500ms 判静音，阈值取全局能量的 2%（约 -34dB）
    SILENCE_WINDOW_MS = 20
    SILENCE_MIN_MS = 500
    SILENCE_RELATIVE_THRESHOLD = 0.02

    def _silence_cut_points(self, audio: "np.ndarray", sample_rate: int) -> list[int]:
        """纯 numpy 能量静音检测：返回可作为切点的采样位置（长静音段中点）。

        切在静音中点：边界两侧都留有静音余量，避免切掉词首词尾。
        """
        import numpy as np

        window = max(int(sample_rate * self.SILENCE_WINDOW_MS / 1000), 1)
        usable = (audio.size // window) * window
        if usable <= 0:
            return []

        frames = audio[:usable].reshape(-1, window).astype("float32")
        rms = np.sqrt(np.mean(np.square(frames), axis=1))
        threshold = max(float(np.percentile(rms, 95)) * self.SILENCE_RELATIVE_THRESHOLD, 1e-5)
        silent = rms <= threshold

        min_windows = max(int(self.SILENCE_MIN_MS / self.SILENCE_WINDOW_MS), 1)
        cuts: list[int] = []
        run_start: int | None = None
        for index, is_silent in enumerate(silent):
            if is_silent and run_start is None:
                run_start = index
            elif not is_silent and run_start is not None:
                if index - run_start >= min_windows:
                    cuts.append((run_start + index) // 2 * window)
                run_start = None
        if run_start is not None and (silent.size - run_start) >= min_windows:
            cuts.append((run_start + silent.size) // 2 * window)
        return cuts

    def _plan_chunks(self, audio_size: int, sample_rate: int, cut_points: list[int]) -> list[tuple[int, int]]:
        """贪心装箱：每块 ≤ max_audio_seconds；优先在窗内最靠右的静音点切，无静音点硬切。"""
        limit = max(int(self.max_audio_seconds), 1) * sample_rate
        chunks: list[tuple[int, int]] = []
        start = 0
        while start < audio_size:
            limit_end = min(start + limit, audio_size)
            if limit_end >= audio_size:
                chunks.append((start, audio_size))
                break
            candidates = [cut for cut in cut_points if start < cut <= limit_end]
            end = candidates[-1] if candidates else limit_end
            chunks.append((start, end))
            start = end
        return chunks

    def _transcribe_chunked_sync(
        self,
        model,
        audio_path: str,
        language: str,
        return_timestamps: bool,
        duration: float,
    ) -> tuple[list[TranscriptionSegment], str, float]:
        """分块转写主流程：静音切点装箱 → 逐块推理 → 时间戳偏移拼接。

        返回 (segments, language, processing_time)；块间不重叠（静音处天然边界）。
        """
        start_time = time.time()
        audio, sample_rate = self._load_audio_input(audio_path)

        chunks = self._plan_chunks(audio.size, sample_rate, self._silence_cut_points(audio, sample_rate))
        logger.info(
            "Qwen3-ASR 长文件分块转写",
            duration=duration,
            chunk_window=self.max_audio_seconds,
            chunks=[round((end - start) / sample_rate, 1) for start, end in chunks],
        )

        normalized_language = self._normalize_language(language)
        detected_language = language
        segments: list[TranscriptionSegment] = []

        for chunk_start, chunk_end in chunks:
            chunk_audio = audio[chunk_start:chunk_end]
            offset = chunk_start / sample_rate
            chunk_seconds = (chunk_end - chunk_start) / sample_rate

            results = model.transcribe(
                audio=(chunk_audio, sample_rate),
                language=normalized_language,
                return_time_stamps=return_timestamps
            )
            if not isinstance(results, list):
                results = [results]
            if not results:
                continue

            result = results[0]
            detected_language = self._normalize_detected_language(
                self._get_result_field(result, "language"),
                fallback_language=language
            )
            text = self._extract_text_value(result, "text")
            timestamps = self._get_result_field(result, "time_stamps", [])
            chunk_segments = self._build_segments_from_timestamps(timestamps, detected_language)

            if not chunk_segments and text:
                chunk_segments = [
                    TranscriptionSegment(
                        start=0.0,
                        end=chunk_seconds,
                        text=text,
                        confidence=1.0,
                        language=detected_language
                    )
                ]

            for segment in chunk_segments:
                segments.append(
                    TranscriptionSegment(
                        start=segment.start + offset,
                        end=segment.end + offset,
                        text=segment.text,
                        confidence=segment.confidence,
                        language=segment.language
                    )
                )

        return segments, detected_language, time.time() - start_time

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

        # 长文件：超过 max_audio_seconds 走静音分块拼接（文件转写路径，设计 §3.3）
        duration = self._read_duration(str(audio_path))
        if duration > self.max_audio_seconds:
            segments, detected_language, processing_time = await asyncio.to_thread(
                self._transcribe_chunked_sync,
                model,
                str(audio_path),
                language,
                requested_timestamps,
                duration,
            )
        else:
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
