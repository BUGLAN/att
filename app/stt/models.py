"""
ASR provider 相关模型
"""
from dataclasses import dataclass, field
from enum import Enum
from typing import Any


class ASRProviderType(str, Enum):
    """支持的 ASR provider 类型"""
    WHISPER = "whisper"
    QWEN_ASR = "qwen_asr"


@dataclass(frozen=True)
class ASRCapabilities:
    """ASR 能力声明"""
    streaming: bool = False
    word_timestamps: bool = False
    vad: bool = False
    diarization: bool = False
    offline_batch: bool = True
    multi_language: bool = True


@dataclass
class ASRProviderConfig:
    """运行时 ASR 配置"""
    provider: ASRProviderType = ASRProviderType.WHISPER
    language: str = "zh"
    whisper: dict[str, Any] = field(default_factory=dict)
    qwen_asr: dict[str, Any] = field(default_factory=dict)


class ASRProviderError(RuntimeError):
    """ASR provider 运行异常"""


class ASRConfigError(ASRProviderError):
    """ASR 配置异常"""


class ASRCapabilityError(ASRProviderError):
    """ASR 能力不满足时的异常"""
