#!/usr/bin/env python3
"""
Qwen3-ASR 真流式桥接服务

用于启动独立的 Qwen 流式服务。
可部署在 Windows（结合 vllm-windows）或 Linux / WSL2，
供主应用通过 HTTP 桥接调用。
"""
import os
import sys
import time
import uuid
from dataclasses import dataclass
from pathlib import Path
from typing import Any

import numpy as np
from fastapi import FastAPI, HTTPException, Request
from pydantic import BaseModel
from dotenv import load_dotenv


PROJECT_ROOT = Path(__file__).resolve().parent.parent
if str(PROJECT_ROOT) not in sys.path:
    sys.path.insert(0, str(PROJECT_ROOT))

load_dotenv(PROJECT_ROOT / ".env")

from app.utils.logging import get_logger, setup_logging

logger = get_logger(__name__)

DEFAULT_SAMPLE_RATE = 16000
DEFAULT_MODEL_NAME = os.getenv("QWEN_STREAMING_MODEL", "Qwen/Qwen3-ASR-1.7B")
DEFAULT_CACHE_DIR = os.getenv(
    "QWEN_STREAMING_CACHE_DIR",
    str(PROJECT_ROOT / "models" / "qwen"),
)
DEFAULT_MAX_NEW_TOKENS = int(os.getenv("QWEN_STREAMING_MAX_NEW_TOKENS", "256"))
DEFAULT_MAX_MODEL_LEN = int(os.getenv("QWEN_STREAMING_MAX_MODEL_LEN", "16384"))
DEFAULT_GPU_MEMORY_UTILIZATION = float(
    os.getenv("QWEN_STREAMING_GPU_MEMORY_UTILIZATION", "0.85")
)
DEFAULT_CHUNK_SIZE_SEC = float(os.getenv("QWEN_STREAMING_CHUNK_SIZE_SEC", "2.0"))
DEFAULT_UNFIXED_CHUNK_NUM = int(os.getenv("QWEN_STREAMING_UNFIXED_CHUNK_NUM", "2"))
DEFAULT_UNFIXED_TOKEN_NUM = int(os.getenv("QWEN_STREAMING_UNFIXED_TOKEN_NUM", "5"))
LANGUAGE_MAP = {
    "auto": None,
    "zh": "Chinese",
    "zh-cn": "Chinese",
    "zh-hans": "Chinese",
    "zh-tw": "Chinese",
    "zh-hk": "Chinese",
    "yue": "Cantonese",
    "en": "English",
    "ja": "Japanese",
    "ko": "Korean",
    "de": "German",
    "fr": "French",
    "es": "Spanish",
    "pt": "Portuguese",
    "ru": "Russian",
    "ar": "Arabic",
    "th": "Thai",
    "vi": "Vietnamese",
    "tr": "Turkish",
    "hi": "Hindi",
    "id": "Indonesian",
    "it": "Italian",
    "ms": "Malay",
    "nl": "Dutch",
    "sv": "Swedish",
    "da": "Danish",
    "fi": "Finnish",
    "pl": "Polish",
    "cs": "Czech",
    "fil": "Filipino",
    "fa": "Persian",
    "el": "Greek",
    "ro": "Romanian",
    "hu": "Hungarian",
    "mk": "Macedonian",
}


class StartSessionRequest(BaseModel):
    """创建流式会话请求"""

    language: str = "zh"
    context: str = ""


@dataclass
class RemoteStreamingSession:
    """远端流式会话"""

    session_id: str
    state: Any
    language: str = "zh"


def _looks_like_local_model_dir(path: Path) -> bool:
    """判断目录是否像一个可直接加载的本地模型目录"""
    return path.is_dir() and (path / "config.json").exists()


def _resolve_model_name_or_path(model_name_or_path: str, cache_dir: str) -> str:
    """优先解析本地模型目录"""
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


def _normalize_language(language: str | None) -> str | None:
    """将前端常见语言代码转换为 Qwen 流式运行时支持的语言名"""
    if language is None:
        return None

    normalized = language.strip()
    if not normalized:
        return None

    mapped = LANGUAGE_MAP.get(normalized.lower())
    if normalized.lower() in LANGUAGE_MAP:
        return mapped

    return normalized


class QwenStreamingRuntime:
    """Qwen 流式运行时"""

    def __init__(self):
        self.model_name = DEFAULT_MODEL_NAME
        self.cache_dir = DEFAULT_CACHE_DIR
        self.max_new_tokens = DEFAULT_MAX_NEW_TOKENS
        self.max_model_len = DEFAULT_MAX_MODEL_LEN
        self.gpu_memory_utilization = DEFAULT_GPU_MEMORY_UTILIZATION
        self.chunk_size_sec = DEFAULT_CHUNK_SIZE_SEC
        self.unfixed_chunk_num = DEFAULT_UNFIXED_CHUNK_NUM
        self.unfixed_token_num = DEFAULT_UNFIXED_TOKEN_NUM

        self._model_class = None
        self._model = None
        self._loading = False
        self._engine_warmed = False
        self._effective_gpu_memory_utilization = self.gpu_memory_utilization
        self._sessions: dict[str, RemoteStreamingSession] = {}

    def _load_dependencies(self) -> None:
        if self._model_class is not None:
            return

        try:
            from qwen_asr import Qwen3ASRModel
        except ImportError as error:
            raise RuntimeError(
                "缺少 qwen-asr[vllm] 依赖，请先安装与当前平台匹配的 Qwen/vLLM 运行时"
            ) from error

        self._model_class = Qwen3ASRModel

    def load_model(self):
        self._load_dependencies()

        if self._model is None:
            Path(self.cache_dir).mkdir(parents=True, exist_ok=True)
            resolved_model_path = _resolve_model_name_or_path(self.model_name, self.cache_dir)
            effective_gpu_memory_utilization = self._resolve_gpu_memory_utilization()

            logger.info(
                "加载 Qwen 流式服务模型",
                model=self.model_name,
                resolved_model_path=resolved_model_path,
                cache_dir=self.cache_dir,
                gpu_memory_utilization=effective_gpu_memory_utilization,
            )

            self._loading = True
            try:
                self._model = self._model_class.LLM(
                    model=resolved_model_path,
                    max_new_tokens=self.max_new_tokens,
                    max_model_len=self.max_model_len,
                    gpu_memory_utilization=effective_gpu_memory_utilization,
                )
                self._effective_gpu_memory_utilization = effective_gpu_memory_utilization
            finally:
                self._loading = False

        return self._model

    def preload(self) -> dict:
        """预加载模型并完成引擎预热。

        注意：仅调用 load_model 是不够的——vLLM 的 CUDA 图捕获与算子编译发生在
        首次推理时，实测首个音频块会因此耗时 32 秒左右。若不在预热阶段吃掉这段开销，
        它会完整地落在用户的第一次录音上（表现为「一开口就卡住几十秒」）。
        """
        model = self.load_model()
        self._warmup_engine(model)
        return self.health()

    def _warmup_engine(self, model) -> None:
        """用一小段静音触发一次真实流式推理，把首次编译开销提前到预热阶段。"""
        if self._engine_warmed:
            return

        if os.getenv("QWEN_STREAMING_WARMUP", "1") == "0":
            logger.info("已按 QWEN_STREAMING_WARMUP=0 跳过引擎预热")
            return

        logger.info("开始流式引擎预热（首次推理包含编译开销，可能耗时数十秒）")
        started = time.perf_counter()

        try:
            silence = np.zeros(int(DEFAULT_SAMPLE_RATE * 0.5), dtype=np.int16)
            state = model.init_streaming_state(
                context="",
                language=None,
                unfixed_chunk_num=self.unfixed_chunk_num,
                unfixed_token_num=self.unfixed_token_num,
                chunk_size_sec=self.chunk_size_sec,
            )
            model.streaming_transcribe(silence, state)
            model.finish_streaming_transcribe(state)
        except Exception as error:
            # 预热失败不应让服务不可用：记录告警，真正推理时仍会自行编译
            logger.warning("流式引擎预热失败，首次推理仍会有编译开销", error=str(error))
            return

        self._engine_warmed = True
        logger.info(
            "流式引擎预热完成",
            elapsed_seconds=round(time.perf_counter() - started, 2),
        )


    def _resolve_gpu_memory_utilization(self) -> float:
        configured = self.gpu_memory_utilization

        try:
            import torch

            if not torch.cuda.is_available():
                return configured

            free_bytes, total_bytes = torch.cuda.mem_get_info()
            free_ratio = free_bytes / total_bytes if total_bytes else configured
            adjusted = max(0.1, min(configured, free_ratio - 0.02))

            if adjusted < configured:
                logger.warning(
                    "根据当前空闲显存下调 GPU memory utilization",
                    configured=configured,
                    adjusted=round(adjusted, 3),
                    free_gib=round(free_bytes / (1024 ** 3), 2),
                    total_gib=round(total_bytes / (1024 ** 3), 2),
                )

            return adjusted
        except Exception as error:
            logger.warning(
                "检测空闲显存失败，继续使用原始 GPU memory utilization",
                configured=configured,
                error=str(error),
            )
            return configured

    def create_session(self, language: str, context: str) -> dict:
        model = self.load_model()
        normalized_language = _normalize_language(language)
        state = model.init_streaming_state(
            context=context or "",
            language=normalized_language,
            unfixed_chunk_num=self.unfixed_chunk_num,
            unfixed_token_num=self.unfixed_token_num,
            chunk_size_sec=self.chunk_size_sec,
        )

        session_id = str(uuid.uuid4())
        self._sessions[session_id] = RemoteStreamingSession(
            session_id=session_id,
            state=state,
            language=normalized_language or "auto",
        )

        return {
            "session_id": session_id,
            "language": normalized_language or "auto",
            "sample_rate": DEFAULT_SAMPLE_RATE,
        }

    def get_session(self, session_id: str) -> RemoteStreamingSession:
        session = self._sessions.get(session_id)
        if session is None:
            raise KeyError(session_id)
        return session

    def append_audio(self, session_id: str, payload: bytes) -> dict:
        session = self.get_session(session_id)

        if payload:
            pcm16 = np.frombuffer(payload, dtype=np.int16)
            self.load_model().streaming_transcribe(pcm16, session.state)

        text = getattr(session.state, "text", "") or ""
        language = getattr(session.state, "language", "") or session.language
        session.language = language

        return {
            "text": text,
            "language": language,
        }

    def finish_session(self, session_id: str) -> dict:
        session = self.get_session(session_id)
        self.load_model().finish_streaming_transcribe(session.state)
        self._sessions.pop(session_id, None)

        return {
            "text": getattr(session.state, "text", "") or "",
            "language": getattr(session.state, "language", "") or session.language,
        }

    def cancel_session(self, session_id: str) -> bool:
        return self._sessions.pop(session_id, None) is not None

    def health(self) -> dict:
        self._load_dependencies()
        resolved_model_path = _resolve_model_name_or_path(self.model_name, self.cache_dir)
        return {
            "ready": self._model is not None,
            "engine_warmed": self._engine_warmed,
            "model_name": self.model_name,
            "resolved_model_path": resolved_model_path,
            "loaded": self._model is not None,
            "loading": self._loading,
            "gpu_memory_utilization": self._effective_gpu_memory_utilization,
            "active_sessions": len(self._sessions),
            "sample_rate": DEFAULT_SAMPLE_RATE,
        }


setup_logging()
runtime = QwenStreamingRuntime()
app = FastAPI(title="Qwen Streaming Bridge", version="0.1.0")


@app.get("/api/health")
async def health() -> dict:
    """健康检查"""
    try:
        return runtime.health()
    except Exception as error:
        return {
            "ready": False,
            "error": str(error),
            "model_name": runtime.model_name,
        }


@app.post("/api/start")
async def start_session(request: StartSessionRequest) -> dict:
    """创建流式会话"""
    try:
        return runtime.create_session(request.language, request.context)
    except Exception as error:
        logger.error("创建 Qwen 流式会话失败", error=str(error))
        raise HTTPException(status_code=500, detail=str(error)) from error


@app.post("/api/preload")
async def preload_model() -> dict:
    """预加载流式模型。"""
    try:
        return runtime.preload()
    except Exception as error:
        logger.error("预加载 Qwen 流式模型失败", error=str(error))
        raise HTTPException(status_code=500, detail=str(error)) from error


@app.post("/api/chunk/{session_id}")
async def append_chunk(session_id: str, request: Request) -> dict:
    """追加音频分片"""
    try:
        payload = await request.body()
        return runtime.append_audio(session_id, payload)
    except KeyError as error:
        raise HTTPException(status_code=404, detail="Session not found") from error
    except Exception as error:
        logger.error("处理 Qwen 流式音频失败", session_id=session_id, error=str(error))
        raise HTTPException(status_code=500, detail=str(error)) from error


@app.post("/api/finish/{session_id}")
async def finish_session(session_id: str) -> dict:
    """结束流式会话"""
    try:
        return runtime.finish_session(session_id)
    except KeyError as error:
        raise HTTPException(status_code=404, detail="Session not found") from error
    except Exception as error:
        logger.error("完成 Qwen 流式会话失败", session_id=session_id, error=str(error))
        raise HTTPException(status_code=500, detail=str(error)) from error


@app.delete("/api/session/{session_id}")
async def cancel_session(session_id: str) -> dict:
    """取消流式会话"""
    cancelled = runtime.cancel_session(session_id)
    return {
        "cancelled": cancelled,
        "session_id": session_id,
    }


if __name__ == "__main__":
    import uvicorn

    uvicorn.run(
        app,
        host=os.getenv("QWEN_STREAMING_HOST", "0.0.0.0"),
        port=int(os.getenv("QWEN_STREAMING_PORT", "9000")),
        reload=False,
    )
