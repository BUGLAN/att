"""
Speech-to-Text Module using faster-whisper
Optimized for Chinese language with fallback support
"""
from faster_whisper import WhisperModel
from typing import Optional, List, Generator
from dataclasses import dataclass
from enum import Enum
import asyncio
from concurrent.futures import ThreadPoolExecutor
import time
from pathlib import Path
import platform

from app.config import settings
from app.records.models import TranscriptionSegment, TranscriptionResult, AudioChunk
from app.utils.logging import get_logger

logger = get_logger(__name__)


class ModelSize(str, Enum):
    """Whisper model sizes"""
    TINY = "tiny"
    BASE = "base"
    SMALL = "small"
    MEDIUM = "medium"
    LARGE_V2 = "large-v2"
    LARGE_V3 = "large-v3"


class WhisperTranscriber:
    """
    faster-whisper wrapper with Chinese optimization

    Chinese Optimization Strategy:
    1. Use large-v3 model (best Chinese accuracy)
    2. Enable language detection with Chinese priority
    3. Use beam search for better accuracy
    4. Configure appropriate temperature for Chinese
    """

    # Chinese-optimized transcription configuration
    CHINESE_CONFIG = {
        "language": "zh",           # Force Chinese
        "task": "transcribe",       # transcribe (not translate)
        "beam_size": 5,             # Beam search for accuracy
        "best_of": 5,               # Keep best candidates
        "temperature": 0.0,         # Deterministic for Chinese
        "initial_prompt": "以下是中文语音转文字内容，包括普通话和方言。",
        "condition_on_previous_text": True,
        "word_timestamps": True,    # Word-level timing
        "vad_filter": True,         # Voice activity detection
        "vad_parameters": {
            "min_silence_duration_ms": 500,
            "speech_pad_ms": 200
        }
    }

    # Model memory requirements (approximate)
    MODEL_MEMORY = {
        "tiny": "~1GB",
        "base": "~1GB",
        "small": "~2GB",
        "medium": "~5GB",
        "large-v2": "~10GB",
        "large-v3": "~10GB"
    }

    def __init__(
        self,
        model_size: str = None,
        device: str = None,
        compute_type: str = None
    ):
        """
        Initialize the transcriber

        Args:
            model_size: Whisper model size (default from settings)
            device: Device to use - 'cuda', 'cpu', 'coreml', or 'auto'
            compute_type: Computation type - 'float16', 'int8_float16', 'int8'
        """
        self.model_size = model_size or settings.whisper_model
        self.device = device or settings.whisper_device
        self.compute_type = compute_type or settings.whisper_compute_type

        # Auto-detect Core ML for Apple Silicon
        if self.device == "auto" and platform.processor() == "arm":
            self.device = "coreml"
            logger.info("Apple Silicon detected, using Core ML acceleration")

        self._model: Optional[WhisperModel] = None
        self._executor = ThreadPoolExecutor(max_workers=2)

        logger.info(
            "Initializing WhisperTranscriber",
            model=self.model_size,
            device=self.device,
            compute_type=self.compute_type
        )

    def _load_model(self) -> WhisperModel:
        """Load the Whisper model (lazy loading)"""
        if self._model is None:
            logger.info("Loading Whisper model...", model=self.model_size)
            start_time = time.time()

            # Apple Silicon: use cpu with int8 compute type for best compatibility
            if self.device == "coreml":
                self._model = WhisperModel(
                    self.model_size,
                    device="cpu",
                    compute_type="int8"
                )
                logger.info("Model loaded for Apple Silicon (cpu/int8)")
            else:
                # Standard mode (CUDA/CPU)
                self._model = WhisperModel(
                    self.model_size,
                    device=self.device,
                    compute_type=self.compute_type
                )

            load_time = time.time() - start_time
            logger.info(
                "Whisper model loaded",
                model=self.model_size,
                device=self.device,
                load_time=f"{load_time:.2f}s"
            )

        return self._model

    def _transcribe_sync(
        self,
        model: WhisperModel,
        audio_path: str,
        config: dict,
        language: str
    ) -> tuple[list[TranscriptionSegment], float, float, str, float]:
        """
        Synchronous transcription helper - runs entirely in thread pool.

        This must be a separate method to ensure the generator iteration
        also happens in the thread pool, not blocking the event loop.
        """
        start_time = time.time()

        segments_gen, info = model.transcribe(audio_path, **config)

        # Collect segments (this loop was blocking the event loop before)
        segments = []
        for seg in segments_gen:
            segments.append(TranscriptionSegment(
                start=seg.start,
                end=seg.end,
                text=seg.text.strip(),
                confidence=getattr(seg, 'avg_logprob', 0),
                language=language
            ))

        processing_time = time.time() - start_time

        return segments, processing_time, info.duration, info.language, info.language_probability

    async def transcribe(
        self,
        audio_path: str | Path,
        language: str = "zh",
        **kwargs
    ) -> TranscriptionResult:
        """
        Transcribe an audio file

        Args:
            audio_path: Path to audio file
            language: Language code (default: 'zh' for Chinese)
            **kwargs: Additional transcription options

        Returns:
            TranscriptionResult with segments and metadata
        """
        model = self._load_model()

        # Merge default config with overrides
        config = {**self.CHINESE_CONFIG, "language": language, **kwargs}

        logger.info(
            "Starting transcription",
            audio_path=str(audio_path),
            language=language
        )

        # Run entire transcription (including segment iteration) in thread pool
        segments, processing_time, duration, detected_lang, lang_prob = await asyncio.to_thread(
            self._transcribe_sync,
            model,
            str(audio_path),
            config,
            language
        )

        result = TranscriptionResult(
            segments=segments,
            language=detected_lang,
            language_probability=lang_prob,
            duration=duration,
            processing_time=processing_time
        )

        logger.info(
            "Transcription complete",
            duration=result.duration,
            processing_time=f"{processing_time:.2f}s",
            num_segments=len(segments),
            detected_language=detected_lang
        )

        return result

    async def transcribe_chunk(
        self,
        chunk: AudioChunk,
        language: str = "zh"
    ) -> TranscriptionResult:
        """
        Transcribe a single audio chunk

        Args:
            chunk: AudioChunk to transcribe
            language: Language code

        Returns:
            TranscriptionResult with adjusted timestamps
        """
        result = await self.transcribe(
            chunk.file_path,
            language=language
        )

        # Adjust timestamps based on chunk start time
        for seg in result.segments:
            seg.start += chunk.start_time
            seg.end += chunk.start_time

        return result

    async def transcribe_chunks(
        self,
        chunks: List[AudioChunk],
        language: str = "zh"
    ) -> TranscriptionResult:
        """
        Transcribe multiple chunks and merge results

        Args:
            chunks: List of AudioChunks to transcribe
            language: Language code

        Returns:
            Merged TranscriptionResult
        """
        all_segments = []
        total_processing_time = 0
        detected_language = language
        language_probability = 1.0
        total_duration = 0

        for chunk in chunks:
            result = await self.transcribe_chunk(chunk, language)
            all_segments.extend(result.segments)
            total_processing_time += result.processing_time
            total_duration = max(total_duration, chunk.end_time)

            # Use detected language from first chunk
            if detected_language == language and result.language:
                detected_language = result.language
                language_probability = result.language_probability

        # Remove overlapping segments (due to chunk overlap)
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
        segments: List[TranscriptionSegment],
        min_gap: float = 0.5
    ) -> List[TranscriptionSegment]:
        """
        Merge overlapping segments from chunked transcription

        Args:
            segments: List of segments with potential overlaps
            min_gap: Minimum gap to consider segments separate

        Returns:
            Merged segments without overlaps
        """
        if not segments:
            return segments

        # Sort by start time
        sorted_segments = sorted(segments, key=lambda s: s.start)

        merged = [sorted_segments[0]]

        for seg in sorted_segments[1:]:
            last = merged[-1]

            # If overlapping, skip the duplicate
            if seg.start < last.end - min_gap:
                # Keep the longer/better segment
                if seg.end > last.end:
                    merged[-1] = seg
            else:
                merged.append(seg)

        return merged

    def unload_model(self) -> None:
        """Unload the model to free memory"""
        if self._model is not None:
            del self._model
            self._model = None
            logger.info("Whisper model unloaded")

    def __del__(self):
        """Cleanup on destruction"""
        self._executor.shutdown(wait=False)
