"""火山引擎豆包双向流式 ASR（SAUC 二进制协议）编解码层。

本模块只做三件事，且全部是纯函数/纯数据，便于单测：

1. 请求体构造（``build_bigmodel_request`` / ``build_dialogue_context_json``）；
2. 帧编解码（``build_full_client_request`` / ``build_audio_request`` / ``parse_server_frame``）；
3. 结果推进与错误归一化（``advance_definite_prefix`` / ``classify_sauc_error`` /
   ``classify_close_code`` / ``classify_handshake_status``）。

协议要点（来源：火山官方双向流式 WebSocket 文档 + 调用文档 §4）：

- 4 字节头：byte0 = 协议版本(高 4 位) | 头长度(低 4 位)，
  byte1 = 消息类型(高 4 位) | flags(低 4 位)，
  byte2 = 序列化方式(高 4 位) | 压缩方式(低 4 位)，byte3 保留；
- 客户端帧体：[payload_size uint32 BE][payload]；
- 服务端结果帧体：[sequence int32?][payload_size uint32 BE][payload(JSON)]；
- 服务端错误帧体：[错误码 uint32 BE][错误信息长度 uint32 BE][错误信息 UTF-8]。

> 关于错误帧：调用文档里记成「头后多一个 序列化+压缩标志字节」，但官方协议与
> 多份可用实现（Python/Go 的 sauc demo）都是「头后直接跟错误码」。这里按官方布局解析，
> 并保留「多一个字节」形态的容错，避免换实现时直接拿不到错误码。
"""
from __future__ import annotations

import gzip
import json
import struct
from dataclasses import dataclass
from typing import Any, Optional, Sequence

from app.stt.models import ASRProviderError

# ==================== 协议常量 ====================

PROTOCOL_VERSION = 0b0001
HEADER_SIZE_UNITS = 0b0001
HEADER_BYTES = 4

MSG_FULL_CLIENT_REQUEST = 0b0001
MSG_AUDIO_ONLY_REQUEST = 0b0010
MSG_FULL_SERVER_RESPONSE = 0b1001
MSG_AUDIO_ONLY_RESPONSE = 0b1011
MSG_SERVER_ERROR_RESPONSE = 0b1111

FLAG_NO_SEQUENCE = 0b0000
FLAG_POS_SEQUENCE = 0b0001
FLAG_LAST_NO_SEQUENCE = 0b0010
FLAG_LAST_NEG_SEQUENCE = 0b0011
# flags 的 bit1 表示「末包」：0b0010（末包无序号）与 0b0011（末包负序号）都会置位
FLAG_LAST_MASK = 0b0010

SERIALIZATION_RAW = 0b0000
SERIALIZATION_JSON = 0b0001

COMPRESSION_NONE = 0b0000
COMPRESSION_GZIP = 0b0001

# 接入点与资源 ID（ASR 2.0 用 seedasr；不匹配会直接 403）
DEFAULT_ENDPOINT = "wss://openspeech.bytedance.com/api/v3/sauc/bigmodel"
DEFAULT_RESOURCE_ID = "volc.seedasr.sauc.duration"
MODEL_NAME = "bigmodel"

SAMPLE_RATE = 16000
CHANNELS = 1
BITS = 16
CHUNK_MS = 200
# 200ms / 16kHz / 单声道 / 16bit = 3200 采样 = 6400 字节
CHUNK_SAMPLES = SAMPLE_RATE * CHUNK_MS // 1000

# 对话上下文（corpus.context）上限，按码点计
ASR_CONTEXT_MAX_CHARS = 800

FRAME_RESULT = "result"
FRAME_ERROR = "error"
FRAME_UNKNOWN = "unknown"

# 错误归一化的 kind，与前端「去修复 / 立即重试」两类出口对齐
KIND_NONE = "none"
KIND_AUTH = "auth"
KIND_QUOTA = "quota"
KIND_SERVER = "server"
KIND_NETWORK = "network"
KIND_PROTOCOL = "protocol"
KIND_INPUT = "input"


# ==================== 数据结构 ====================


@dataclass(frozen=True)
class SaucUtterance:
    """服务端返回的单句信息。"""

    text: str
    start_ms: int = 0
    end_ms: int = 0
    definite: bool = False


@dataclass(frozen=True)
class SaucResultFrame:
    """已解析的服务端结果帧。"""

    text: str = ""
    utterances: tuple[SaucUtterance, ...] = ()
    audio_duration_ms: int = 0


@dataclass(frozen=True)
class SaucErrorFrame:
    """已解析的服务端错误帧。"""

    code: int = 0
    message: str = ""


@dataclass(frozen=True)
class SaucParsedFrame:
    """解析结果：kind ∈ {result, error, unknown}。

    ``unknown`` 表示结构异常或空载荷 —— 调用方必须静默忽略，不能因此断流。
    """

    kind: str = FRAME_UNKNOWN
    message_type: int = 0
    flags: int = 0
    sequence: Optional[int] = None
    result: Optional[SaucResultFrame] = None
    error: Optional[SaucErrorFrame] = None
    raw_payload: Optional[bytes] = None

    @property
    def is_last(self) -> bool:
        """服务端是否标记这是最后一个响应包。"""
        return bool(self.flags & FLAG_LAST_MASK)


@dataclass(frozen=True)
class SaucErrorInfo:
    """归一化后的错误信息。"""

    kind: str
    message: str
    retryable: bool = False
    next_action: str = ""
    provider_code: Optional[int] = None

    def to_message(self) -> str:
        """拼成可直接展示给用户的一句话。"""
        if self.next_action:
            return f"{self.message}（{self.next_action}）"
        return self.message


class SaucSessionError(ASRProviderError):
    """豆包流式会话异常：带归一化分类，便于上层决定「去修复」还是「立即重试」。"""

    def __init__(self, info: SaucErrorInfo):
        super().__init__(info.to_message())
        self.info = info
        self.kind = info.kind
        self.retryable = info.retryable
        self.next_action = info.next_action
        self.provider_code = info.provider_code


@dataclass(frozen=True)
class UtteranceAdvance:
    """一次「已确认前缀推进」的结果。"""

    committed: tuple[str, ...]
    new_finals: tuple[SaucUtterance, ...]
    pending: str


# ==================== 请求体构造 ====================


def truncate_context_text(context_text: str, max_chars: int = ASR_CONTEXT_MAX_CHARS) -> str:
    """按码点截尾：越靠近当前句的上下文偏置越强，同时避免切坏 emoji 的代理对。"""
    if not context_text:
        return ""

    chars = list(context_text)
    if len(chars) <= max_chars:
        return context_text

    return "".join(chars[-max_chars:])


def build_dialogue_context_json(context_text: str) -> str:
    """把已有文字稿封成 ``corpus.context`` 需要的 **JSON 字符串**。

    注意：``corpus.context`` 的类型是字符串（内容是 JSON）。
    直接塞纯文本会被服务端判成 45000001 并直接断连 —— 这是续录必挂的根因。
    空串返回 ""，调用方据此决定不发送 ``corpus`` 字段。
    """
    trimmed = truncate_context_text(context_text)
    if not trimmed:
        return ""

    return json.dumps(
        {
            "context_type": "dialog_ctx",
            "context_data": [{"text": trimmed}],
        },
        ensure_ascii=False,
    )


def normalize_language(language: str) -> str:
    """把应用内的短语言码映射成协议取值（协议用 zh-CN 这类 BCP-47 形式）。"""
    normalized = (language or "").strip()
    if not normalized:
        return "zh-CN"

    mapping = {
        "zh": "zh-CN",
        "zh-cn": "zh-CN",
        "cn": "zh-CN",
        "en": "en-US",
        "en-us": "en-US",
        "ja": "ja-JP",
        "ko": "ko-KR",
        "yue": "yue-CN",
        "auto": "",
    }
    return mapping.get(normalized.lower(), normalized)


def build_bigmodel_request(
    *,
    uid: str = "",
    language: str = "zh-CN",
    sample_rate: int = SAMPLE_RATE,
    enable_itn: bool = True,
    enable_punc: bool = True,
    enable_ddc: bool = False,
    result_type: str = "full",
    show_utterances: bool = True,
    context_text: str = "",
    end_window_size: int = 0,
    force_to_speech_time: int = 0,
) -> dict[str, Any]:
    """构造全量客户端请求体。

    ``result_type`` 固定 ``full``：``single`` 时 ``result.text`` 只跟随当前句且不补发
    final，文字稿会静默丢句；``show_utterances`` 固定 True：definite 判据依赖句级信息。
    """
    request: dict[str, Any] = {
        "model_name": MODEL_NAME,
        "enable_itn": bool(enable_itn),
        "enable_punc": bool(enable_punc),
        "enable_ddc": bool(enable_ddc),
        "result_type": result_type or "full",
        "show_utterances": bool(show_utterances),
    }

    # 强制断句：设置后服务端会在该静音时长后输出 definite，实时字幕能更早定稿
    if int(end_window_size or 0) > 0:
        request["end_window_size"] = int(end_window_size)
        request["force_to_speech_time"] = max(0, int(force_to_speech_time or 0))

    context_json = build_dialogue_context_json(context_text)
    if context_json:
        request["corpus"] = {"context": context_json}

    audio: dict[str, Any] = {
        "format": "pcm",
        "rate": int(sample_rate),
        "bits": BITS,
        "channel": CHANNELS,
    }
    normalized_language = normalize_language(language)
    if normalized_language:
        audio["language"] = normalized_language

    payload: dict[str, Any] = {"audio": audio, "request": request}
    if uid:
        payload["user"] = {"uid": uid}

    return payload


# ==================== 帧编解码 ====================


def build_header(
    message_type: int,
    flags: int = FLAG_NO_SEQUENCE,
    serialization: int = SERIALIZATION_RAW,
    compression: int = COMPRESSION_NONE,
) -> bytes:
    """构造 4 字节 SAUC 帧头。"""
    return bytes(
        [
            (PROTOCOL_VERSION << 4) | HEADER_SIZE_UNITS,
            ((message_type & 0x0F) << 4) | (flags & 0x0F),
            ((serialization & 0x0F) << 4) | (compression & 0x0F),
            0x00,
        ]
    )


def _maybe_gzip(payload: bytes, compress: bool) -> bytes:
    return gzip.compress(payload) if compress else payload


def build_full_client_request(payload: dict[str, Any], *, compress: bool = False) -> bytes:
    """全量客户端请求帧：JSON 序列化，可选 gzip。"""
    body = json.dumps(payload, ensure_ascii=False).encode("utf-8")
    body = _maybe_gzip(body, compress)
    header = build_header(
        MSG_FULL_CLIENT_REQUEST,
        FLAG_NO_SEQUENCE,
        SERIALIZATION_JSON,
        COMPRESSION_GZIP if compress else COMPRESSION_NONE,
    )
    return header + struct.pack(">I", len(body)) + body


def build_audio_request(
    pcm: bytes,
    *,
    final: bool = False,
    compress: bool = False,
) -> bytes:
    """音频帧：RAW 序列化；末包使用 flags=0b0010（末包无序号）。"""
    body = _maybe_gzip(bytes(pcm), compress)
    header = build_header(
        MSG_AUDIO_ONLY_REQUEST,
        FLAG_LAST_NO_SEQUENCE if final else FLAG_NO_SEQUENCE,
        SERIALIZATION_RAW,
        COMPRESSION_GZIP if compress else COMPRESSION_NONE,
    )
    return header + struct.pack(">I", len(body)) + body


def _gunzip_tolerant(payload: bytes, compression: int) -> bytes:
    """按头的压缩标志尝试解压，失败按原文处理。

    错误帧的压缩约定官方未明确：严格解压会让「连错误码都拿不到」，因此必须容错。
    """
    if compression != COMPRESSION_GZIP or not payload:
        return payload

    try:
        return gzip.decompress(payload)
    except OSError:
        return payload


def _parse_result_payload(payload: bytes) -> Optional[SaucResultFrame]:
    """解析结果帧 payload；结构异常返回 None（上层归类为 unknown）。"""
    try:
        decoded = json.loads(payload.decode("utf-8"))
    except (UnicodeDecodeError, ValueError):
        return None

    if not isinstance(decoded, dict):
        return None

    audio_info = decoded.get("audio_info")
    duration = 0
    if isinstance(audio_info, dict):
        duration = _safe_int(audio_info.get("duration"))

    result = decoded.get("result")
    text = ""
    utterances: list[SaucUtterance] = []

    if isinstance(result, dict):
        text = str(result.get("text") or "")
        raw_utterances = result.get("utterances")
        if isinstance(raw_utterances, list):
            for item in raw_utterances:
                if not isinstance(item, dict):
                    continue
                utterances.append(
                    SaucUtterance(
                        text=str(item.get("text") or ""),
                        start_ms=_safe_int(item.get("start_time")),
                        end_ms=_safe_int(item.get("end_time")),
                        definite=bool(item.get("definite")),
                    )
                )

    return SaucResultFrame(
        text=text,
        utterances=tuple(utterances),
        audio_duration_ms=duration,
    )


def _safe_int(value: Any) -> int:
    try:
        return int(value)
    except (TypeError, ValueError):
        return 0


def _read_error_at(data: bytes, offset: int) -> Optional[SaucErrorFrame]:
    """按 [code uint32][size uint32][message] 读错误帧，长度不吻合返回 None。"""
    if offset + 8 > len(data):
        return None

    code = struct.unpack(">I", data[offset : offset + 4])[0]
    size = struct.unpack(">I", data[offset + 4 : offset + 8])[0]
    if size > len(data) - offset - 8:
        return None

    # 火山错误码是 8 位十进制（如 45000001 / 55000031），用它做合理性校验
    if not 10_000_000 <= code <= 99_999_999:
        return None

    message = data[offset + 8 : offset + 8 + size].decode("utf-8", errors="replace")
    return SaucErrorFrame(code=code, message=message)


def _parse_error_frame(data: bytes, message_type: int, flags: int) -> SaucParsedFrame:
    """解析错误帧：官方布局优先，兼容「多一个标志字节」的形态。"""
    error: Optional[SaucErrorFrame] = None
    for offset in (HEADER_BYTES, HEADER_BYTES + 1):
        error = _read_error_at(data, offset)
        if error is not None:
            break

    if error is None:
        # 两种布局都不吻合：退化成「尽力取错误码」，避免丢失错误信息
        code = struct.unpack(">I", data[HEADER_BYTES : HEADER_BYTES + 4])[0] if len(data) >= 8 else 0
        error = SaucErrorFrame(code=code, message="")

    return SaucParsedFrame(
        kind=FRAME_ERROR,
        message_type=message_type,
        flags=flags,
        error=error,
    )


def parse_server_frame(data: bytes) -> SaucParsedFrame:
    """解析服务端帧。结构异常一律返回 kind=unknown（调用方静默忽略，不断流）。"""
    if not data or len(data) < HEADER_BYTES:
        return SaucParsedFrame(kind=FRAME_UNKNOWN)

    message_type = (data[1] >> 4) & 0x0F
    flags = data[1] & 0x0F
    serialization = (data[2] >> 4) & 0x0F
    compression = data[2] & 0x0F

    if message_type == MSG_SERVER_ERROR_RESPONSE:
        return _parse_error_frame(data, message_type, flags)

    if message_type not in (MSG_FULL_SERVER_RESPONSE, MSG_AUDIO_ONLY_RESPONSE):
        return SaucParsedFrame(kind=FRAME_UNKNOWN, message_type=message_type, flags=flags)

    position = HEADER_BYTES
    sequence: Optional[int] = None

    # flags 低 2 位为 0b01 / 0b11 时，头后先跟一个 int32 序号
    if (flags & 0b0011) in (FLAG_POS_SEQUENCE, FLAG_LAST_NEG_SEQUENCE):
        if position + 4 > len(data):
            return SaucParsedFrame(kind=FRAME_UNKNOWN, message_type=message_type, flags=flags)
        sequence = struct.unpack(">i", data[position : position + 4])[0]
        position += 4

    if position + 4 > len(data):
        return SaucParsedFrame(kind=FRAME_UNKNOWN, message_type=message_type, flags=flags)

    payload_size = struct.unpack(">I", data[position : position + 4])[0]
    position += 4

    if payload_size == 0 or position + payload_size > len(data):
        return SaucParsedFrame(
            kind=FRAME_UNKNOWN,
            message_type=message_type,
            flags=flags,
            sequence=sequence,
        )

    raw_payload = _gunzip_tolerant(data[position : position + payload_size], compression)

    if serialization != SERIALIZATION_JSON:
        return SaucParsedFrame(
            kind=FRAME_UNKNOWN,
            message_type=message_type,
            flags=flags,
            sequence=sequence,
            raw_payload=raw_payload,
        )

    result = _parse_result_payload(raw_payload)
    if result is None:
        return SaucParsedFrame(
            kind=FRAME_UNKNOWN,
            message_type=message_type,
            flags=flags,
            sequence=sequence,
            raw_payload=raw_payload,
        )

    return SaucParsedFrame(
        kind=FRAME_RESULT,
        message_type=message_type,
        flags=flags,
        sequence=sequence,
        result=result,
        raw_payload=raw_payload,
    )


# ==================== 结果推进 ====================


def advance_definite_prefix(
    committed: Sequence[str],
    utterances: Sequence[SaucUtterance],
) -> UtteranceAdvance:
    """按 definite 连续前缀推进已确认文本。

    三条硬规则：

    1. 每一帧结果都走前缀推进，**不做** ``text == last_text`` 短路 ——
       句子的 interim → definite 转移可以在累积文本不变的情况下发生，
       短路会把定稿事件整个吞掉（全程零 final）；
    2. 遇到第一个未定稿句就停，只把它之后的尾巴作为中间结果；
    3. 服务端回退（前缀变短）时同步收缩已确认列表，避免重复提交；
       但 ``utterances`` 为空属于「没有句级信息」，只代表本帧不推进，不能据此收缩。
    """
    if not utterances:
        return UtteranceAdvance(committed=tuple(committed), new_finals=(), pending="")

    prefix = 0
    while prefix < len(utterances) and utterances[prefix].definite:
        prefix += 1

    kept = list(committed[:prefix]) if prefix < len(committed) else list(committed)

    new_finals: list[SaucUtterance] = []
    for index in range(len(kept), prefix):
        utterance = utterances[index]
        if utterance.text:
            new_finals.append(utterance)
            kept.append(utterance.text)

    pending = "".join((utterance.text or "") for utterance in utterances[prefix:])

    return UtteranceAdvance(
        committed=tuple(kept),
        new_finals=tuple(new_finals),
        pending=pending,
    )


def pending_from_cumulative_text(committed: Sequence[str], text: str) -> str:
    """拿不到句级信息时的兜底：从累积文本里减掉已确认前缀。"""
    cumulative = text or ""
    joined = "".join(committed)
    if joined and cumulative.startswith(joined):
        return cumulative[len(joined) :]
    return cumulative


# ==================== 错误归一化 ====================


def classify_sauc_error(code: int, message: str = "") -> SaucErrorInfo:
    """服务端错误码 → 归一化错误（文案与下一步动作直接给用户看）。"""
    detail = f"：{message}" if message else ""

    if code == 20000000:
        return SaucErrorInfo(KIND_NONE, "成功", False, "", code)

    if code == 45000001:
        return SaucErrorInfo(
            KIND_PROTOCOL,
            f"请求参数无效{detail}",
            False,
            "检查服务商参数后重新测试连接",
            code,
        )

    if code == 45000002:
        return SaucErrorInfo(
            KIND_INPUT,
            "音频为空",
            False,
            "检查音频格式（16kHz 单声道 16bit）",
            code,
        )

    if code == 45000081:
        return SaucErrorInfo(
            KIND_NETWORK,
            "服务端等包超时",
            True,
            "立即重试",
            code,
        )

    if code == 45000151:
        return SaucErrorInfo(
            KIND_INPUT,
            "音频格式不正确",
            False,
            "检查音频格式（16kHz 单声道 16bit）",
            code,
        )

    if code == 55000031:
        return SaucErrorInfo(
            KIND_QUOTA,
            "服务端过载",
            True,
            "稍后重试，或检查并发配额",
            code,
        )

    if 55000000 <= code <= 55999999:
        return SaucErrorInfo(
            KIND_SERVER,
            f"服务端内部错误（{code}）{detail}",
            True,
            "立即重试",
            code,
        )

    return SaucErrorInfo(
        KIND_PROTOCOL,
        f"服务端返回未知错误（{code}）{detail}",
        False,
        "切换服务商或稍后重试",
        code,
    )


def classify_close_code(code: Optional[int], reason: str = "") -> SaucErrorInfo:
    """WebSocket 关闭码 → 归一化错误。

    服务端拒绝请求时常常不发错误帧、直接断开（corpus 非法即是一例），
    若一律归成 network/可重试，就会把「请求被拒」当成「网络抖动」反复退避。
    """
    if code is None:
        return SaucErrorInfo(KIND_NETWORK, "连接已断开（无关闭码）", True, "立即重试")

    if code == 1000:
        return SaucErrorInfo(KIND_NONE, "连接正常关闭", False, "")

    detail = f" {reason}" if reason else ""

    if code in (1002, 1003, 1007, 1008) or 4400 <= code <= 4499:
        return SaucErrorInfo(
            KIND_PROTOCOL,
            f"连接被服务端关闭（code {code}{detail}）",
            False,
            "检查服务商参数后重新测试连接",
        )

    if code in (1011, 1012, 1013):
        return SaucErrorInfo(
            KIND_SERVER,
            f"连接被服务端关闭（code {code}{detail}）",
            True,
            "立即重试",
        )

    return SaucErrorInfo(
        KIND_NETWORK,
        f"连接被服务端关闭（code {code}{detail}）",
        True,
        "立即重试",
    )


def classify_handshake_status(status_code: int, reason: str = "") -> SaucErrorInfo:
    """握手 HTTP 状态码 → 归一化错误。

    403 的典型原因是 Resource ID 与账号实际开通的产品不匹配，
    而服务端不会告诉你该换哪一个，所以文案里直接把规律写出来。
    """
    detail = f" {reason}".rstrip()

    if status_code == 401:
        return SaucErrorInfo(
            KIND_AUTH,
            f"鉴权失败：Access Token 无效或已过期（401{detail}）",
            False,
            "在设置中重新填写火山语音技术 Access Token",
            status_code,
        )

    if status_code == 403:
        return SaucErrorInfo(
            KIND_AUTH,
            f"鉴权失败：Resource ID 与账号开通的产品不匹配（403{detail}）",
            False,
            "ASR 1.0 用 volc.bigasr.sauc.*，ASR 2.0 用 volc.seedasr.sauc.*，逐个确认",
            status_code,
        )

    if status_code == 404:
        return SaucErrorInfo(
            KIND_PROTOCOL,
            f"接入点不存在（404{detail}）",
            False,
            "检查 WebSocket 接入点地址",
            status_code,
        )

    if status_code == 429:
        return SaucErrorInfo(
            KIND_QUOTA,
            f"请求过于频繁（429{detail}）",
            True,
            "稍后重试",
            status_code,
        )

    if 500 <= status_code <= 599:
        return SaucErrorInfo(
            KIND_SERVER,
            f"服务端异常（{status_code}{detail}）",
            True,
            "立即重试",
            status_code,
        )

    return SaucErrorInfo(
        KIND_NETWORK,
        f"握手失败（{status_code}{detail}）",
        True,
        "立即重试",
        status_code,
    )
