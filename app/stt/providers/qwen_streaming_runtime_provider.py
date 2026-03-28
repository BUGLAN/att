"""
Qwen3-ASR 流式 provider 运行时适配层
"""
import asyncio
from pathlib import Path
from typing import Any

import httpx
import numpy as np

from app.stt.models import ASRProviderError
from app.stt.streaming_base import (
    StreamingASRProvider,
    StreamingASRSession,
    StreamingTranscriptionUpdate,
)
from app.utils.logging import get_logger

logger = get_logger(__name__)


def _looks_like_local_model_dir(path: Path) -> bool:
    """判断目录是否像一个可直接加载的本地模型目录"""
    return path.is_dir() and (path / "config.json").exists()


def _resolve_local_model_path(model_name_or_path: str, cache_dir: str) -> str:
    """优先解析项目内已下载的本地模型目录"""
    if not model_name_or_path:
        return model_name_or_path

    direct_path = Path(model_name_or_path)
    if _looks_like_local_model_dir(direct_path):
        return str(direct_path)

    short_name = model_name_or_path.replace("\\", "/").split("/")[-1]
    candidate = Path(cache_dir) / short_name
    if _looks_like_local_model_dir(candidate):
        return str(candidate)

    return model_name_or_path


class QwenLocalStreamingSession(StreamingASRSession):
    """本地 vLLM Qwen 流式会话"""

    def __init__(self, model: Any, state: Any):
        self._model = model
        self._state = state

    def append_audio(self, pcm16k: np.ndarray) -> StreamingTranscriptionUpdate:
        self._model.streaming_transcribe(pcm16k, self._state)
        return StreamingTranscriptionUpdate(
            text=getattr(self._state, "text", "") or "",
            language=getattr(self._state, "language", "") or "",
            is_final=False,
        )

    def finish(self) -> StreamingTranscriptionUpdate:
        self._model.finish_streaming_transcribe(self._state)
        return StreamingTranscriptionUpdate(
            text=getattr(self._state, "text", "") or "",
            language=getattr(self._state, "language", "") or "",
            is_final=True,
        )


class QwenLocalVLLMStreamingProvider(StreamingASRProvider):
    """本地 vLLM Qwen 流式 provider"""

    def __init__(
        self,
        model_name: str,
        cache_dir: str,
        device: str = "auto",
        max_new_tokens: int = 256,
        gpu_memory_utilization: float = 0.85,
        chunk_size_sec: float = 2.0,
        unfixed_chunk_num: int = 2,
        unfixed_token_num: int = 5,
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
        return "qwen_asr_streaming_local_vllm"

    @property
    def model_name(self) -> str:
        return self._model_name

    def _load_dependencies(self) -> None:
        if self._model_class is not None and self._torch is not None:
            return

        try:
            import torch
            import vllm  # noqa: F401
            from qwen_asr import Qwen3ASRModel
        except ImportError as error:
            raise ASRProviderError(
                "当前环境缺少 qwen-asr[vllm] 依赖，请先安装与当前平台匹配的 Qwen/vLLM 运行时；"
                "Windows 可结合 vllm-windows 使用，Linux/WSL2 可使用官方 vLLM"
            ) from error

        self._torch = torch
        self._model_class = Qwen3ASRModel

    def _ensure_supported_device(self) -> None:
        self._load_dependencies()

        normalized_device = (self.device or "auto").lower()
        if normalized_device == "cpu":
            raise ASRProviderError("Qwen 真流式当前仅支持 vLLM/GPU，不支持 CPU 模式")

        if not self._torch.cuda.is_available():
            raise ASRProviderError("当前环境未检测到 CUDA，无法启用本地 Qwen 真流式")

    async def check_availability(self) -> dict:
        await asyncio.to_thread(self._ensure_supported_device)
        return {
            "available": True,
            "backend": "local_vllm",
            "model_name": self._model_name,
        }

    def _load_model(self):
        self._ensure_supported_device()

        if self._model is None:
            resolved_model_path = _resolve_local_model_path(self._model_name, self.cache_dir)
            Path(self.cache_dir).mkdir(parents=True, exist_ok=True)

            logger.info(
                "加载本地 Qwen 流式模型",
                model=self._model_name,
                resolved_model_path=resolved_model_path,
                cache_dir=self.cache_dir,
                chunk_size_sec=self.chunk_size_sec,
            )

            try:
                self._model = self._model_class.LLM(
                    model=resolved_model_path,
                    max_new_tokens=self.max_new_tokens,
                    gpu_memory_utilization=self.gpu_memory_utilization,
                )
            except ImportError as error:
                logger.error(
                    "Local Qwen streaming model init ImportError",
                    model=self._model_name,
                    resolved_model_path=resolved_model_path,
                    error=str(error),
                    error_repr=repr(error),
                )
                raise ASRProviderError(
                    "当前环境缺少可用的 vLLM 运行时，请先安装与平台匹配的 vLLM；"
                    "Windows 可使用 vllm-windows，Linux/WSL2 可使用官方 vLLM"
                ) from error
            except Exception as error:
                raise ASRProviderError(f"加载本地 Qwen 流式模型失败: {error}") from error

        return self._model

    def _create_session_sync(self, language: str, context: str) -> StreamingASRSession:
        model = self._load_model()
        state = model.init_streaming_state(
            context=context or "",
            language=language or None,
            unfixed_chunk_num=self.unfixed_chunk_num,
            unfixed_token_num=self.unfixed_token_num,
            chunk_size_sec=self.chunk_size_sec,
        )
        return QwenLocalStreamingSession(model=model, state=state)

    async def create_session(
        self,
        language: str = "zh",
        context: str = "",
    ) -> StreamingASRSession:
        return await asyncio.to_thread(self._create_session_sync, language, context)

    async def close(self) -> None:
        if self._model is not None:
            del self._model
            self._model = None


class QwenRemoteStreamingSession(StreamingASRSession):
    """远端桥接 Qwen 流式会话"""

    def __init__(
        self,
        client: httpx.Client,
        session_id: str,
        language: str,
    ):
        self._client = client
        self._session_id = session_id
        self._language = language or "zh"
        self._closed = False

    def _request_json(self, method: str, path: str, **kwargs) -> dict:
        try:
            response = self._client.request(method, path, **kwargs)
            response.raise_for_status()
        except httpx.HTTPError as error:
            raise ASRProviderError(f"远端 Qwen 流式服务请求失败: {error}") from error

        try:
            return response.json()
        except ValueError as error:
            raise ASRProviderError("远端 Qwen 流式服务返回了无效 JSON") from error

    def append_audio(self, pcm16k: np.ndarray) -> StreamingTranscriptionUpdate:
        payload = np.asarray(pcm16k, dtype=np.int16).tobytes()
        data = self._request_json(
            "POST",
            f"/api/chunk/{self._session_id}",
            content=payload,
            headers={"Content-Type": "application/octet-stream"},
        )
        return StreamingTranscriptionUpdate(
            text=data.get("text", "") or "",
            language=data.get("language", self._language) or self._language,
            is_final=False,
        )

    def finish(self) -> StreamingTranscriptionUpdate:
        try:
            data = self._request_json("POST", f"/api/finish/{self._session_id}")
            return StreamingTranscriptionUpdate(
                text=data.get("text", "") or "",
                language=data.get("language", self._language) or self._language,
                is_final=True,
            )
        finally:
            self.close()

    def cancel(self) -> None:
        if self._closed:
            return

        try:
            self._request_json("DELETE", f"/api/session/{self._session_id}")
        except ASRProviderError:
            logger.warning("取消远端 Qwen 流式会话失败", session_id=self._session_id)
        finally:
            self.close()

    def close(self) -> None:
        if self._closed:
            return

        self._closed = True
        self._client.close()


class QwenRemoteBridgeStreamingProvider(StreamingASRProvider):
    """远端桥接 Qwen 流式 provider"""

    def __init__(
        self,
        model_name: str,
        base_url: str,
        timeout_seconds: float = 30.0,
    ):
        self._model_name = model_name
        self.base_url = (base_url or "").rstrip("/")
        self.timeout_seconds = timeout_seconds

    @property
    def provider_type(self) -> str:
        return "qwen_asr_streaming_remote_bridge"

    @property
    def model_name(self) -> str:
        return self._model_name

    def _ensure_base_url(self) -> str:
        if not self.base_url:
            raise ASRProviderError("未配置 Qwen 流式桥接地址，请设置 QWEN_STREAMING_BASE_URL")
        return self.base_url

    def _health_check_sync(self) -> dict:
        base_url = self._ensure_base_url()

        try:
            with httpx.Client(base_url=base_url, timeout=self.timeout_seconds) as client:
                response = client.get("/api/health")
                response.raise_for_status()
                payload = response.json()
        except httpx.HTTPError as error:
            raise ASRProviderError(f"无法连接远端 Qwen 流式服务: {error}") from error
        except ValueError as error:
            raise ASRProviderError("远端 Qwen 流式服务健康检查返回了无效 JSON") from error

        return {
            "available": bool(payload.get("ready", True)),
            "backend": "remote_bridge",
            "base_url": base_url,
            "model_name": payload.get("model_name", self._model_name),
            "detail": payload,
        }

    async def check_availability(self) -> dict:
        result = await asyncio.to_thread(self._health_check_sync)
        if not result["available"]:
            raise ASRProviderError("远端 Qwen 流式服务尚未就绪")
        return result

    def _create_session_sync(self, language: str, context: str) -> StreamingASRSession:
        base_url = self._ensure_base_url()
        client = httpx.Client(base_url=base_url, timeout=self.timeout_seconds)

        try:
            response = client.post(
                "/api/start",
                json={
                    "language": language or "zh",
                    "context": context or "",
                },
            )
            response.raise_for_status()
            payload = response.json()
        except Exception:
            client.close()
            raise

        session_id = payload.get("session_id")
        if not session_id:
            client.close()
            raise ASRProviderError("远端 Qwen 流式服务未返回 session_id")

        return QwenRemoteStreamingSession(
            client=client,
            session_id=session_id,
            language=payload.get("language", language or "zh"),
        )

    async def create_session(
        self,
        language: str = "zh",
        context: str = "",
    ) -> StreamingASRSession:
        try:
            return await asyncio.to_thread(self._create_session_sync, language, context)
        except httpx.HTTPError as error:
            raise ASRProviderError(f"创建远端 Qwen 流式会话失败: {error}") from error
        except ValueError as error:
            raise ASRProviderError("远端 Qwen 流式服务返回了无效 JSON") from error


class QwenStreamingProvider(StreamingASRProvider):
    """Qwen 流式 provider 适配层"""

    SUPPORTED_BACKENDS = {"auto", "local_vllm", "remote_bridge"}

    def __init__(
        self,
        model_name: str,
        cache_dir: str,
        device: str = "auto",
        max_new_tokens: int = 256,
        gpu_memory_utilization: float = 0.85,
        chunk_size_sec: float = 2.0,
        unfixed_chunk_num: int = 2,
        unfixed_token_num: int = 5,
        streaming_backend: str = "auto",
        streaming_base_url: str = "",
        streaming_timeout_seconds: float = 30.0,
    ):
        self._model_name = model_name
        self.cache_dir = cache_dir
        self.device = device
        self.max_new_tokens = max_new_tokens
        self.gpu_memory_utilization = gpu_memory_utilization
        self.chunk_size_sec = chunk_size_sec
        self.unfixed_chunk_num = unfixed_chunk_num
        self.unfixed_token_num = unfixed_token_num
        self.streaming_backend = (streaming_backend or "auto").lower()
        self.streaming_base_url = streaming_base_url
        self.streaming_timeout_seconds = streaming_timeout_seconds

        self._provider: StreamingASRProvider | None = None

    @property
    def provider_type(self) -> str:
        return "qwen_asr_streaming"

    @property
    def model_name(self) -> str:
        return self._model_name

    @property
    def backend_type(self) -> str:
        backend = self.streaming_backend
        if backend not in self.SUPPORTED_BACKENDS:
            raise ASRProviderError(f"不支持的 Qwen 流式 backend: {backend}")

        if backend == "auto":
            return "remote_bridge" if self.streaming_base_url else "local_vllm"

        return backend

    def _create_provider(self) -> StreamingASRProvider:
        backend = self.backend_type

        if backend == "remote_bridge":
            return QwenRemoteBridgeStreamingProvider(
                model_name=self._model_name,
                base_url=self.streaming_base_url,
                timeout_seconds=self.streaming_timeout_seconds,
            )

        return QwenLocalVLLMStreamingProvider(
            model_name=self._model_name,
            cache_dir=self.cache_dir,
            device=self.device,
            max_new_tokens=self.max_new_tokens,
            gpu_memory_utilization=self.gpu_memory_utilization,
            chunk_size_sec=self.chunk_size_sec,
            unfixed_chunk_num=self.unfixed_chunk_num,
            unfixed_token_num=self.unfixed_token_num,
        )

    def _get_provider(self) -> StreamingASRProvider:
        if self._provider is None:
            self._provider = self._create_provider()
        return self._provider

    async def check_availability(self) -> dict:
        provider = self._get_provider()
        result = await provider.check_availability()
        return {
            **result,
            "backend": self.backend_type,
        }

    async def create_session(
        self,
        language: str = "zh",
        context: str = "",
    ) -> StreamingASRSession:
        provider = self._get_provider()
        return await provider.create_session(language=language, context=context)

    async def close(self) -> None:
        if self._provider is not None:
            await self._provider.close()
            self._provider = None
