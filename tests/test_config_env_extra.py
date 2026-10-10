"""`Settings` 读取 `.env` 的健壮性回归测试。

背景（实际踩到）：`.env` 是**全项目共用**的配置文件，除 `Settings` 字段外还包含
docker-compose（`VLLM_IMAGE` / `ATT_BACKEND_PORT`）与桥接脚本
（`QWEN_STREAMING_MODEL` / `QWEN_STREAMING_CHUNK_SIZE_SEC` …）读取的键。
pydantic-settings 对 env 文件里的未知键默认 `extra="forbid"`，因此照文档
「复制 .env.example 为 .env」会让后端直接 `ValidationError` 起不来。

这里锁死两条：
1. 直接拿仓库里的 `.env.example` 当 `.env` 加载，不得抛异常；
2. 已知字段仍要正常解析（不能因为忽略未知键就把配置整体丢掉）。
"""
from pathlib import Path

from app.config import Settings

PROJECT_ROOT = Path(__file__).resolve().parents[1]


def test_settings_tolerates_env_example_with_extra_keys(tmp_path):
    """`.env.example` 里存在非 Settings 字段时，加载不得报错。"""
    env_file = tmp_path / ".env"
    env_file.write_text(
        (PROJECT_ROOT / ".env.example").read_text(encoding="utf-8"),
        encoding="utf-8",
    )

    settings = Settings(_env_file=str(env_file))

    # 非 Settings 字段被忽略，但不能影响已知字段的解析
    assert settings.debug is False
    assert settings.asr_provider == "whisper"


def test_settings_ignores_unknown_keys_but_keeps_known_ones(tmp_path):
    env_file = tmp_path / ".env"
    env_file.write_text(
        "\n".join(
            [
                "VLLM_IMAGE=vllm/vllm-openai:v0.14.0",
                "ATT_BACKEND_PORT=8000",
                "QWEN_STREAMING_WARMUP=1",
                "ASR_PROVIDER=doubao_asr",
                "DOUBAO_ASR_RESOURCE_ID=volc.bigasr.sauc.duration",
                "MAX_UPLOAD_SIZE=1048576",
            ]
        ),
        encoding="utf-8",
    )

    settings = Settings(_env_file=str(env_file))

    assert settings.asr_provider == "doubao_asr"
    assert settings.doubao_asr_resource_id == "volc.bigasr.sauc.duration"
    assert settings.max_upload_size == 1048576


def test_settings_model_config_allows_extra():
    """契约护栏：不要把 extra 改回 forbid，否则 .env.example 直接复制即崩。"""
    assert Settings.model_config.get("extra") == "ignore"
