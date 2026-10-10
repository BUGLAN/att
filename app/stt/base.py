"""
ASR 抽象基类
"""
from abc import ABC, abstractmethod
from pathlib import Path

from app.records.models import TranscriptionResult
from app.stt.models import ASRCapabilities, ASRRuntimeCapabilities


class ASRProvider(ABC):
    """统一的 ASR provider 接口"""

    @property
    @abstractmethod
    def provider_type(self) -> str:
        """provider 类型标识"""

    @property
    @abstractmethod
    def model_name(self) -> str:
        """当前 provider 使用的模型名"""

    def capabilities(self) -> ASRCapabilities:
        """返回 provider 能力声明"""
        return ASRCapabilities()

    async def get_runtime_capabilities(
        self,
        recording_mode: str = "single"
    ) -> ASRRuntimeCapabilities:
        """返回当前录音模式下的运行时能力。"""
        if recording_mode == "streaming":
            if not self.capabilities().streaming:
                return ASRRuntimeCapabilities(
                    model_service_supported=False,
                    model_service_reason="当前 provider 不支持流式录音",
                    streaming_supported=False,
                    streaming_available=False,
                    streaming_reason="当前 provider 不支持流式录音",
                )

            return ASRRuntimeCapabilities(
                model_service_supported=True,
                streaming_supported=True,
                streaming_available=False,
                streaming_reason="流式运行时尚未就绪",
            )

        return ASRRuntimeCapabilities(
            model_service_supported=True,
            model_service_mode="provider",
            streaming_supported=False,
            streaming_available=False,
            streaming_reason="当前录音模式不是流式",
        )

    @abstractmethod
    async def transcribe(
        self,
        audio_path: str | Path,
        language: str = "zh",
        **kwargs
    ) -> TranscriptionResult:
        """转写单个音频文件"""

    async def warmup(self) -> None:
        """预热模型"""

    async def close(self) -> None:
        """释放资源"""
