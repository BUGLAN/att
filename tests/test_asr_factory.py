from app.stt.factory import create_asr_provider, get_asr_provider_config
from app.stt.providers.qwen_asr_provider import QwenASRProvider
from app.stt.providers.whisper_provider import WhisperProvider


def test_create_whisper_provider_from_runtime_settings():
    config = get_asr_provider_config({
        "asr_provider": "whisper",
        "whisper_model": "small",
        "whisper_device": "cpu",
        "whisper_compute_type": "int8"
    })

    provider = create_asr_provider(config)

    assert isinstance(provider, WhisperProvider)
    assert provider.provider_type == "whisper"
    assert provider.model_name == "small"
    assert provider.device == "cpu"
    assert provider.download_root.endswith("models\\faster_whisper")


def test_create_qwen_provider_from_runtime_settings():
    config = get_asr_provider_config({
        "asr_provider": "qwen_asr",
        "qwen_asr_model": "Qwen/Qwen3-ASR-1.7B",
        "qwen_asr_device": "cpu",
        "qwen_asr_dtype": "float32",
        "qwen_asr_return_timestamps": True,
        "qwen_asr_use_forced_aligner": True,
        "qwen_asr_forced_aligner_model": "Qwen/Qwen3-ForcedAligner-0.6B",
        "qwen_streaming_backend": "remote_bridge",
        "qwen_streaming_base_url": "http://127.0.0.1:9000",
        "qwen_streaming_timeout_seconds": 12.5,
    })

    provider = create_asr_provider(config)

    assert isinstance(provider, QwenASRProvider)
    assert provider.provider_type == "qwen_asr"
    assert provider.model_name == "Qwen/Qwen3-ASR-1.7B"
    assert provider.cache_dir.endswith("models\\qwen")
    assert provider.return_timestamps is True
    assert provider.use_forced_aligner is True
    assert provider.streaming_backend == "remote_bridge"
    assert provider.streaming_base_url == "http://127.0.0.1:9000"
    assert provider.streaming_timeout_seconds == 12.5
