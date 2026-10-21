"""本地文件 → 文字转写服务（外部采集层脚本与 App 内「转文字」页共用的能力层）。

与 records 域完全解耦：不建记录、不触发 LLM 优化、不发 WebSocket 广播，
调用方拿到响应体即最终结果。核心流程：

1. 路径校验（绝对路径、存在、扩展名可识别）；
2. 按文件内容 sha256 查缓存，命中直接秒回（调用方会重试、会重跑，靠缓存省算力）；
3. ffprobe 探测时长：失败 → 415，超上限 → 413；
4. 视频容器先用 ffmpeg 抽 16kHz 单声道音轨，音频容器直接交给 provider；
5. 单并发闸门：忙时直接 503 + Retry-After，不做内存排队；
6. 结果写缓存（write_json_atomic 原子落盘）。

超过同步阈值的长文件返回 job_id 走 202 异步任务；任务状态只存内存
（调用方重跑同一文件即可从缓存恢复），并按保留时长清理防堆积。
"""

from __future__ import annotations

import asyncio
import hashlib
import json
import time
import uuid
from dataclasses import dataclass
from pathlib import Path

from app.config import settings
from app.repositories.json_store import read_json_lenient, write_json_atomic
from app.stt import factory as stt_factory
from app.stt.models import ASRProviderConfig, ASRProviderType
from app.utils.logging import get_logger

logger = get_logger(__name__)

# 视频容器：内部先 ffmpeg 抽音轨（16kHz 单声道）再转写
VIDEO_EXTRACT_EXTENSIONS = {".mp4", ".flv", ".webm", ".mkv", ".mov", ".avi", ".m4v", ".ts"}
# 音频容器：直接交给 provider（faster-whisper 自带解码）
AUDIO_DIRECT_EXTENSIONS = {".wav", ".mp3", ".m4a", ".flac", ".ogg", ".opus", ".aac", ".wma", ".amr"}

# 异步任务表约束：任务状态只存内存，完成/失败保留一段时间供调用方轮询，之后清理
JOB_KEEP_SECONDS = 30 * 60
JOB_MAX_ENTRIES = 100

# Windows 下隐藏 ffmpeg/ffprobe 子进程的控制台窗口
SUBPROCESS_WINDOWS_NO_WINDOW = 0x08000000


class FileTranscribeError(Exception):
    """业务错误：status_code 直接映射 HTTP 状态码，headers 可携带 Retry-After。"""

    def __init__(self, status_code: int, message: str, headers: dict | None = None):
        super().__init__(message)
        self.status_code = status_code
        self.message = message
        self.headers = headers or {}


@dataclass
class TranscribeOutcome:
    """一次转写请求的处理结果：同步结果（result）或异步任务号（job）。"""

    kind: str      # "result" | "job"
    payload: dict  # result 时为响应体；job 时为 {"job_id": ...}


class FileTranscriptionService:
    """文件转写编排：校验 / 缓存 / 抽音轨 / 单并发 ASR / 202 任务表。"""

    def __init__(self) -> None:
        self._lock = asyncio.Lock()        # 单并发闸门：同一时刻只跑一个转写
        self._jobs: dict[str, dict] = {}   # job_id → {"status","result","error","created_at"}
        self._job_tasks: set[asyncio.Task] = set()  # 持有任务引用防 GC
        self._cache: dict[str, dict] = {}  # sha256 → 响应体（LRU，dict 插入序即访问序）
        self._cache_loaded = False
        self._provider = None              # 本地引擎实例（模型常驻，避免每请求重载）
        self._provider_fingerprint = ""

    # ------------------------------------------------------------------ 对外入口

    async def handle_transcribe(
        self,
        path: str,
        language: str = "zh",
        want_segments: bool = False,
    ) -> TranscribeOutcome:
        """处理一次 POST /api/transcribe：缓存命中秒回；短文件同步，长文件转 202 任务。"""
        target = self._validate_path(path)
        # 内容哈希只算一次（大文件全量读不便宜），查缓存与转写后落缓存共用
        content_hash = await self._content_hash(target)

        cached = self._cache_get(content_hash)
        if cached is not None:
            logger.info("文件转写缓存命中", path=str(target), sha256=content_hash)
            return TranscribeOutcome(
                kind="result",
                payload=self._shape_payload(cached, want_segments, cached=True),
            )

        duration = await self._probe_duration(target)

        # 时长上限是业务规则，统一在入口检查（不放在探测里，避免被探测实现遗漏）
        max_seconds = settings.transcribe_file_max_duration_seconds
        if duration > max_seconds:
            raise FileTranscribeError(
                413,
                f"音频时长约 {duration / 60:.0f} 分钟，超过上限 {max_seconds / 60:.0f} 分钟",
            )

        if duration > settings.transcribe_file_async_threshold_seconds:
            job_id = self._start_job(target, language, want_segments, duration, content_hash)
            return TranscribeOutcome(kind="job", payload={"job_id": job_id})

        payload = await self._run_transcription(target, language, want_segments, duration, content_hash)
        return TranscribeOutcome(kind="result", payload=self._shape_payload(payload, want_segments))

    def get_job(self, job_id: str) -> dict | None:
        """查询异步任务状态；不存在或已过期返回 None（路由层映射 404）。"""
        self._prune_jobs()
        return self._jobs.get(job_id)

    # ------------------------------------------------------------------ 校验与探测

    @staticmethod
    def _validate_path(path: str) -> Path:
        """路径校验：绝对路径 + 文件存在 + 扩展名可识别。"""
        raw = str(path or "").strip().strip('"')
        if not raw:
            raise FileTranscribeError(404, "缺少文件路径")

        target = Path(raw)
        if not target.is_absolute():
            raise FileTranscribeError(404, f"必须是本地绝对路径：{raw}")
        if not target.is_file():
            raise FileTranscribeError(404, f"文件不存在：{raw}")

        suffix = target.suffix.lower()
        if suffix not in VIDEO_EXTRACT_EXTENSIONS | AUDIO_DIRECT_EXTENSIONS:
            raise FileTranscribeError(415, f"无法解码：不支持的文件类型「{suffix or '无后缀'}」")
        return target

    async def _probe_duration(self, target: Path) -> float:
        """ffprobe 探测媒体时长；只负责解码与取数（413 上限检查在入口统一做）。"""
        stdout, stderr, code = await self._run_subprocess(
            ["ffprobe", "-v", "error", "-show_entries", "format=duration", "-of", "json", str(target)]
        )
        if code != 0:
            raise FileTranscribeError(415, "无法解码：文件损坏或不是有效的音视频文件")

        try:
            duration = float(json.loads(stdout.decode("utf-8", "replace"))["format"]["duration"])
        except (KeyError, TypeError, ValueError) as error:
            raise FileTranscribeError(415, "无法解码：读取媒体时长失败") from error
        return duration

    async def _extract_audio_if_needed(self, target: Path) -> Path | None:
        """视频容器抽 16kHz 单声道 wav 临时文件；音频容器原样透传。"""
        if target.suffix.lower() not in VIDEO_EXTRACT_EXTENSIONS:
            return None

        out_dir = settings.audio_temp_path
        out_dir.mkdir(parents=True, exist_ok=True)
        out_path = out_dir / f"ft_{uuid.uuid4().hex[:12]}.wav"

        stdout, stderr, code = await self._run_subprocess(
            [
                "ffmpeg", "-y", "-i", str(target),
                "-vn", "-ac", "1", "-ar", "16000", "-f", "wav", str(out_path),
            ]
        )
        if code != 0 or not out_path.is_file():
            out_path.unlink(missing_ok=True)
            logger.warning("音轨提取失败", path=str(target), stderr=stderr.decode("utf-8", "replace")[:500])
            raise FileTranscribeError(415, "无法解码：音轨提取失败（文件损坏或没有音轨）")
        return out_path

    @staticmethod
    async def _run_subprocess(args: list[str]) -> tuple[bytes, bytes, int]:
        """跑 ffmpeg/ffprobe 子进程，返回 (stdout, stderr, returncode)。"""
        import sys

        kwargs: dict = {}
        if sys.platform == "win32":
            kwargs["creationflags"] = SUBPROCESS_WINDOWS_NO_WINDOW
        proc = await asyncio.create_subprocess_exec(
            *args,
            stdout=asyncio.subprocess.PIPE,
            stderr=asyncio.subprocess.PIPE,
            **kwargs,
        )
        stdout, stderr = await proc.communicate()
        return stdout, stderr, proc.returncode

    # ------------------------------------------------------------------ 转写执行

    async def _run_transcription(
        self,
        target: Path,
        language: str,
        want_segments: bool,
        duration: float,
        content_hash: str = "",
    ) -> dict:
        """单并发执行一次转写并写缓存；忙时抛 503（带 Retry-After）。"""
        busy_headers = {"Retry-After": str(settings.transcribe_file_busy_retry_after_seconds)}
        if self._lock.locked():
            raise FileTranscribeError(503, "转写引擎忙，请稍后重试", headers=busy_headers)

        await self._lock.acquire()
        extracted: Path | None = None
        try:
            extracted = await self._extract_audio_if_needed(target)
            asr_path = extracted or target
            provider = self._get_provider()

            result = await provider.transcribe(asr_path, language=language)

            payload = {
                "_sha256": content_hash or self._content_hash_sync(target),
                "text": result.text,
                "duration_seconds": float(duration or result.duration or 0.0),
                "engine": f"{provider.provider_type}:{provider.model_name}",
                "segments": [
                    {"start": float(seg.start), "end": float(seg.end), "text": seg.text}
                    for seg in result.segments
                ],
            }
            self._cache_store(target, payload)
            return payload
        except FileTranscribeError:
            raise
        except Exception as error:
            logger.error("文件转写失败", path=str(target), error=str(error))
            raise FileTranscribeError(500, f"转写失败：{error}") from error
        finally:
            if extracted is not None:
                extracted.unlink(missing_ok=True)
            self._lock.release()

    def _get_provider(self):
        """装配本地引擎并常驻：全局配置指向在线 provider（豆包）时强制回落 whisper。

        本接口约定不走出网 ASR，中文效果由本地 faster-whisper / Qwen3-ASR 保证。
        配置指纹变化时重建实例（旧实例异步释放）。
        """
        config: ASRProviderConfig = stt_factory.get_asr_provider_config()

        if config.provider not in (ASRProviderType.WHISPER, ASRProviderType.QWEN_ASR):
            logger.warning(
                "文件转写接口忽略在线 ASR 配置，回落本地 whisper",
                configured_provider=config.provider.value,
            )
            config = ASRProviderConfig(
                provider=ASRProviderType.WHISPER,
                language=config.language,
                whisper={
                    "model_size": settings.whisper_model,
                    "device": settings.whisper_device,
                    "compute_type": settings.whisper_compute_type,
                    "download_root": str(settings.faster_whisper_models_path),
                },
            )

        fingerprint = (
            config.provider.value,
            json.dumps(config.whisper, sort_keys=True, ensure_ascii=False),
            json.dumps(config.qwen_asr, sort_keys=True, ensure_ascii=False),
        )
        if self._provider is None or fingerprint != self._provider_fingerprint:
            old_provider = self._provider
            self._provider = stt_factory.create_asr_provider(config)
            self._provider_fingerprint = fingerprint
            if old_provider is not None:
                asyncio.ensure_future(self._close_provider(old_provider))
            logger.info("文件转写引擎已装配", provider=config.provider.value)

        return self._provider

    @staticmethod
    async def _close_provider(provider) -> None:
        try:
            await provider.close()
        except Exception as error:  # 释放失败不影响新实例工作
            logger.warning("旧转写引擎释放失败", error=str(error))

    # ------------------------------------------------------------------ 异步任务

    def _start_job(self, target: Path, language: str, want_segments: bool, duration: float, content_hash: str = "") -> str:
        """登记并启动一个后台转写任务，返回 job_id；忙/表满按 503 拒绝。"""
        busy_headers = {"Retry-After": str(settings.transcribe_file_busy_retry_after_seconds)}
        if self._lock.locked():
            raise FileTranscribeError(503, "转写引擎忙，请稍后重试", headers=busy_headers)
        if len(self._jobs) >= JOB_MAX_ENTRIES:
            raise FileTranscribeError(503, "转写任务表已满，请稍后重试", headers=busy_headers)

        self._prune_jobs()
        job_id = uuid.uuid4().hex
        self._jobs[job_id] = {
            "status": "running",
            "result": None,
            "error": "",
            "created_at": time.time(),
        }

        task = asyncio.create_task(self._execute_job(job_id, target, language, want_segments, duration, content_hash))
        self._job_tasks.add(task)
        task.add_done_callback(self._job_tasks.discard)
        logger.info("长文件转写任务已受理", job_id=job_id, path=str(target), duration=duration)
        return job_id

    async def _execute_job(
        self,
        job_id: str,
        target: Path,
        language: str,
        want_segments: bool,
        duration: float,
        content_hash: str = "",
    ) -> None:
        """后台任务体：终态只写 done / failed，结果与错误进任务表。"""
        try:
            payload = await self._run_transcription(target, language, want_segments, duration, content_hash)
            self._jobs[job_id]["result"] = self._shape_payload(payload, want_segments)
            self._jobs[job_id]["status"] = "done"
            logger.info("长文件转写任务完成", job_id=job_id)
        except FileTranscribeError as error:
            self._jobs[job_id]["status"] = "failed"
            self._jobs[job_id]["error"] = error.message
            logger.warning("长文件转写任务失败", job_id=job_id, error=error.message)
        except Exception as error:  # 兜底：任务不允许无声消失
            self._jobs[job_id]["status"] = "failed"
            self._jobs[job_id]["error"] = f"转写失败：{error}"
            logger.error("长文件转写任务异常", job_id=job_id, error=str(error))

    def _prune_jobs(self) -> None:
        """清理超过保留时长的终态任务，防止任务表无限膨胀。"""
        now = time.time()
        expired = [
            job_id
            for job_id, job in self._jobs.items()
            if job["status"] in ("done", "failed") and now - job["created_at"] > JOB_KEEP_SECONDS
        ]
        for job_id in expired:
            self._jobs.pop(job_id, None)

    # ------------------------------------------------------------------ 缓存

    def _cache_path(self) -> Path:
        """缓存落盘位置（独立小方法，方便测试替换路径）。"""
        return settings.transcribe_file_cache_path

    async def _content_hash(self, target: Path) -> str:
        """按文件内容算 sha256（分块读，不按路径——调用方会换目录重跑）。"""
        return await asyncio.to_thread(self._content_hash_sync, target)

    @staticmethod
    def _content_hash_sync(target: Path) -> str:
        digest = hashlib.sha256()
        with open(target, "rb") as handle:
            for chunk in iter(lambda: handle.read(1024 * 1024), b""):
                digest.update(chunk)
        return digest.hexdigest()

    def _cache_get(self, content_hash: str) -> dict | None:
        """按内容哈希查缓存。"""
        self._ensure_cache_loaded()
        return self._cache.get(content_hash)

    def _cache_store(self, target: Path, payload: dict) -> None:
        """写缓存并原子落盘；超出容量按 LRU（访问序）淘汰。"""
        self._ensure_cache_loaded()

        key = payload.get("_sha256") or self._content_hash_sync(target)
        entry = {k: v for k, v in payload.items() if not k.startswith("_")}
        entry["cached_at"] = time.time()
        self._cache.pop(key, None)  # 先删再插，保证「最近使用」排到尾部
        self._cache[key] = {**entry, "_sha256": key}

        max_size = max(int(settings.transcribe_file_cache_size), 1)
        while len(self._cache) > max_size:
            self._cache.pop(next(iter(self._cache)))

        self._save_cache()

    def _ensure_cache_loaded(self) -> None:
        """惰性加载磁盘缓存；文件损坏按空处理（宽容读，不阻塞主流程）。"""
        if self._cache_loaded:
            return
        self._cache_loaded = True
        data = read_json_lenient(self._cache_path(), default=None)
        if isinstance(data, dict):
            self._cache = {str(k): v for k, v in data.items() if isinstance(v, dict)}

    def _save_cache(self) -> None:
        """原子写缓存文件（硬约束：JSON 写入必须走 json_store）。"""
        serializable = {
            key: {k: v for k, v in entry.items() if not k.startswith("_")}
            for key, entry in self._cache.items()
        }
        try:
            self._cache_path().parent.mkdir(parents=True, exist_ok=True)
            write_json_atomic(self._cache_path(), serializable)
        except Exception as error:
            # 缓存写失败只影响下次秒回，不影响本次响应
            logger.warning("文件转写缓存落盘失败", error=str(error))

    # ------------------------------------------------------------------ 响应整形

    @staticmethod
    def _shape_payload(entry: dict, want_segments: bool, cached: bool = False) -> dict:
        """按接口契约整形响应体：text 恒返回，segments 仅在需要时返回。"""
        payload = {
            "text": str(entry.get("text", "")),
            "duration_seconds": float(entry.get("duration_seconds") or 0.0),
            "engine": str(entry.get("engine", "")),
            "cached": bool(cached),
        }
        if want_segments:
            payload["segments"] = list(entry.get("segments") or [])
        return payload


# 模块级单例：路由层直接使用
file_transcription_service = FileTranscriptionService()
