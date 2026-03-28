"""
流式 ASR 抽象接口
"""
from abc import ABC, abstractmethod
from dataclasses import dataclass


@dataclass
class StreamingTranscriptionUpdate:
    """流式转写增量结果"""
    text: str
    language: str = ""
    is_final: bool = False


class StreamingASRSession(ABC):
    """单条流式 ASR 会话"""

    @abstractmethod
    def append_audio(self, pcm16k) -> StreamingTranscriptionUpdate:
        """追加 16k 单声道 PCM 音频"""

    @abstractmethod
    def finish(self) -> StreamingTranscriptionUpdate:
        """结束流式会话并返回最终结果"""


class StreamingASRProvider(ABC):
    """流式 ASR provider"""

    @property
    @abstractmethod
    def provider_type(self) -> str:
        """provider 标识"""

    @property
    @abstractmethod
    def model_name(self) -> str:
        """当前模型名"""

    @abstractmethod
    async def create_session(
        self,
        language: str = "zh",
        context: str = ""
    ) -> StreamingASRSession:
        """创建流式会话"""

    async def close(self) -> None:
        """释放资源"""
