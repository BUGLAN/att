"""服务商注册表回归测试（模型串组装规则）。

锁死的点（2026-10-22 实测踩坑：本地网关返回 group/deepseek-v4.1-flash 被原样透传，
LiteLLM 把首段 group 误当服务商前缀报 "LLM Provider NOT provided"）：

- custom（自定义 OpenAI 兼容）档案下，**任何**模型名都组装为 `openai/<完整模型ID>`，
  含 / 的名字也不例外；对已是 openai/ 开头的名字幂等；
- 其余服务商：裸名按前缀组装，含 / 的完整串原样透传。
"""
import pytest

from app.llm.registry import compose_model_string


def test_custom_always_wraps_openai_prefix():
    assert compose_model_string("custom", "deepseek-flash", "http://127.0.0.1:9999/v1") == "openai/deepseek-flash"
    assert (
        compose_model_string("custom", "group/deepseek-v4.1-flash", "http://127.0.0.1:9999/v1")
        == "openai/group/deepseek-v4.1-flash"
    ), "本地网关带斜杠的模型 ID 必须整串包进 openai/，绝不能把首段透传给 LiteLLM 当服务商"


def test_custom_is_idempotent_for_openai_prefixed_ids():
    assert (
        compose_model_string("custom", "openai/mistral", "http://127.0.0.1:9999/v1")
        == "openai/mistral"
    ), "上游模型 ID 本身叫 openai/xxx 时不应拼成 openai/openai/xxx"


def test_custom_requires_api_base():
    with pytest.raises(ValueError, match="API Base"):
        compose_model_string("custom", "deepseek-flash", None)
    with pytest.raises(ValueError, match="API Base"):
        compose_model_string("custom", "deepseek-flash", "   ")


def test_registry_providers_compose_and_passthrough():
    assert compose_model_string("deepseek", "deepseek-flash", None) == "deepseek/deepseek-flash"
    # 非 custom 服务商：完整串（含 /）原样透传是预期行为——前缀决定协议，后面交给上游
    assert compose_model_string("deepseek", "some-group/deepseek-flash", None) == "some-group/deepseek-flash"
    assert compose_model_string("ollama", "qwen3:14b", None) == "ollama_chat/qwen3:14b"


def test_unknown_provider_and_empty_name_raise():
    with pytest.raises(ValueError, match="未知服务商"):
        compose_model_string("nope", "x", None)
    with pytest.raises(ValueError, match="模型名不能为空"):
        compose_model_string("deepseek", "  ", None)
