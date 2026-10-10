"""
流式 ASR 桥接目标

统一描述「后端从哪里获得流式 ASR 服务」这件事，屏蔽两种托管方式的差异：

1. local：由后端进程自己从源码目录拉起 uvicorn 子进程（Windows 原生、单容器场景）
2. remote：桥接服务由外部编排（Docker Compose）托管，后端只做健康探测与预热

存在的意义：模型服务管理器原先无论什么配置都会去 `127.0.0.1:9000` 拉起本地子进程，
在 Docker 双容器部署下会导致后端容器重复加载模型抢显存，且远端服务即使健康也被误报为不可用。
"""
from __future__ import annotations

from typing import Optional, Protocol, runtime_checkable

import httpx

from app.config import settings
from app.utils.logging import get_logger

logger = get_logger(__name__)

# 远端预热（加载模型）可能耗时远长于普通请求，这里给一个兜底下限
REMOTE_PRELOAD_TIMEOUT_SECONDS = 300.0
# 健康探测必须快速失败，避免拖垮状态查询
REMOTE_PROBE_TIMEOUT_SECONDS = 5.0


@runtime_checkable
class StreamingBridgeTarget(Protocol):
    """流式桥接目标的统一接口，对齐原 qwen_streaming_service_manager 的方法签名。"""

    async def start(self, wait_until_ready: bool = False) -> dict:
        ...

    async def stop(self) -> dict:
        ...

    async def status(self) -> dict:
        ...


def _read_bridge_config() -> tuple[str, str, float]:
    """读取流式桥接配置，优先使用用户设置，读取失败则回退环境变量。"""
    backend = str(settings.qwen_streaming_backend or "auto").lower()
    base_url = settings.qwen_streaming_base_url or ""
    timeout_seconds = float(settings.qwen_streaming_timeout_seconds or 30.0)

    try:
        from app.stt.factory import get_asr_provider_config

        qwen_config = get_asr_provider_config().qwen_asr or {}
        backend = str(qwen_config.get("streaming_backend", backend) or backend).lower()
        base_url = str(qwen_config.get("streaming_base_url", base_url) or "")
        timeout_seconds = float(
            qwen_config.get("streaming_timeout_seconds", timeout_seconds) or timeout_seconds
        )
    except Exception as error:
        logger.warning("读取流式桥接配置失败，回退环境变量配置", error=str(error))

    return backend, base_url, timeout_seconds


def is_remote_bridge() -> bool:
    """判断当前配置是否指向远端桥接服务。"""
    backend, base_url, _ = _read_bridge_config()

    if not base_url:
        return False

    # auto：配了地址就走远端，与 QwenStreamingProvider.backend_type 的判定保持一致
    return backend in {"auto", "remote_bridge"}


class LocalStreamingBridgeTarget:
    """本地桥接目标：由后端进程自己拉起桥接子进程。"""

    async def start(self, wait_until_ready: bool = False) -> dict:
        from app.services.qwen_streaming_service_manager import qwen_streaming_service_manager

        return await qwen_streaming_service_manager.start(wait_until_ready=wait_until_ready)

    async def stop(self) -> dict:
        from app.services.qwen_streaming_service_manager import qwen_streaming_service_manager

        return await qwen_streaming_service_manager.stop()

    async def status(self) -> dict:
        from app.services.qwen_streaming_service_manager import qwen_streaming_service_manager

        return await qwen_streaming_service_manager.status()


class RemoteStreamingBridgeTarget:
    """远端桥接目标：桥接服务由外部编排托管，本进程只探测与预热。"""

    def __init__(self, base_url: str, timeout_seconds: float = 30.0) -> None:
        self.base_url = (base_url or "").rstrip("/")
        self.timeout_seconds = float(timeout_seconds or 30.0)
        self._last_error: Optional[str] = None

    async def _probe_health(self) -> dict:
        async with httpx.AsyncClient(
            base_url=self.base_url,
            timeout=REMOTE_PROBE_TIMEOUT_SECONDS,
        ) as client:
            response = await client.get("/api/health")
            response.raise_for_status()
            return response.json()

    def _unavailable_status(self, error: str) -> dict:
        return {
            "managed": False,
            "running": False,
            "loading": False,
            "ready": False,
            "pid": None,
            "base_url": self.base_url,
            "error": error,
            "health": None,
            "remote": True,
        }

    async def status(self) -> dict:
        try:
            health = await self._probe_health()
        except Exception as error:
            self._last_error = f"无法连接远端流式 ASR 服务（{self.base_url}）: {error}"
            return self._unavailable_status(self._last_error)

        ready = bool(health.get("ready") and health.get("loaded"))
        # loading 必须严格表示「加载正在进行中」。
        # 不能写成「未就绪就当加载中」：远端服务可达但处于「未加载且没在加载」的空闲态时，
        # 前端会把 loading=true 渲染成「模型加载中」并**禁用开启模型按钮**，
        # 导致用户完全无法手动触发加载（实测踩过这个坑）。
        loading = bool(health.get("loading"))

        self._last_error = health.get("error")

        return {
            "managed": False,
            "running": True,
            "loading": loading,
            "ready": ready,
            "pid": None,
            "base_url": self.base_url,
            "error": health.get("error"),
            "health": health,
            "remote": True,
        }

    async def start(self, wait_until_ready: bool = False) -> dict:
        """触发远端模型预热。模型加载可能很慢，因此使用独立的长超时。"""
        self._last_error = None
        preload_timeout = max(self.timeout_seconds, REMOTE_PRELOAD_TIMEOUT_SECONDS)

        try:
            async with httpx.AsyncClient(
                base_url=self.base_url,
                timeout=preload_timeout,
            ) as client:
                response = await client.post("/api/preload")
                response.raise_for_status()
        except Exception as error:
            self._last_error = f"远端流式 ASR 服务预热失败（{self.base_url}）: {error}"
            logger.error(
                "远端流式 ASR 服务预热失败",
                base_url=self.base_url,
                error=str(error),
            )
            if wait_until_ready:
                raise RuntimeError(self._last_error) from error

        return await self.status()

    async def stop(self) -> dict:
        """远端服务由容器编排托管，后端不负责停止它，只回报当前状态。"""
        logger.info(
            "远端流式 ASR 服务由容器编排托管，后端跳过停止操作",
            base_url=self.base_url,
        )
        return await self.status()


_target: Optional[StreamingBridgeTarget] = None
_target_signature: Optional[tuple[str, str, float]] = None


def get_streaming_bridge_target() -> StreamingBridgeTarget:
    """按当前配置返回桥接目标；配置变化时自动重建。"""
    global _target, _target_signature

    backend, base_url, timeout_seconds = _read_bridge_config()
    signature = (backend, base_url, timeout_seconds)

    if _target is not None and _target_signature == signature:
        return _target

    if is_remote_bridge():
        _target = RemoteStreamingBridgeTarget(
            base_url=base_url,
            timeout_seconds=timeout_seconds,
        )
        logger.info(
            "流式 ASR 使用远端桥接目标",
            base_url=base_url,
            timeout_seconds=timeout_seconds,
        )
    else:
        _target = LocalStreamingBridgeTarget()
        logger.info("流式 ASR 使用本地托管目标")

    _target_signature = signature
    return _target


def reset_streaming_bridge_target() -> None:
    """清空缓存的桥接目标（配置热更新与测试使用）。"""
    global _target, _target_signature
    _target = None
    _target_signature = None
