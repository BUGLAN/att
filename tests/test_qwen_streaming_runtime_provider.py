from app.stt.models import ASRProviderError
from app.stt.providers.qwen_streaming_runtime_provider import QwenStreamingProvider


def test_qwen_streaming_provider_prefers_remote_bridge_when_base_url_exists():
    provider = QwenStreamingProvider(
        model_name="Qwen/Qwen3-ASR-1.7B",
        cache_dir="models/qwen",
        streaming_backend="auto",
        streaming_base_url="http://127.0.0.1:9000",
    )

    assert provider.backend_type == "remote_bridge"


def test_qwen_streaming_provider_falls_back_to_local_vllm_without_base_url():
    provider = QwenStreamingProvider(
        model_name="Qwen/Qwen3-ASR-1.7B",
        cache_dir="models/qwen",
        streaming_backend="auto",
        streaming_base_url="",
    )

    assert provider.backend_type == "local_vllm"


def test_qwen_streaming_provider_rejects_invalid_backend():
    provider = QwenStreamingProvider(
        model_name="Qwen/Qwen3-ASR-1.7B",
        cache_dir="models/qwen",
        streaming_backend="invalid",
    )

    try:
        _ = provider.backend_type
    except ASRProviderError as error:
        assert "不支持的 Qwen 流式 backend" in str(error)
        return

    raise AssertionError("Expected ASRProviderError")
