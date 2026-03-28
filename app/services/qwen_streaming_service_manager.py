"""
Qwen 流式桥接服务进程管理器
"""
import asyncio
import contextlib
import os
import signal
import sys
from pathlib import Path
from typing import Optional

import httpx

from app.utils.logging import get_logger

logger = get_logger(__name__)


class QwenStreamingServiceManager:
    """管理独立 Qwen 流式桥接服务进程。"""

    def __init__(self) -> None:
        self.host = "127.0.0.1"
        self.port = 9000
        self.base_url = f"http://{self.host}:{self.port}"
        self._process: Optional[asyncio.subprocess.Process] = None
        self._start_task: Optional[asyncio.Task] = None
        self._last_error: Optional[str] = None
        self._log_handle = None
        self._project_root = Path(__file__).resolve().parents[2]
        self._log_path = self._project_root / "logs" / "qwen_streaming_service.log"

    def _is_managed_process_alive(self) -> bool:
        return self._process is not None and self._process.returncode is None

    async def _probe_health(self) -> dict:
        async with httpx.AsyncClient(timeout=2.0) as client:
            response = await client.get(f"{self.base_url}/api/health")
            response.raise_for_status()
            return response.json()

    async def _wait_until_ready(self, timeout_seconds: float = 120.0) -> dict:
        deadline = asyncio.get_running_loop().time() + timeout_seconds
        last_error = None

        while asyncio.get_running_loop().time() < deadline:
            if self._process is not None and self._process.returncode is not None:
                raise RuntimeError(self._last_error or "Qwen 流式模型服务启动失败")

            try:
                health = await self._probe_health()
                if health.get("ready"):
                    self._last_error = None
                    return health
                last_error = health.get("error")
            except Exception as error:
                last_error = str(error)

            await asyncio.sleep(0.5)

        raise RuntimeError(last_error or "等待 Qwen 流式模型服务启动超时")

    def _close_log_handle(self) -> None:
        if self._log_handle is not None:
            self._log_handle.close()
            self._log_handle = None

    async def _spawn_process(self) -> None:
        if self._is_managed_process_alive():
            return

        self._log_path.parent.mkdir(parents=True, exist_ok=True)
        self._log_handle = open(self._log_path, "a", encoding="utf-8")
        env = os.environ.copy()
        python_path = env.get("PYTHONPATH", "")
        env["PYTHONPATH"] = (
            str(self._project_root)
            if not python_path
            else f"{self._project_root}{os.pathsep}{python_path}"
        )

        self._process = await asyncio.create_subprocess_exec(
            sys.executable,
            "-m",
            "uvicorn",
            "scripts.qwen_streaming_service:app",
            "--host",
            self.host,
            "--port",
            str(self.port),
            cwd=str(self._project_root),
            env=env,
            stdout=self._log_handle,
            stderr=self._log_handle,
        )
        logger.info(
            "已启动 Qwen 流式桥接子进程",
            pid=self._process.pid,
            port=self.port,
            log_path=str(self._log_path),
        )

    async def start(self, wait_until_ready: bool = False) -> dict:
        if self._start_task and not self._start_task.done():
            if wait_until_ready:
                await self._start_task
            return await self.status()

        if self._is_managed_process_alive():
            if wait_until_ready:
                await self._wait_until_ready()
            return await self.status()

        async def run_start() -> None:
            self._last_error = None
            try:
                await self._spawn_process()
                await self._wait_until_ready()
            except Exception as error:
                self._last_error = str(error)
                logger.error("启动 Qwen 流式模型服务失败", error=self._last_error)
                if self._process and self._process.returncode is None:
                    self._process.terminate()
                    await self._process.wait()
                self._process = None
                self._close_log_handle()
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

        if self._process is None:
            self._last_error = None
            self._close_log_handle()
            return await self.status()

        if self._process.returncode is None:
            self._process.send_signal(signal.SIGTERM)
            try:
                await asyncio.wait_for(self._process.wait(), timeout=10.0)
            except asyncio.TimeoutError:
                self._process.kill()
                await self._process.wait()

        logger.info("已停止 Qwen 流式桥接子进程", pid=self._process.pid)
        self._process = None
        self._last_error = None
        self._close_log_handle()
        return await self.status()

    async def status(self) -> dict:
        loading = self._start_task is not None and not self._start_task.done()
        running = self._is_managed_process_alive()
        ready = False
        health = None
        last_error = self._last_error

        try:
            health = await self._probe_health()
            ready = bool(health.get("ready"))
            if ready:
                last_error = None
        except Exception as error:
            if running or loading:
                last_error = str(error)

        return {
            "managed": True,
            "running": running,
            "loading": loading,
            "ready": ready,
            "pid": self._process.pid if self._process else None,
            "base_url": self.base_url,
            "error": last_error,
            "health": health,
        }

    async def shutdown(self) -> None:
        await self.stop()


qwen_streaming_service_manager = QwenStreamingServiceManager()
