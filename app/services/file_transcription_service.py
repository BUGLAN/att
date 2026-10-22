"""本地文件 → 文字转写服务（外部采集层脚本与 App 内「转文字」页共用的能力层）。

与 records 域完全解耦：不建记录、不触发 LLM 文本优化（校对管道见设计文档 M2）、
不发 WebSocket 广播，调用方拿到响应体即最终结果。核心流程：

1. 路径校验（绝对路径、存在、扩展名可识别）；
2. engine 解析：显式指定 > auto（全局配置，指向在线引擎时回落 whisper）；
   豆包凭据未配置按 409 拒绝（见设计文档 §3.1/§3.4）；
3. 按文件内容 sha256 + 引擎查缓存（v2：按文件聚合、引擎分桶），命中直接秒回；
4. ffprobe 探测时长：失败 → 415，超上限 → 413；
5. 输入适配矩阵（§3.2）：whisper/doubao 仅视频容器抽 16k 单声道 wav；
   qwen 除 wav 外一律抽 wav（soundfile 只认 wav/flac/ogg）；
6. 单并发闸门只包 ASR 执行段：忙时 503 + Retry-After，不排队；
7. 豆包超 30 分钟按会话上限分段转写后拼接，计费时长透出 billing 字段；
8. 结果写缓存（write_json_atomic 原子落盘，LRU 按整文件条目淘汰）。

超过同步阈值的长文件返回 job_id 走 202 异步任务；任务状态只存内存
（调用方重跑同一文件即可从缓存恢复），并按保留时长清理防堆积。
"""

from __future__ import annotations

import asyncio
import hashlib
import json
import math
import time
import uuid
from dataclasses import dataclass
from pathlib import Path

from app.config import settings
from app.records.models import TranscriptionResult
from app.repositories.json_store import read_json_lenient, write_json_atomic
from app.stt import factory as stt_factory
from app.stt.models import ASRProviderConfig, ASRProviderType
from app.utils.logging import get_logger

logger = get_logger(__name__)

# 视频容器：内部先 ffmpeg 抽音轨（16kHz 单声道）再转写
VIDEO_EXTRACT_EXTENSIONS = {".mp4", ".flv", ".webm", ".mkv", ".mov", ".avi", ".m4v", ".ts"}
# 音频容器（faster-whisper / 豆包 可直接读；qwen 只认其中的 wav）
AUDIO_DIRECT_EXTENSIONS = {".wav", ".mp3", ".m4a", ".flac", ".ogg", ".opus", ".aac", ".wma", ".amr"}

# 本地引擎集合：auto 语义下在线配置回落 whisper（设计文档 §3.1）
LOCAL_ENGINES = (ASRProviderType.WHISPER, ASRProviderType.QWEN_ASR)
# engine 请求参数的合法取值（路由层用 Literal 校验，服务层兜底）
ENGINE_CHOICES = ("auto", "whisper", "qwen_asr", "doubao_asr")

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
    """文件转写编排：校验 / 引擎分流 / 缓存 / 抽音轨 / 单并发 ASR / 202 任务表。"""

    def __init__(self) -> None:
        self._lock = asyncio.Lock()        # 单并发闸门：同一时刻只跑一个 ASR
        self._jobs: dict[str, dict] = {}   # job_id → {"status","result","error","created_at"}
        self._job_tasks: set[asyncio.Task] = set()  # 持有任务引用防 GC
        self._cache: dict[str, dict] = {}  # sha256 → 文件条目 {engines:{引擎:结果}}（LRU 整条淘汰）
        self._cache_loaded = False
        self._providers: dict[str, tuple[str, object]] = {}  # engine.value → (指纹, provider 实例)

    # ------------------------------------------------------------------ 对外入口

    async def handle_transcribe(
        self,
        path: str,
        language: str = "zh",
        want_segments: bool = False,
        engine: str = "auto",
    ) -> TranscribeOutcome:
        """处理一次 POST /api/transcribe：缓存命中秒回；短文件同步，长文件转 202 任务。

        engine 解析在最前（哈希大文件之前就能对凭据缺失/非法值快速失败）。
        """
        engine_type, config = self._resolve_engine(engine)
        target = self._validate_path(path)
        # 内容哈希只算一次（大文件全量读不便宜），查缓存与转写后落缓存共用
        content_hash = await self._content_hash(target)

        cached = self._cache_lookup(content_hash, engine_type.value)
        if cached is not None:
            logger.info("文件转写缓存命中", path=str(target), engine=engine_type.value)
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
            job_id = self._start_job(target, language, want_segments, duration, content_hash, engine_type, config)
            return TranscribeOutcome(kind="job", payload={"job_id": job_id})

        payload = await self._run_transcription(
            target, language, want_segments, duration, content_hash, engine_type, config
        )
        return TranscribeOutcome(kind="result", payload=self._shape_payload(payload, want_segments))

    def get_job(self, job_id: str) -> dict | None:
        """查询异步任务状态；不存在或已过期返回 None（路由层映射 404）。"""
        self._prune_jobs()
        return self._jobs.get(job_id)

    # ------------------------------------------------------------------ 引擎解析

    def _resolve_engine(self, requested: str) -> tuple[ASRProviderType, ASRProviderConfig]:
        """engine 参数解析（设计文档 §3.1）：显式 > auto；豆包凭据缺失 → 409。

        返回 (引擎类型, 请求时点配置快照)——配置快照随请求/任务走，
        避免任务执行期间全局设置变更导致行为漂移。
        """
        config: ASRProviderConfig = stt_factory.get_asr_provider_config()
        value = str(requested or "auto").strip().lower()

        if value == "auto":
            engine = config.provider
            if engine not in LOCAL_ENGINES:
                logger.warning("engine=auto：全局 ASR 配置指向在线引擎，回落本地 whisper",
                               configured=engine.value)
                engine = ASRProviderType.WHISPER
        elif value in ("whisper", "qwen_asr", "doubao_asr"):
            engine = ASRProviderType(value)
        else:
            raise FileTranscribeError(422, f"不支持的 engine：{requested}（可选 auto/whisper/qwen_asr/doubao_asr）")

        # 走 whisper 时兜底补全模型段：全局配置可能指向别的引擎而 whisper 段为空
        if engine == ASRProviderType.WHISPER and not str(config.whisper.get("model_size") or ""):
            config.whisper = {
                "model_size": settings.whisper_model,
                "device": settings.whisper_device,
                "compute_type": settings.whisper_compute_type,
                "download_root": str(settings.faster_whisper_models_path),
            }

        if engine == ASRProviderType.DOUBAO_ASR and not str(config.doubao_asr.get("api_key") or ""):
            raise FileTranscribeError(
                409,
                "豆包凭据未配置：请在 ATT 设置页填写 Access Token 后重试，或改用本地引擎",
            )

        config.provider = engine
        return engine, config

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

    async def _extract_audio_if_needed(self, target: Path, engine: ASRProviderType) -> Path | None:
        """输入适配矩阵（设计文档 §3.2），返回需转写的 wav 临时文件或 None（透传）。

        - whisper / doubao：仅视频容器抽 16kHz 单声道 wav（两者自带解码）；
        - qwen：除 .wav 外一律抽 wav——provider 用 soundfile 读音频，mp3/m4a 会失败。
        """
        suffix = target.suffix.lower()
        if engine == ASRProviderType.QWEN_ASR:
            need_extract = suffix != ".wav"
        else:
            need_extract = suffix in VIDEO_EXTRACT_EXTENSIONS
        if not need_extract:
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

    async def _slice_audio(self, target: Path, start: float, seconds: float) -> Path:
        """切一段 16kHz 单声道 wav（豆包超长文件分段用，设计文档 §3.4）。"""
        out_dir = settings.audio_temp_path
        out_dir.mkdir(parents=True, exist_ok=True)
        out_path = out_dir / f"fts_{uuid.uuid4().hex[:12]}.wav"

        stdout, stderr, code = await self._run_subprocess(
            [
                "ffmpeg", "-y", "-ss", f"{max(start, 0.0):.3f}", "-t", f"{max(seconds, 0.1):.3f}",
                "-i", str(target), "-vn", "-ac", "1", "-ar", "16000", "-f", "wav", str(out_path),
            ]
        )
        if code != 0 or not out_path.is_file():
            out_path.unlink(missing_ok=True)
            logger.warning("音频分段失败", path=str(target), start=start, seconds=seconds,
                           stderr=stderr.decode("utf-8", "replace")[:500])
            raise FileTranscribeError(415, "无法解码：音频分段失败")
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
        content_hash: str,
        engine: ASRProviderType,
        config: ASRProviderConfig,
    ) -> dict:
        """单并发执行一次转写并写缓存；忙时抛 503（带 Retry-After）。"""
        busy_headers = {"Retry-After": str(settings.transcribe_file_busy_retry_after_seconds)}
        if self._lock.locked():
            raise FileTranscribeError(503, "转写引擎忙，请稍后重试", headers=busy_headers)

        await self._lock.acquire()
        extracted: Path | None = None
        try:
            extracted = await self._extract_audio_if_needed(target, engine)
            asr_media = extracted or target
            provider = self._get_provider(engine, config)

            if engine == ASRProviderType.DOUBAO_ASR:
                result = await self._transcribe_doubao(provider, asr_media, language, duration)
            else:
                result = await provider.transcribe(asr_media, language=language)

            payload = {
                "_sha256": content_hash or self._content_hash_sync(target),
                "text": result.text,
                "duration_seconds": float(duration or result.duration or 0.0),
                "engine": f"{engine.value}:{provider.model_name}",
                "segments": [
                    {"start": float(seg.start), "end": float(seg.end), "text": seg.text}
                    for seg in result.segments
                ],
            }
            billing = float(getattr(result, "billing_seconds", 0.0) or 0.0)
            if billing > 0:
                payload["billing"] = {"audio_duration_seconds": round(billing, 3)}

            self._cache_store(content_hash or self._content_hash_sync(target), engine.value, payload)
            return payload
        except FileTranscribeError:
            raise
        except Exception as error:
            logger.error("文件转写失败", path=str(target), engine=engine.value, error=str(error))
            raise FileTranscribeError(500, f"转写失败：{error}") from error
        finally:
            if extracted is not None:
                extracted.unlink(missing_ok=True)
            self._lock.release()

    async def _transcribe_doubao(
        self, provider, media: Path, language: str, duration: float
    ) -> TranscriptionResult:
        """豆包转写：超会话上限先分段（每段一条 SAUC 会话），文本/时间戳/计费拼接。

        单会话时长上限未经火山官方确认，保守取 env 配置（默认 30 分钟，
        设计文档 §3.4）；实测支持更长后调大即可。
        """
        session_limit = max(int(settings.transcribe_file_doubao_session_seconds), 60)
        if duration <= session_limit:
            return await provider.transcribe(str(media), language=language)

        chunk_count = math.ceil(duration / session_limit)
        logger.info("豆包长文件分段转写", duration=duration, session_limit=session_limit, chunks=chunk_count)

        segments = []
        text_parts: list[str] = []
        billing_seconds = 0.0
        processing_time = 0.0
        detected_language = language

        for index in range(chunk_count):
            offset = index * session_limit
            length = min(session_limit, duration - offset)
            chunk_path = await self._slice_audio(media, offset, length)
            try:
                chunk_result = await provider.transcribe(str(chunk_path), language=language)
            finally:
                chunk_path.unlink(missing_ok=True)

            processing_time += float(chunk_result.processing_time or 0.0)
            billing_seconds += float(getattr(chunk_result, "billing_seconds", 0.0) or 0.0)
            if chunk_result.language:
                detected_language = chunk_result.language

            chunk_text = chunk_result.text or ""
            if chunk_text:
                text_parts.append(chunk_text)
            for seg in chunk_result.segments:
                segments.append(type(seg)(
                    start=float(seg.start) + offset,
                    end=float(seg.end) + offset,
                    text=seg.text,
                    confidence=getattr(seg, "confidence", 1.0),
                    language=getattr(seg, "language", language),
                ))

        return TranscriptionResult(
            segments=segments,
            language=detected_language,
            language_probability=1.0,
            duration=duration,
            processing_time=processing_time,
            billing_seconds=billing_seconds,
        )

    def _get_provider(self, engine: ASRProviderType, config: ASRProviderConfig):
        """按引擎装配并常驻实例；配置指纹变化时重建（旧实例异步释放）。

        指纹只进内存比对，绝不写日志（豆包段含密钥明文）。
        """
        section = {
            ASRProviderType.WHISPER: config.whisper,
            ASRProviderType.QWEN_ASR: config.qwen_asr,
            ASRProviderType.DOUBAO_ASR: config.doubao_asr,
        }[engine]
        fingerprint = json.dumps(
            {"engine": engine.value, "language": config.language, "section": section},
            sort_keys=True, ensure_ascii=False,
        )

        cached = self._providers.get(engine.value)
        if cached is not None and cached[0] == fingerprint:
            return cached[1]

        provider = stt_factory.create_asr_provider(config)
        self._providers[engine.value] = (fingerprint, provider)
        old = cached[1] if cached else None
        if old is not None:
            asyncio.ensure_future(self._close_provider(old))
        logger.info("文件转写引擎已装配", engine=engine.value)
        return provider

    @staticmethod
    async def _close_provider(provider) -> None:
        try:
            await provider.close()
        except Exception as error:  # 释放失败不影响新实例工作
            logger.warning("旧转写引擎释放失败", error=str(error))

    # ------------------------------------------------------------------ 异步任务

    def _start_job(
        self,
        target: Path,
        language: str,
        want_segments: bool,
        duration: float,
        content_hash: str,
        engine: ASRProviderType,
        config: ASRProviderConfig,
    ) -> str:
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

        task = asyncio.create_task(
            self._execute_job(job_id, target, language, want_segments, duration, content_hash, engine, config)
        )
        self._job_tasks.add(task)
        task.add_done_callback(self._job_tasks.discard)
        logger.info("长文件转写任务已受理", job_id=job_id, engine=engine.value,
                    path=str(target), duration=duration)
        return job_id

    async def _execute_job(
        self,
        job_id: str,
        target: Path,
        language: str,
        want_segments: bool,
        duration: float,
        content_hash: str,
        engine: ASRProviderType,
        config: ASRProviderConfig,
    ) -> None:
        """后台任务体：终态只写 done / failed，结果与错误进任务表。"""
        try:
            payload = await self._run_transcription(
                target, language, want_segments, duration, content_hash, engine, config
            )
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

    # ------------------------------------------------------------------ 缓存（v2：按文件聚合，引擎分桶）

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

    def _cache_lookup(self, content_hash: str, engine_key: str) -> dict | None:
        """按（内容哈希, 引擎）查缓存；命中会刷新 LRU 访问序。

        v1 兼容：旧条目是平铺结构（必然由 whisper 产出），视为 engines.whisper。
        """
        self._ensure_cache_loaded()
        entry = self._cache.get(content_hash)
        if not isinstance(entry, dict):
            return None

        engines = entry.get("engines")
        if isinstance(engines, dict) and isinstance(engines.get(engine_key), dict):
            self._cache.pop(content_hash, None)
            self._cache[content_hash] = entry  # 触碰至队尾
            return engines[engine_key]

        if engine_key == "whisper" and "engines" not in entry and isinstance(entry.get("text"), str):
            return entry  # v1 平铺条目
        return None

    def _cache_store(self, content_hash: str, engine_key: str, payload: dict) -> None:
        """写缓存并原子落盘；LRU 按**整文件条目**淘汰（多引擎不挤占其他文件名额）。"""
        self._ensure_cache_loaded()

        entry = dict(self._cache.get(content_hash) or {})
        engines = entry.get("engines")
        if not isinstance(engines, dict):
            engines = {}
            # v1 平铺条目提升：顶层结果挪进 whisper 桶（v1 只可能由 whisper 产出）
            if isinstance(entry.get("text"), str):
                engines["whisper"] = {
                    k: v for k, v in entry.items()
                    if k in ("text", "duration_seconds", "engine", "segments", "billing")
                }

        bucket = {k: v for k, v in payload.items() if not k.startswith("_")}
        bucket["cached_at"] = time.time()
        engines[engine_key] = bucket
        entry["engines"] = engines

        self._cache.pop(content_hash, None)  # 先删再插，保证「最近使用」排到尾部
        self._cache[content_hash] = entry

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
        try:
            self._cache_path().parent.mkdir(parents=True, exist_ok=True)
            write_json_atomic(self._cache_path(), self._cache)
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
        if isinstance(entry.get("billing"), dict):
            payload["billing"] = entry["billing"]
        if want_segments:
            payload["segments"] = list(entry.get("segments") or [])
        return payload


# 模块级单例：路由层直接使用
file_transcription_service = FileTranscriptionService()
