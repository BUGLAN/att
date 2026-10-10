"""火山引擎豆包双向流式 ASR provider（在线识别，无需本地模型）。

一条会话的完整生命周期：

1. ``create_session``：握手（三个头部）→ 发送全量客户端请求（含音频元数据与 corpus）；
2. ``append_audio``：把 16k 单声道 PCM 按 200ms 切帧推流，并读取服务端已到达的结果；
3. ``finish``：发送末包 → 等末包结果/关闭 → 返回整场文字稿；
4. ``cancel``：用户取消，不算失败。

关键约定（与调用文档一致，改动前先读）：

- 文本按 ``utterances`` 的 **definite 连续前缀**推进，不用 ``result.text`` 变化做判据；
- ``result_type='full'``（累积文本）+ ``show_utterances=true``（句级 definite）必须同时满足；
- 错误既可能在推流中到达（错误帧），也可能表现为握手失败或直接断连，
  三条路径都要归一化成可读中文，并且**不要**把「请求被拒」当成「网络抖动」重试；
- 密钥只进握手头，不落日志、不进异常文案。
"""
from __future__ import annotations

import asyncio
import inspect
import time
import uuid
from typing import Any, Optional

import numpy as np

from app.records.models import TranscriptionResult, TranscriptionSegment
from app.stt.base import ASRProvider
from app.stt.models import (
    ASRCapabilities,
    ASRProviderError,
    ASRRuntimeCapabilities,
)
from app.stt.providers.volcengine_sauc import (
    ASR_CONTEXT_MAX_CHARS,
    CHUNK_MS,
    CHUNK_SAMPLES,
    DEFAULT_ENDPOINT,
    DEFAULT_RESOURCE_ID,
    FRAME_ERROR,
    FRAME_RESULT,
    KIND_NETWORK,
    MODEL_NAME,
    SAMPLE_RATE,
    SaucErrorInfo,
    SaucResultFrame,
    SaucSessionError,
    advance_definite_prefix,
    build_audio_request,
    build_bigmodel_request,
    build_full_client_request,
    classify_close_code,
    classify_handshake_status,
    classify_sauc_error,
    normalize_language,
    parse_server_frame,
    pending_from_cumulative_text,
    truncate_context_text,
)
from app.stt.streaming_base import (
    StreamingASRProvider,
    StreamingASRSession,
    StreamingTranscriptionUpdate,
)
from app.utils.logging import get_logger

logger = get_logger(__name__)

# 建连超时：超过这个时间没握手成功就按 network/可重试处理
DEFAULT_CONNECT_TIMEOUT_SECONDS = 10.0
# 每次追加音频后等待服务端结果的时长（服务端对每个音频包回一帧）
DEFAULT_DRAIN_TIMEOUT_SECONDS = 0.1
# finish 的硬超时与「静默多久算收完」窗口
DEFAULT_FINISH_TIMEOUT_SECONDS = 6.0
DEFAULT_FINISH_QUIET_SECONDS = 1.2
# 批量转写时每推 N 个音频包主动清一次接收队列，避免队列压满导致反向背压
BATCH_DRAIN_INTERVAL_CHUNKS = 25
# 单次非阻塞清队列的帧数上限，避免异常情况下死循环
MAX_DRAIN_FRAMES = 512

try:  # pragma: no cover - 依赖缺失属于环境问题，单独给出可读报错
    from websockets.exceptions import ConnectionClosed, InvalidStatus
    from websockets.sync.client import connect as _ws_connect

    # websockets 12/13 用 extra_headers，14+ 改名为 additional_headers
    _HEADER_KWARG = (
        "additional_headers"
        if "additional_headers" in inspect.signature(_ws_connect).parameters
        else "extra_headers"
    )
    WEBSOCKETS_AVAILABLE = True
except ImportError:  # pragma: no cover
    ConnectionClosed = Exception  # type: ignore[assignment,misc]
    InvalidStatus = Exception  # type: ignore[assignment,misc]
    _ws_connect = None  # type: ignore[assignment]
    _HEADER_KWARG = "additional_headers"
    WEBSOCKETS_AVAILABLE = False


def _require_websockets() -> None:
    if not WEBSOCKETS_AVAILABLE:
        raise ASRProviderError("缺少 websockets 依赖，无法启用豆包在线流式识别")


class DoubaoStreamingSession(StreamingASRSession):
    """豆包双向流式会话（同步实现，由调用方放到工作线程里跑）。

    ``websockets.sync`` 的连接对象自带锁，允许多个线程**串行**调用 send/recv；
    本会话的所有方法都由同一个流式会话串行驱动，因此不需要额外的线程池。
    """

    def __init__(
        self,
        socket: Any,
        *,
        language: str = "zh-CN",
        connect_id: str = "",
        log_id: str = "",
        chunk_samples: int = CHUNK_SAMPLES,
        drain_timeout_seconds: float = DEFAULT_DRAIN_TIMEOUT_SECONDS,
        finish_timeout_seconds: float = DEFAULT_FINISH_TIMEOUT_SECONDS,
        finish_quiet_seconds: float = DEFAULT_FINISH_QUIET_SECONDS,
    ):
        self._socket = socket
        self._language = language or "zh-CN"
        self.connect_id = connect_id
        # 服务端 X-Tt-Logid：排查问题时提供给火山侧定位，不含任何凭据
        self.log_id = log_id
        self._chunk_samples = max(1, int(chunk_samples))
        self._drain_timeout = max(0.0, float(drain_timeout_seconds))
        self._finish_timeout = max(0.0, float(finish_timeout_seconds))
        self._finish_quiet = max(0.0, float(finish_quiet_seconds))

        self._buffer = np.empty(0, dtype=np.int16)
        self._committed: list[str] = []
        self._pending = ""
        self._server_text = ""
        self._last_text = ""
        self._audio_duration_ms = 0
        self._server_final = False
        self._closed = False
        # 「逻辑已关闭」与「socket 已关闭」必须分开记：
        # 错误帧会把 _closed 置真（让等待循环立刻退出），但 socket 还得真关，否则连接与后台线程泄漏
        self._socket_closed = False
        self._close_code: Optional[int] = None
        self._close_reason = ""
        self._error: Optional[SaucSessionError] = None

    # ---------------- 对外契约 ----------------

    def append_audio(self, pcm16k: np.ndarray) -> StreamingTranscriptionUpdate:
        """追加 16k 单声道 PCM（int16），返回当前整场文本。"""
        if self._error is not None:
            raise self._error
        if self._closed:
            raise self._closed_error()

        samples = np.asarray(pcm16k, dtype=np.int16).reshape(-1)
        if samples.size:
            self._buffer = (
                samples if self._buffer.size == 0 else np.concatenate((self._buffer, samples))
            )

        self._send_buffered(final=False)
        self._pump(self._drain_timeout)

        if self._error is not None:
            raise self._error
        if self._closed:
            raise self._closed_error()

        return self._current_update()

    def finish(self) -> StreamingTranscriptionUpdate:
        """发送末包并收尾；若中途已出错则直接抛出归一化错误。"""
        try:
            if self._error is None and not self._closed:
                self._send_buffered(final=True)
                self._pump(
                    self._finish_timeout,
                    until_final=True,
                    quiet_seconds=self._finish_quiet,
                )
        finally:
            self.close()

        if self._error is not None:
            raise self._error

        return StreamingTranscriptionUpdate(
            text=self.final_text(),
            language=self._language,
            is_final=True,
        )

    def cancel(self) -> None:
        """用户主动取消：不算失败。"""
        self._closed = True
        self._close_socket(code=1000, reason="client cancel")

    def close(self) -> None:
        """关闭连接（幂等）：无论此前是否因错误帧被标记为逻辑关闭，都要真关 socket。"""
        self._closed = True
        self._close_socket()

    def _close_socket(self, code: Optional[int] = None, reason: str = "") -> None:
        if self._socket_closed:
            return

        self._socket_closed = True
        try:
            if code is None:
                self._socket.close()
            else:
                self._socket.close(code, reason)
        except Exception as error:  # 关闭失败不影响调用方语义
            logger.debug("关闭豆包流式连接失败", error=str(error))

    # ---------------- 结果状态 ----------------

    @property
    def audio_duration_ms(self) -> int:
        """服务端回报的计费时长（毫秒）。"""
        return self._audio_duration_ms

    @property
    def committed_text(self) -> str:
        """已确认（definite）的文本前缀，用于观测定稿进度。"""
        return "".join(self._committed)

    @property
    def pending_text(self) -> str:
        """尚未定稿的中间结果。"""
        return self._pending

    @property
    def chunk_samples(self) -> int:
        """单帧采样数（默认 200ms @16kHz = 3200）。"""
        return self._chunk_samples

    def drain(self) -> int:
        """非阻塞清空接收队列，返回读取到的帧数（批量推流用）。"""
        return self._drain_queue()

    def final_text(self) -> str:
        """整场文字稿：已确认句 + 未定稿尾巴；都没有时回落到服务端累积文本。"""
        text = self._last_text or self._server_text
        return (text or "").strip()

    def _current_update(self) -> StreamingTranscriptionUpdate:
        return StreamingTranscriptionUpdate(
            text=self._last_text or self._server_text,
            language=self._language,
            is_final=False,
        )

    def _refresh_text(self) -> None:
        self._last_text = "".join(self._committed) + self._pending

    # ---------------- 推流 ----------------

    def _send_buffered(self, *, final: bool) -> None:
        """按 200ms 切帧发送；``final`` 时把余量作为末包发出（余量为空也发末包）。"""
        while self._buffer.size >= self._chunk_samples:
            chunk = self._buffer[: self._chunk_samples]
            self._buffer = self._buffer[self._chunk_samples :]
            self._send_audio(chunk.tobytes(), final=False)

        if not final:
            return

        tail = self._buffer.tobytes()
        self._buffer = self._buffer[:0]
        self._send_audio(tail, final=True)

    def _send_audio(self, payload: bytes, *, final: bool) -> None:
        frame = build_audio_request(payload, final=final)
        try:
            self._socket.send(frame)
        except ConnectionClosed as error:
            self._mark_closed(error)
            raise self._closed_error() from error
        except Exception as error:
            raise SaucSessionError(
                SaucErrorInfo(KIND_NETWORK, f"发送音频失败（{error}）", True, "重新开始录音")
            ) from error

    # ---------------- 收流 ----------------

    def _pump(
        self,
        timeout: float,
        *,
        until_final: bool = False,
        quiet_seconds: float = 0.0,
    ) -> None:
        """读取服务端帧。

        - ``until_final=False``：最多花 ``timeout`` 秒等结果，之后把队列里已到的帧读干净；
        - ``until_final=True``：最多花 ``timeout`` 秒等末包/关闭，收到结果后静默
          ``quiet_seconds`` 秒即认为服务端不会再回。
        """
        deadline = time.monotonic() + max(0.0, timeout)
        last_frame_at: Optional[float] = None

        while not (self._server_final or self._closed):
            remaining = deadline - time.monotonic()
            if remaining < 0:
                break

            wait = max(0.001, min(0.1, remaining))
            try:
                data = self._socket.recv(timeout=wait)
            except TimeoutError:
                if not until_final:
                    break
                if last_frame_at is not None and time.monotonic() - last_frame_at >= quiet_seconds:
                    break
                continue
            except ConnectionClosed as error:
                self._mark_closed(error)
                break

            last_frame_at = time.monotonic()
            if data is not None:
                self._handle_frame(data)

        self._drain_queue()

    def _drain_queue(self, max_frames: int = MAX_DRAIN_FRAMES) -> int:
        """非阻塞地把接收队列读干净（批量推流与收尾用）。"""
        count = 0
        while count < max_frames and not (self._server_final or self._closed):
            try:
                data = self._socket.recv(timeout=0.001)
            except TimeoutError:
                break
            except ConnectionClosed as error:
                self._mark_closed(error)
                break

            count += 1
            if data is not None:
                self._handle_frame(data)

        return count

    def _handle_frame(self, data: bytes) -> None:
        frame = parse_server_frame(data)

        if frame.kind == FRAME_ERROR:
            error_frame = frame.error
            code = error_frame.code if error_frame else 0
            info = classify_sauc_error(code, error_frame.message if error_frame else "")
            # 只记录错误码与可读文案，不打印 payload（避免把用户语音文本写进日志）
            logger.warning("豆包流式返回错误帧", code=code, kind=info.kind, connect_id=self.connect_id)
            self._error = SaucSessionError(info)
            self._closed = True
            return

        if frame.kind != FRAME_RESULT:
            # 结构异常/空载荷：静默忽略，绝不能因此断流
            return

        if frame.is_last:
            self._server_final = True

        result = frame.result
        if result is not None:
            self._apply_result(result)

    def _apply_result(self, result: SaucResultFrame) -> None:
        if result.audio_duration_ms > 0:
            self._audio_duration_ms = max(self._audio_duration_ms, result.audio_duration_ms)

        if result.text:
            self._server_text = result.text

        if not result.utterances:
            # 拿不到句级信息就不确认：只把累积文本减掉已确认前缀当作中间结果
            self._pending = pending_from_cumulative_text(self._committed, result.text)
            self._refresh_text()
            return

        advance = advance_definite_prefix(self._committed, result.utterances)
        self._committed = list(advance.committed)
        self._pending = advance.pending
        self._refresh_text()

    def _mark_closed(self, error: Exception) -> None:
        self._closed = True
        code = getattr(error, "code", None)
        reason = getattr(error, "reason", "") or ""
        if code is None:
            received = getattr(error, "rcvd", None)
            if received is not None:
                code = getattr(received, "code", None)
                reason = getattr(received, "reason", "") or reason

        self._close_code = code
        self._close_reason = reason

    def _closed_error(self) -> SaucSessionError:
        if self._error is not None:
            return self._error

        if self._close_code in (None, 1000):
            # 1000 常见于服务端 VAD 判定长时间静音后正常关闭会话，不是协议错误；
            # 但「建立后一两秒就断」同样会走到这里（可能是参数不被服务端接受），
            # 所以文案必须带上 code/reason，别把真实原因藏进一句「静音」里。
            detail = f"（code={self._close_code}"
            if self._close_reason:
                detail += f"，reason={self._close_reason}"
            detail += "）"
            return SaucSessionError(
                SaucErrorInfo(
                    KIND_NETWORK,
                    f"流式连接被服务端关闭{detail}；录音较久可能是静音超时，"
                    "刚建连就断请检查「强制断句静音时长」等设置",
                    True,
                    "重新开始录音",
                )
            )

        return SaucSessionError(classify_close_code(self._close_code, self._close_reason))


class DoubaoStreamingProvider(StreamingASRProvider):
    """豆包流式 provider：把 SAUC 协议包成 ``StreamingASRSession``。"""

    def __init__(
        self,
        *,
        api_key: str = "",
        resource_id: str = DEFAULT_RESOURCE_ID,
        endpoint: str = DEFAULT_ENDPOINT,
        connect_timeout_seconds: float = DEFAULT_CONNECT_TIMEOUT_SECONDS,
        chunk_ms: int = CHUNK_MS,
        end_window_size: int = 0,
        force_to_speech_time: int = 0,
        enable_itn: bool = True,
        enable_punc: bool = True,
        enable_ddc: bool = False,
        context_max_chars: int = ASR_CONTEXT_MAX_CHARS,
        uid: str = "",
        drain_timeout_seconds: float = DEFAULT_DRAIN_TIMEOUT_SECONDS,
        finish_timeout_seconds: float = DEFAULT_FINISH_TIMEOUT_SECONDS,
        finish_quiet_seconds: float = DEFAULT_FINISH_QUIET_SECONDS,
    ):
        self.api_key = api_key or ""
        self.resource_id = resource_id or DEFAULT_RESOURCE_ID
        self.endpoint = endpoint or DEFAULT_ENDPOINT
        self.connect_timeout_seconds = float(connect_timeout_seconds or DEFAULT_CONNECT_TIMEOUT_SECONDS)
        self.chunk_ms = int(chunk_ms or CHUNK_MS)
        self.end_window_size = int(end_window_size or 0)
        self.force_to_speech_time = int(force_to_speech_time or 0)
        self.enable_itn = bool(enable_itn)
        self.enable_punc = bool(enable_punc)
        self.enable_ddc = bool(enable_ddc)
        self.context_max_chars = int(context_max_chars or ASR_CONTEXT_MAX_CHARS)
        self.uid = uid or ""
        self.drain_timeout_seconds = float(drain_timeout_seconds)
        self.finish_timeout_seconds = float(finish_timeout_seconds)
        self.finish_quiet_seconds = float(finish_quiet_seconds)

        # 非常规取值预警：火山对 end_window_size 的支持范围以官方文档为准，
        # 实测 200–800ms 稳定；超出常用范围的值服务端可能直接断连（表现为
        # 「流式连接被服务端关闭」）。这里只告警不拦截，真实原因由 _closed_error 带出。
        if 0 < self.end_window_size < 200 or self.end_window_size > 1000:
            logger.warning(
                "end_window_size 超出常用取值范围（200–1000ms），服务端可能拒绝该值并直接断连",
                end_window_size=self.end_window_size,
            )

    @property
    def provider_type(self) -> str:
        return "doubao_asr_streaming"

    @property
    def model_name(self) -> str:
        return MODEL_NAME

    @property
    def chunk_samples(self) -> int:
        return max(1, int(SAMPLE_RATE * self.chunk_ms / 1000))

    # ---------------- 配置校验 ----------------

    def ensure_config(self) -> None:
        """校验接入配置；缺失时给出可执行的中文提示。"""
        _require_websockets()

        if not self.api_key.strip():
            raise ASRProviderError(
                "未配置火山引擎豆包语音 Access Token，请在「设置 → 语音识别」中填写后重试"
            )

        if not self.endpoint.strip():
            raise ASRProviderError("未配置豆包语音 WebSocket 接入点地址")

        if not self.resource_id.strip():
            raise ASRProviderError("未配置豆包语音 Resource ID")

    async def check_availability(self) -> dict:
        """只做配置校验（在线服务无需预热，真实连通性在建立会话时暴露）。"""
        self.ensure_config()
        return {
            "available": True,
            "backend": "doubao_sauc",
            "model_name": MODEL_NAME,
            "resource_id": self.resource_id,
            "endpoint": self.endpoint,
        }

    async def warmup(self) -> None:
        """在线 provider 的「预热」= 校验凭据，避免用户开了模型服务却连不上。"""
        await asyncio.to_thread(self.ensure_config)

    # ---------------- 会话 ----------------

    def _open_socket(self, connect_id: str) -> tuple[Any, str]:
        """握手并返回 (连接, 服务端 log id)。log id 只用于排查，不含密钥。"""
        headers = {
            "X-Api-Key": self.api_key,
            "X-Api-Resource-Id": self.resource_id,
            "X-Api-Connect-Id": connect_id,
        }
        logger.info(
            "连接豆包双向流式 ASR",
            endpoint=self.endpoint,
            resource_id=self.resource_id,
            connect_id=connect_id,
        )

        try:
            socket = _ws_connect(
                self.endpoint,
                open_timeout=self.connect_timeout_seconds,
                close_timeout=5,
                ping_interval=20,
                ping_timeout=20,
                max_size=2**22,
                **{_HEADER_KWARG: headers},
            )
        except InvalidStatus as error:
            status_code = getattr(getattr(error, "response", None), "status_code", 0) or 0
            reason = getattr(getattr(error, "response", None), "reason_phrase", "") or ""
            logger.error(
                "豆包流式握手失败",
                status_code=status_code,
                resource_id=self.resource_id,
                connect_id=connect_id,
            )
            raise SaucSessionError(classify_handshake_status(status_code, reason)) from error
        except SaucSessionError:
            raise
        except Exception as error:
            raise SaucSessionError(
                SaucErrorInfo(
                    KIND_NETWORK,
                    f"无法连接豆包语音服务（{error}）",
                    True,
                    "检查网络后重试",
                )
            ) from error

        log_id = ""
        try:
            response_headers = socket.response.headers
            log_id = response_headers.get("X-Tt-Logid") or response_headers.get("x-tt-logid") or ""
        except Exception:
            log_id = ""

        return socket, log_id

    def _create_session_sync(
        self,
        language: str,
        context: str,
        *,
        drain_timeout_seconds: Optional[float] = None,
    ) -> DoubaoStreamingSession:
        self.ensure_config()

        connect_id = str(uuid.uuid4())
        socket, log_id = self._open_socket(connect_id)

        try:
            payload = build_bigmodel_request(
                uid=self.uid or f"att-desktop-{connect_id[:8]}",
                language=language,
                sample_rate=SAMPLE_RATE,
                enable_itn=self.enable_itn,
                enable_punc=self.enable_punc,
                enable_ddc=self.enable_ddc,
                result_type="full",
                show_utterances=True,
                context_text=truncate_context_text(context, self.context_max_chars),
                end_window_size=self.end_window_size,
                force_to_speech_time=self.force_to_speech_time,
            )
            socket.send(build_full_client_request(payload))
        except Exception:
            try:
                socket.close()
            except Exception:
                pass
            raise

        return DoubaoStreamingSession(
            socket,
            language=normalize_language(language) or "zh-CN",
            connect_id=connect_id,
            log_id=log_id,
            chunk_samples=self.chunk_samples,
            drain_timeout_seconds=(
                self.drain_timeout_seconds
                if drain_timeout_seconds is None
                else drain_timeout_seconds
            ),
            finish_timeout_seconds=self.finish_timeout_seconds,
            finish_quiet_seconds=self.finish_quiet_seconds,
        )

    async def create_session(
        self,
        language: str = "zh",
        context: str = "",
    ) -> StreamingASRSession:
        return await asyncio.to_thread(self._create_session_sync, language, context)


class DoubaoASRProvider(ASRProvider):
    """豆包在线 ASR provider（批量入口）。

    「批量」只是把整段音频按 200ms 推给同一条 SAUC 会话，因此单段/多段录音
    也能用在线识别，不需要本地模型。
    """

    def __init__(
        self,
        *,
        api_key: str = "",
        resource_id: str = DEFAULT_RESOURCE_ID,
        endpoint: str = DEFAULT_ENDPOINT,
        connect_timeout_seconds: float = DEFAULT_CONNECT_TIMEOUT_SECONDS,
        chunk_ms: int = CHUNK_MS,
        end_window_size: int = 0,
        force_to_speech_time: int = 0,
        enable_itn: bool = True,
        enable_punc: bool = True,
        enable_ddc: bool = False,
        context_max_chars: int = ASR_CONTEXT_MAX_CHARS,
        uid: str = "",
    ):
        self.streaming_provider = DoubaoStreamingProvider(
            api_key=api_key,
            resource_id=resource_id,
            endpoint=endpoint,
            connect_timeout_seconds=connect_timeout_seconds,
            chunk_ms=chunk_ms,
            end_window_size=end_window_size,
            force_to_speech_time=force_to_speech_time,
            enable_itn=enable_itn,
            enable_punc=enable_punc,
            enable_ddc=enable_ddc,
            context_max_chars=context_max_chars,
            uid=uid,
        )

    @property
    def provider_type(self) -> str:
        return "doubao_asr"

    @property
    def model_name(self) -> str:
        return MODEL_NAME

    @property
    def api_key(self) -> str:
        return self.streaming_provider.api_key

    @property
    def resource_id(self) -> str:
        return self.streaming_provider.resource_id

    @property
    def endpoint(self) -> str:
        return self.streaming_provider.endpoint

    def capabilities(self) -> ASRCapabilities:
        return ASRCapabilities(
            streaming=True,
            vad=True,
            multi_language=True,
            offline_batch=True,
        )

    async def get_runtime_capabilities(
        self,
        recording_mode: str = "single",
    ) -> ASRRuntimeCapabilities:
        """在线 provider：模型服务不需要加载，可用性只取决于凭据是否配齐。"""
        reason: Optional[str] = None
        try:
            self.streaming_provider.ensure_config()
        except ASRProviderError as error:
            reason = str(error)

        if recording_mode == "streaming":
            return ASRRuntimeCapabilities(
                model_service_supported=True,
                model_service_mode="online",
                model_service_reason=reason,
                online_ready=reason is None,
                streaming_supported=True,
                streaming_available=reason is None,
                streaming_reason=reason,
                streaming_backend="doubao_sauc",
            )

        return ASRRuntimeCapabilities(
            model_service_supported=True,
            model_service_mode="online",
            model_service_reason=reason,
            online_ready=reason is None,
            streaming_supported=False,
            streaming_available=False,
            streaming_reason="当前录音模式不是流式",
            streaming_backend="doubao_sauc",
        )

    async def warmup(self) -> None:
        await self.streaming_provider.warmup()

    async def close(self) -> None:
        """在线 provider 无常驻连接，无需释放资源。"""

    # ---------------- 批量转写 ----------------

    def _transcribe_sync(
        self,
        audio_path: str,
        language: str,
        context: str,
    ) -> tuple[str, int, float]:
        samples, duration = _read_pcm16_mono(audio_path)
        provider = self.streaming_provider

        # 批量推流不需要每包等结果，靠周期性清队列避免反向背压
        session = provider._create_session_sync(
            language,
            context,
            drain_timeout_seconds=0.0,
        )

        try:
            if samples.size == 0:
                raise ASRProviderError("音频为空，无法转写")

            chunk_samples = session.chunk_samples
            chunk_count = 0

            for start in range(0, samples.size, chunk_samples):
                session.append_audio(samples[start : start + chunk_samples])
                chunk_count += 1
                if chunk_count % BATCH_DRAIN_INTERVAL_CHUNKS == 0:
                    session.drain()

            update = session.finish()
        finally:
            session.close()

        return update.text, session.audio_duration_ms, duration

    async def transcribe(
        self,
        audio_path: str,
        language: str = "zh",
        **kwargs,
    ) -> TranscriptionResult:
        context = str(kwargs.get("context") or "")
        started_at = time.monotonic()

        text, _audio_duration_ms, duration = await asyncio.to_thread(
            self._transcribe_sync,
            str(audio_path),
            language,
            context,
        )

        processing_time = time.monotonic() - started_at
        segments: list[TranscriptionSegment] = []
        if text:
            segments.append(
                TranscriptionSegment(
                    start=0.0,
                    end=float(duration),
                    text=text,
                    confidence=1.0,
                    language=language or "zh",
                )
            )

        return TranscriptionResult(
            segments=segments,
            language=language or "zh",
            language_probability=1.0,
            duration=float(duration),
            processing_time=processing_time,
        )


def _read_pcm16_mono(audio_path: str) -> tuple[np.ndarray, float]:
    """读取任意支持格式的音频为 16k 单声道 int16，并返回时长（秒）。

    主路径用 soundfile（wav/mp3/flac/ogg），不支持的容器（m4a/webm）与需要重采样时
    回退 pydub（ffmpeg）—— 录音落盘本来就是 16k 单声道 wav，正常不会走到回退。
    """
    try:
        import soundfile as sf

        data, sample_rate = sf.read(str(audio_path), dtype="int16", always_2d=True)
    except Exception as soundfile_error:
        return _read_pcm16_mono_with_pydub(audio_path, soundfile_error)

    if data.size == 0:
        return np.empty(0, dtype=np.int16), 0.0

    mono = data[:, 0] if data.shape[1] == 1 else data.mean(axis=1).astype(np.int16)
    mono = np.ascontiguousarray(mono, dtype=np.int16)

    if int(sample_rate) == SAMPLE_RATE:
        return mono, mono.size / float(SAMPLE_RATE)

    try:
        from pydub import AudioSegment

        segment = AudioSegment(
            data=mono.tobytes(),
            sample_width=2,
            frame_rate=int(sample_rate),
            channels=1,
        ).set_frame_rate(SAMPLE_RATE)
        resampled = np.frombuffer(segment.raw_data, dtype=np.int16).copy()
        return resampled, resampled.size / float(SAMPLE_RATE)
    except Exception as error:
        raise ASRProviderError(f"音频重采样失败：{error}") from error


def _read_pcm16_mono_with_pydub(
    audio_path: str,
    cause: Exception,
) -> tuple[np.ndarray, float]:
    """soundfile 读不了时用 ffmpeg（pydub）兜底。"""
    try:
        from pydub import AudioSegment

        segment = (
            AudioSegment.from_file(str(audio_path))
            .set_frame_rate(SAMPLE_RATE)
            .set_channels(1)
        )
    except Exception as error:
        raise ASRProviderError(f"读取音频失败：{cause}") from error

    samples = np.frombuffer(segment.raw_data, dtype=np.int16).copy()
    return samples, len(segment) / 1000.0
