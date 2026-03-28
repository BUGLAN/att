"""
ASR provider 工厂
"""
from app.config import settings, user_settings
from app.stt.base import ASRProvider
from app.stt.models import ASRConfigError, ASRProviderConfig, ASRProviderType
from app.stt.providers import QwenASRProvider, WhisperProvider


def get_asr_provider_config(runtime_settings: dict | None = None) -> ASRProviderConfig:
    """从环境变量和用户设置构建运行时 ASR 配置"""
    source = {
        "asr_provider": settings.asr_provider,
        "asr_language": settings.asr_language,
        "whisper_model": settings.whisper_model,
        "whisper_device": settings.whisper_device,
        "whisper_compute_type": settings.whisper_compute_type,
        "qwen_asr_model": settings.qwen_asr_model,
        "qwen_asr_device": settings.qwen_asr_device,
        "qwen_asr_dtype": settings.qwen_asr_dtype,
        "qwen_asr_max_new_tokens": settings.qwen_asr_max_new_tokens,
        "qwen_asr_return_timestamps": settings.qwen_asr_return_timestamps,
        "qwen_asr_use_forced_aligner": settings.qwen_asr_use_forced_aligner,
        "qwen_asr_forced_aligner_model": settings.qwen_asr_forced_aligner_model,
        "qwen_asr_max_audio_seconds": settings.qwen_asr_max_audio_seconds,
        "qwen_streaming_backend": settings.qwen_streaming_backend,
        "qwen_streaming_base_url": settings.qwen_streaming_base_url,
        "qwen_streaming_timeout_seconds": settings.qwen_streaming_timeout_seconds,
        **user_settings.get_all()
    }

    if runtime_settings:
        source.update(runtime_settings)

    provider_value = str(source.get("asr_provider", ASRProviderType.WHISPER.value)).lower()
    try:
        provider = ASRProviderType(provider_value)
    except ValueError as error:
        raise ASRConfigError(f"不支持的 ASR provider: {provider_value}") from error

    return ASRProviderConfig(
        provider=provider,
        language=source.get("asr_language", "zh"),
        whisper={
            "model_size": source.get("whisper_model", settings.whisper_model),
            "device": source.get("whisper_device", settings.whisper_device),
            "compute_type": source.get("whisper_compute_type", settings.whisper_compute_type),
            "download_root": str(settings.faster_whisper_models_path)
        },
        qwen_asr={
            "model_name": source.get("qwen_asr_model", settings.qwen_asr_model),
            "device": source.get("qwen_asr_device", settings.qwen_asr_device),
            "dtype": source.get("qwen_asr_dtype", settings.qwen_asr_dtype),
            "cache_dir": str(settings.qwen_models_path),
            "max_new_tokens": int(source.get("qwen_asr_max_new_tokens", settings.qwen_asr_max_new_tokens)),
            "return_timestamps": bool(source.get("qwen_asr_return_timestamps", settings.qwen_asr_return_timestamps)),
            "use_forced_aligner": bool(source.get("qwen_asr_use_forced_aligner", settings.qwen_asr_use_forced_aligner)),
            "forced_aligner_model": source.get(
                "qwen_asr_forced_aligner_model",
                settings.qwen_asr_forced_aligner_model
            ),
            "max_audio_seconds": int(source.get("qwen_asr_max_audio_seconds", settings.qwen_asr_max_audio_seconds)),
            "streaming_backend": source.get("qwen_streaming_backend", settings.qwen_streaming_backend),
            "streaming_base_url": source.get("qwen_streaming_base_url", settings.qwen_streaming_base_url),
            "streaming_timeout_seconds": float(
                source.get("qwen_streaming_timeout_seconds", settings.qwen_streaming_timeout_seconds)
            )
        }
    )


def create_asr_provider(config: ASRProviderConfig) -> ASRProvider:
    """根据配置创建 ASR provider 实例"""
    if config.provider == ASRProviderType.WHISPER:
        return WhisperProvider(**config.whisper)

    if config.provider == ASRProviderType.QWEN_ASR:
        return QwenASRProvider(**config.qwen_asr)

    raise ASRConfigError(f"未实现的 ASR provider: {config.provider.value}")
