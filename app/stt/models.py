"""
ASR provider 相关模型
"""
from dataclasses import dataclass, field
from enum import Enum
from typing import Any, Optional


class ASRProviderType(str, Enum):
    """支持的 ASR provider 类型"""
    WHISPER = "whisper"
    QWEN_ASR = "qwen_asr"
    DOUBAO_ASR = "doubao_asr"
    CHATGPT_ASR = "chatgpt_asr"


@dataclass(frozen=True)
class ASRCapabilities:
    """ASR 能力声明"""
    streaming: bool = False
    word_timestamps: bool = False
    vad: bool = False
    diarization: bool = False
    offline_batch: bool = True
    multi_language: bool = True


@dataclass(frozen=True)
class ASRRuntimeCapabilities:
    """ASR 运行时能力声明。"""
    model_service_supported: bool = True
    model_service_mode: str = "provider"
    model_service_reason: Optional[str] = None
    # 在线 provider（如豆包）：模型服务没有需要加载的本地模型，
    # 只要凭据配齐就算「已就绪」，前端不需要用户再点「开启模型」
    online_ready: bool = False
    streaming_supported: bool = False
    streaming_available: bool = False
    streaming_reason: Optional[str] = None
    streaming_backend: Optional[str] = None


@dataclass
class ASRProviderConfig:
    """运行时 ASR 配置"""
    provider: ASRProviderType = ASRProviderType.WHISPER
    language: str = "zh"
    whisper: dict[str, Any] = field(default_factory=dict)
    qwen_asr: dict[str, Any] = field(default_factory=dict)
    doubao_asr: dict[str, Any] = field(default_factory=dict)
    chatgpt_asr: dict[str, Any] = field(default_factory=dict)


class ASRProviderError(RuntimeError):
    """ASR provider 运行异常"""


class ASRConfigError(ASRProviderError):
    """ASR 配置异常"""
