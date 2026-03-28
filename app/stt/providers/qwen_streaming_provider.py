"""
Qwen3-ASR 流式 provider
"""
import asyncio
from pathlib import Path
from typing import Any

import numpy as np

from app.stt.models import ASRProviderError
from app.stt.streaming_base import (
    StreamingASRProvider,
    StreamingASRSession,
    StreamingTranscriptionUpdate,
)
from app.utils.logging import get_logger

logger = get_logger(__name__)


class QwenStreamingSession(StreamingASRSession):
    """Qwen 流式转写会话"""

    def __init__(self, model, state):
        self._model = model
        self._state = state

    def append_audio(self, pcm16k: np.ndarray) -> StreamingTranscriptionUpdate:
        self._model.streaming_transcribe(pcm16k, self._state)
        return StreamingTranscriptionUpdate(
            text=getattr(self._state, "text", "") or "",
            language=getattr(self._state, "language", "") or "",
            is_final=False
        )

    def finish(self) -> StreamingTranscriptionUpdate:
        self._model.finish_streaming_transcribe(self._state)
        return StreamingTranscriptionUpdate(
            text=getattr(self._state, "text", "") or "",
            language=getattr(self._state, "language", "") or "",
            is_final=True
        )


class QwenStreamingProvider(StreamingASRProvider):
    """基于 qwen-asr[vllm] 的真流式 provider"""

    def __init__(
        self,
        model_name: str,
        cache_dir: str,
        device: str = "auto",
        max_new_tokens: int = 256,
        gpu_memory_utilization: float = 0.85,
        chunk_size_sec: float = 2.0,
        unfixed_chunk_num: int = 2,
        unfixed_token_num: int = 5
    ):
        self._model_name = model_name
        self.cache_dir = cache_dir
        self.device = device
        self.max_new_tokens = max_new_tokens
        self.gpu_memory_utilization = gpu_memory_utilization
        self.chunk_size_sec = chunk_size_sec
        self.unfixed_chunk_num = unfixed_chunk_num
        self.unfixed_token_num = unfixed_token_num

        self._torch = None
        self._model_class = None
        self._model = None

    @property
    def provider_type(self) -> str:
        return "qwen_asr_streaming"

    @property
    def model_name(self) -> str:
        return self._model_name

    def _load_dependencies(self) -> None:
        if self._model_class is not None and self._torch is not None:
            return

        try:
            import torch
            from qwen_asr import Qwen3ASRModel
        except ImportError as error:
            raise ASRProviderError(
                "Qwen 流式依赖未安装，请先执行 `pip install qwen-asr[vllm]`"
            ) from error

        self._torch = torch
        self._model_class = Qwen3ASRModel

    def _looks_like_local_model_dir(self, path: Path) -> bool:
        """判断目录是否像可直接加载的本地模型目录"""
        return path.is_dir() and (path / "config.json").exists()

    def _resolve_local_model_path(self, model_name_or_path: str) -> str:
        """优先解析项目内本地模型目录"""
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

    def _ensure_supported_device(self) -> None:
        self._load_dependencies()

        normalized_device = (self.device or "auto").lower()
        if normalized_device == "cpu":
            raise ASRProviderError("Qwen 真流式当前仅支持 vLLM/GPU，不支持 CPU 模式")

        if not self._torch.cuda.is_available():
            raise ASRProviderError("当前环境未检测到 CUDA，无法启用 Qwen 真流式")

    def _load_model(self):
        self._load_dependencies()
        self._ensure_supported_device()

        if self._model is None:
            resolved_model_path = self._resolve_local_model_path(self._model_name)
            Path(self.cache_dir).mkdir(parents=True, exist_ok=True)

            logger.info(
                "加载 Qwen 流式模型",
                model=self._model_name,
                resolved_model_path=resolved_model_path,
                cache_dir=self.cache_dir,
                chunk_size_sec=self.chunk_size_sec
            )

            try:
                self._model = self._model_class.LLM(
                    model=resolved_model_path,
                    max_new_tokens=self.max_new_tokens,
                    gpu_memory_utilization=self.gpu_memory_utilization
                )
            except ImportError as error:
                raise ASRProviderError(
                    "当前环境缺少 vLLM，请执行 `pip install qwen-asr[vllm]` 后再使用真流式"
                ) from error
            except Exception as error:
                raise ASRProviderError(f"加载 Qwen 流式模型失败: {error}") from error

        return self._model

    def _create_session_sync(
        self,
        language: str,
        context: str
    ) -> StreamingASRSession:
        model = self._load_model()
        state = model.init_streaming_state(
            context=context or "",
            language=language or None,
            unfixed_chunk_num=self.unfixed_chunk_num,
            unfixed_token_num=self.unfixed_token_num,
            chunk_size_sec=self.chunk_size_sec
        )
        return QwenStreamingSession(model=model, state=state)

    async def create_session(
        self,
        language: str = "zh",
        context: str = ""
    ) -> StreamingASRSession:
        return await asyncio.to_thread(self._create_session_sync, language, context)

    async def close(self) -> None:
        if self._model is not None:
            del self._model
            self._model = None
