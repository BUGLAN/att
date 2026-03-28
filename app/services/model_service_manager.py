"""
通用模型服务管理器
"""
import asyncio
import contextlib
from typing import Optional

from app.config import user_settings
from app.services.qwen_streaming_service_manager import qwen_streaming_service_manager
from app.utils.logging import get_logger

logger = get_logger(__name__)


class ModelServiceManager:
    """统一管理当前 ASR provider 的模型预热与释放。"""

    def __init__(self) -> None:
        self._asr_provider = None
        self._start_task: Optional[asyncio.Task] = None
        self._provider_ready = False
        self._provider_loading = False
        self._last_error: Optional[str] = None

    def configure(self, asr_provider) -> None:
        self._asr_provider = asr_provider
        self._provider_ready = False
        self._provider_loading = False
        self._last_error = None

    def _current_mode(self) -> str:
        return str(user_settings.get("recording_mode", "single") or "single").lower()

    def _is_streaming_bridge_mode(self) -> bool:
        return (
            self._asr_provider is not None and
            getattr(self._asr_provider, "provider_type", "") == "qwen_asr" and
            self._current_mode() == "streaming"
        )

    async def _warmup_standard_provider(self) -> None:
        if self._asr_provider is None:
            raise RuntimeError("ASR provider 未初始化")

        self._provider_loading = True
        self._provider_ready = False
        try:
            await self._asr_provider.warmup()
            self._provider_ready = True
        finally:
            self._provider_loading = False

    async def start(self, wait_until_ready: bool = False) -> dict:
        if self._start_task and not self._start_task.done():
            if wait_until_ready:
                await self._start_task
            return await self.status()

        async def run_start() -> None:
            self._last_error = None
            try:
                if self._is_streaming_bridge_mode():
                    self._provider_ready = False
                    self._provider_loading = False
                    await qwen_streaming_service_manager.start(wait_until_ready=True)
                else:
                    await qwen_streaming_service_manager.stop()
                    await self._warmup_standard_provider()
            except Exception as error:
                self._last_error = str(error)
                logger.error("启动模型服务失败", error=self._last_error)
                raise

        self._start_task = asyncio.create_task(run_start())
        if wait_until_ready:
            await self._start_task
        return await self.status()

    async def stop(self) -> dict:
        if self._start_task and not self._start_task.done():
            self._start_task.cancel()
            with contextlib.suppress(asyncio.CancelledError):
                await self._start_task
        self._start_task = None

        await qwen_streaming_service_manager.stop()

        if self._asr_provider is not None:
            await self._asr_provider.close()

        self._provider_ready = False
        self._provider_loading = False
        self._last_error = None
        return await self.status()

    async def status(self) -> dict:
        streaming_status = await qwen_streaming_service_manager.status()
        loading = self._start_task is not None and not self._start_task.done()
        provider_type = getattr(self._asr_provider, "provider_type", None)
        model_name = getattr(self._asr_provider, "model_name", None)
        mode = self._current_mode()
        using_streaming_bridge = self._is_streaming_bridge_mode()

        if using_streaming_bridge:
            ready = bool(streaming_status.get("ready"))
            running = bool(streaming_status.get("running"))
            error = streaming_status.get("error") or self._last_error
        else:
            ready = self._provider_ready
            running = self._provider_ready or self._provider_loading
            error = self._last_error

        health = streaming_status.get("health") if using_streaming_bridge else {
            "ready": self._provider_ready,
            "loaded": self._provider_ready,
            "loading": self._provider_loading,
            "backend": provider_type,
            "model_name": model_name,
            "error": self._last_error,
        }

        return {
            "managed": True,
            "provider_type": provider_type,
            "model_name": model_name,
            "recording_mode": mode,
            "loading": loading or self._provider_loading or bool(streaming_status.get("loading")),
            "running": running,
            "ready": ready,
            "error": error,
            "pid": streaming_status.get("pid"),
            "base_url": streaming_status.get("base_url"),
            "health": health,
            "components": {
                "provider": {
                    "ready": self._provider_ready,
                    "loading": self._provider_loading,
                },
                "streaming_bridge": streaming_status,
            },
        }

    async def shutdown(self) -> None:
        await self.stop()


model_service_manager = ModelServiceManager()
