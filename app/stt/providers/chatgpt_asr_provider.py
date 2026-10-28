"""ChatGPT 听写 ASR provider（在线批量转写，无本地模型）。

实现移植自 Wangnov/codex-asr（MIT License，https://github.com/Wangnov/codex-asr
`src/lib.rs` 的 `CodexAsrClient`），按 ATT 的 provider 约定改写为 Python：

- 端点：``POST https://chatgpt.com/backend-api/transcribe``（ChatGPT 网页听写
  接口的逆向封装，见《ASR接口整合说明.md》）。**鉴权用的是 ChatGPT 账号
  access token（网页登录态），不是 platform.openai.com 的 API Key**；
- 请求：multipart 只带 ``file``（文件名 + 按后缀推断的 MIME）与可选的
  ``language`` 文本字段，不带 session/attempt 等字段（codex-asr 实测不需要）；
- 鉴权头四件套（缺一会被边缘风控拦截成 403 HTML）：
  ``Authorization: Bearer <token>``、``originator: Codex Desktop``、
  ``User-Agent: Codex Desktop/<版本> (<os>; <arch>)``、
  ``ChatGPT-Account-Id: <从 JWT 提取的 chatgpt_account_id>``；
- 响应：``{"text": …, "asset_pointer": …, "asset_ttl": …, "asset_format": …}``，
  只取 ``text``；text 为空是合法结果（音频里没有语音）；
- 限制：非流式、无时间戳；官方建议单文件 < 11 分钟，超长音频上游可能拒绝。

ATT 侧的存储约定（与豆包 Access Token 完全一致）：

- token 明文只写不读：PUT 设置时加密成 ``chatgpt_asr_api_key_encrypted`` 落盘，
  GET /api/settings 只回报 ``chatgpt_asr_api_key_set``；
- token 等同于账号登录态，绝不进日志与异常文案。
"""
from __future__ import annotations

import asyncio
import base64
import io
import json
import platform
import time
from pathlib import Path
from typing import Optional

import httpx

from app.records.models import TranscriptionResult, TranscriptionSegment
from app.stt.base import ASRProvider
from app.stt.models import (
    ASRCapabilities,
    ASRProviderError,
    ASRRuntimeCapabilities,
)
from app.utils.logging import get_logger

logger = get_logger(__name__)

# ---- 移植常量（codex-asr src/lib.rs）----
DEFAULT_ENDPOINT = "https://chatgpt.com/backend-api/transcribe"
DEFAULT_ORIGINATOR = "Codex Desktop"
DEFAULT_DESKTOP_VERSION = "26.429.30905"
DEFAULT_TIMEOUT_SECONDS = 120.0

# 官方建议单文件 < 11 分钟（超长上游可能直接拒绝，提前给可读提示）
RECOMMENDED_MAX_AUDIO_SECONDS = 11 * 60

# 文档说明支持的容器（codex-asr infer_content_type 的后缀 → MIME 映射）
_SUFFIX_MIME = {
    ".wav": "audio/wav",
    ".wave": "audio/wav",
    ".webm": "audio/webm",
    ".mp3": "audio/mpeg",
    ".m4a": "audio/mp4",
    ".mp4": "audio/mp4",
    ".ogg": "audio/ogg",
    ".oga": "audio/ogg",
    ".flac": "audio/flac",
}


def _normalize_arch() -> str:
    """Rust `env::consts::ARCH` 风格的架构名（x86_64 / aarch64）。"""
    machine = platform.machine().lower()
    if machine in ("amd64", "x86_64", "x64"):
        return "x86_64"
    if machine in ("arm64", "aarch64"):
        return "aarch64"
    return machine or "unknown"


def strip_bearer_prefix(token: str) -> str:
    """容忍用户连着 `Bearer ` 前缀一起粘贴（codex-asr strip_bearer_prefix）。"""
    value = (token or "").strip()
    if value[:7].lower() == "bearer ":
        return value[7:].strip()
    return value


def account_id_from_access_token(access_token: str) -> str:
    """从 JWT payload 提取 chatgpt_account_id（codex-asr 同名函数）。

    提取失败返回空串——此时不带 ChatGPT-Account-Id 头（codex-asr 同样允许缺省）。
    """
    try:
        payload = access_token.split(".")[1]
        payload += "=" * (-len(payload) % 4)
        claims = json.loads(base64.urlsafe_b64decode(payload))
        value = claims["https://api.openai.com/auth"]["chatgpt_account_id"]
        return value if isinstance(value, str) and value else ""
    except Exception:
        return ""


class ChatGPTASRProvider(ASRProvider):
    """ChatGPT 听写 provider（批量入口；协议细节只存在于本文件）。"""

    def __init__(
        self,
        *,
        api_key: str = "",
        endpoint: str = DEFAULT_ENDPOINT,
        timeout_seconds: float = DEFAULT_TIMEOUT_SECONDS,
    ):
        self.api_key = strip_bearer_prefix(api_key)
        self.endpoint = (endpoint or DEFAULT_ENDPOINT).strip()
        self.timeout_seconds = float(timeout_seconds or DEFAULT_TIMEOUT_SECONDS)

    @property
    def provider_type(self) -> str:
        return "chatgpt_asr"

    @property
    def model_name(self) -> str:
        # 上游没有可选模型，固定标识为听写接口名
        return "chatgpt-dictation"

    def capabilities(self) -> ASRCapabilities:
        return ASRCapabilities(
            streaming=False,
            multi_language=True,
            offline_batch=True,
        )

    async def get_runtime_capabilities(
        self,
        recording_mode: str = "single",
    ) -> ASRRuntimeCapabilities:
        """在线 provider：凭据配齐即「已就绪」，不需要用户点「开启模型」（同豆包）。"""
        reason: Optional[str] = None
        try:
            self.ensure_config()
        except ASRProviderError as error:
            reason = str(error)

        if recording_mode == "streaming":
            return ASRRuntimeCapabilities(
                model_service_supported=False,
                model_service_mode="online",
                model_service_reason="ChatGPT 听写暂不支持流式录音，请使用单段或多段录音模式",
                online_ready=False,
                streaming_supported=False,
                streaming_available=False,
                streaming_reason="ChatGPT 听写暂不支持流式录音",
            )

        return ASRRuntimeCapabilities(
            model_service_supported=True,
            model_service_mode="online",
            model_service_reason=reason,
            online_ready=reason is None,
            streaming_supported=False,
            streaming_available=False,
            streaming_reason="ChatGPT 听写为批量接口，不支持流式",
        )

    # ---------------- 配置校验 ----------------

    def ensure_config(self) -> None:
        """校验接入配置；缺失时给出可执行的中文提示。"""
        if not self.api_key.strip():
            raise ASRProviderError(
                "未配置 ChatGPT 访问令牌，请在「设置 → 语音识别」中填写后重试"
            )
        if not self.endpoint.strip():
            raise ASRProviderError("未配置 ChatGPT 听写端点地址")
        if not self.endpoint.startswith(("http://", "https://")):
            raise ASRProviderError("ChatGPT 听写端点必须以 http:// 或 https:// 开头")

    async def warmup(self) -> None:
        """在线 provider 的「预热」= 校验凭据（真实连通性在转写时暴露）。"""
        await asyncio.to_thread(self.ensure_config)

    async def close(self) -> None:
        """无常驻连接，无需释放资源。"""

    # ---------------- 鉴权头 ----------------

    def _auth_headers(self) -> dict[str, str]:
        """codex-asr auth_headers 的移植：四件套缺一会被边缘风控拦成 403。"""
        headers = {
            "Authorization": f"Bearer {self.api_key}",
            "originator": DEFAULT_ORIGINATOR,
            "User-Agent": (
                f"{DEFAULT_ORIGINATOR}/{DEFAULT_DESKTOP_VERSION} "
                f"({platform.system().lower()}; {_normalize_arch()})"
            ),
        }
        account_id = account_id_from_access_token(self.api_key)
        if account_id:
            headers["ChatGPT-Account-Id"] = account_id
        return headers

    # ---------------- 批量转写 ----------------

    async def transcribe(
        self,
        audio_path: str | Path,
        language: str = "zh",
        **kwargs,
    ) -> TranscriptionResult:
        started_at = time.monotonic()
        text, duration = await asyncio.to_thread(
            self._transcribe_sync, str(audio_path), language
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

    def _transcribe_sync(self, audio_path: str, language: str) -> tuple[str, float]:
        self.ensure_config()
        filename, content_type, payload, duration = _load_audio_for_upload(audio_path)

        if duration > RECOMMENDED_MAX_AUDIO_SECONDS:
            logger.warning(
                "音频超过 ChatGPT 听写建议上限（11 分钟），上游可能拒绝",
                audio_seconds=round(duration),
            )

        # multipart：file + 可选 language（codex-asr transcribe_bytes 的字段集）
        data = {}
        if language and language.strip() and language.strip().lower() != "auto":
            data["language"] = language.strip()

        logger.info(
            "调用 ChatGPT 听写接口",
            endpoint=self.endpoint,
            audio_seconds=round(duration, 2),
            with_language=bool(data),
        )

        try:
            response = httpx.post(
                self.endpoint,
                headers=self._auth_headers(),
                files={"file": (filename.replace('"', ""), payload, content_type)},
                data=data,
                timeout=httpx.Timeout(self.timeout_seconds, connect=10.0),
            )
        except httpx.ConnectError as error:
            raise ASRProviderError(
                f"无法连接听写服务（{self.endpoint}），请检查网络或代理设置"
            ) from error
        except httpx.HTTPError as error:
            raise ASRProviderError(
                f"听写请求失败（{error.__class__.__name__}），请稍后重试"
            ) from error

        if response.status_code != 200:
            raise _classify_status(response.status_code, response.text)

        try:
            body = response.json()
        except ValueError as error:
            raise ASRProviderError("听写接口返回了无法解析的响应（非 JSON）") from error

        text = str(body.get("text") or "").strip()
        if not text:
            logger.info("听写结果为空（音频可能没有语音内容）")
        return text, duration


def _classify_status(status_code: int, body: str) -> ASRProviderError:
    """把上游 HTTP 状态归一成可执行的中文提示（不回显完整 body，避免噪声）。"""
    is_html = body.lstrip().startswith("<")
    if status_code == 401:
        return ASRProviderError(
            "ChatGPT 访问令牌无效或已过期（HTTP 401），请重新获取令牌后更新设置"
        )
    if status_code == 403:
        if is_html:
            return ASRProviderError(
                "请求被 ChatGPT 风控拦截（HTTP 403），请稍后重试；"
                "频繁出现请更换网络环境或重新登录获取新令牌"
            )
        return ASRProviderError(
            "ChatGPT 拒绝了该请求（HTTP 403），令牌可能缺少听写权限，请重新获取"
        )
    if status_code == 404:
        return ASRProviderError("听写端点不存在（HTTP 404），请检查端点地址设置")
    if status_code == 413:
        return ASRProviderError("音频文件过大，被听写接口拒绝（HTTP 413）")
    if status_code == 429:
        return ASRProviderError(
            "听写请求被限流（HTTP 429），请稍后重试"
        )
    if status_code >= 500:
        return ASRProviderError(f"听写服务故障（HTTP {status_code}），请稍后重试")

    detail = body.strip()[:200]
    return ASRProviderError(
        f"听写请求失败（HTTP {status_code}）" + (f"：{detail}" if detail else "")
    )


def _load_audio_for_upload(audio_path: str) -> tuple[str, str, bytes, float]:
    """读取音频为可直接上传的 (文件名, MIME, 字节, 时长秒)。

    - 后缀在支持列表内：原样上传（录音落盘是 16k 单声道 wav，走这条快路径）；
    - 其它后缀：用 pydub（ffmpeg）转成 16k 单声道 wav 再上传，保证任意输入
      格式都能被端点接受。
    """
    path = Path(audio_path)
    mime = _SUFFIX_MIME.get(path.suffix.lower())

    if mime is not None and path.exists():
        payload = path.read_bytes()
        return path.name, mime, payload, _audio_duration_or_zero(path)

    try:
        from pydub import AudioSegment

        segment = (
            AudioSegment.from_file(str(path))
            .set_frame_rate(16000)
            .set_channels(1)
            .set_sample_width(2)
        )
    except Exception as error:
        raise ASRProviderError(f"读取音频失败（{path.name}）：{error}") from error

    buffer = io.BytesIO()
    segment.export(buffer, format="wav")
    return f"{path.stem}.wav", "audio/wav", buffer.getvalue(), len(segment) / 1000.0


def _audio_duration_or_zero(path: Path) -> float:
    """尽力而为的时长探测：失败返回 0（时长只用于展示，不参与计费）。"""
    try:
        import soundfile as sf

        return float(sf.info(str(path)).duration)
    except Exception:
        try:
            from pydub import AudioSegment

            return len(AudioSegment.from_file(str(path))) / 1000.0
        except Exception:
            return 0.0
