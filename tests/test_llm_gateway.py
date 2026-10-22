"""LiteLLMGateway 回归测试。

锁死的点（docs/LLM配置模块重设计.md §6.2/§12）：
1. 每次请求必须显式注入 `timeout` 与 `num_retries`（修复已知问题 #4：SDK 默认 600s）；
2. 异常按类名/状态码分级成 error_code，且文案先脱敏（AGENTS.md §3.4）；
3. 前缀未被 LiteLLM 收录时（如 doubao），自动回退 openai/ + 兼容端点重试一次；
4. 上游模型列表拉取的解析与失败兜底。
"""
import asyncio
from typing import Any

import pytest

from app.llm import gateway as gateway_module
from app.llm.errors import (
    CODE_AUTH,
    CODE_MODEL_NOT_FOUND,
    CODE_NETWORK,
    CODE_PROVIDER,
    CODE_QUOTA,
    CODE_TIMEOUT,
    CODE_UNSUPPORTED_PROVIDER,
    LLMError,
    classify_exception,
)
from app.llm.gateway import LiteLLMGateway, is_valid_model_string
from app.llm.registry import get_provider_spec


def _exc(name: str, message: str, status: int | None = None) -> Exception:
    """构造带指定类名/状态码的假异常（避免测试期 import litellm 的重依赖）。"""
    cls = type(name, (Exception,), {})
    instance = cls(message)
    if status is not None:
        instance.status_code = status
    return instance


# ==================== 错误分级 ====================


def test_classify_by_exception_class_name():
    assert classify_exception(_exc("AuthenticationError", "bad key")).code == CODE_AUTH
    assert classify_exception(_exc("NotFoundError", "no model")).code == CODE_MODEL_NOT_FOUND
    assert classify_exception(_exc("RateLimitError", "slow down")).code == CODE_QUOTA
    assert classify_exception(_exc("APITimeoutError", "timed out")).code == CODE_TIMEOUT
    assert classify_exception(_exc("APIConnectionError", "connection reset")).code == CODE_NETWORK


def test_classify_by_status_code_fallback():
    assert classify_exception(_exc("WeirdError", "nope", status=401)).code == CODE_AUTH
    assert classify_exception(_exc("WeirdError", "nope", status=404)).code == CODE_MODEL_NOT_FOUND
    assert classify_exception(_exc("WeirdError", "nope", status=429)).code == CODE_QUOTA
    assert classify_exception(_exc("WeirdError", "nope", status=503)).code == CODE_PROVIDER
    assert classify_exception(_exc("WeirdError", "nope", status=400)).code == CODE_PROVIDER


def test_classify_unsupported_provider_by_message():
    exc = _exc("BadRequestError", "LLM Provider NOT provided: doubao is not supported")
    assert classify_exception(exc).code == CODE_UNSUPPORTED_PROVIDER


def test_classify_redacts_secret_in_message():
    err = classify_exception(_exc("AuthenticationError", "Your api key: sk-secret-abcd is invalid"))
    assert "sk-secret-abcd" not in err.message
    assert "***" in err.message


def test_llm_error_carries_user_hint():
    err = classify_exception(_exc("AuthenticationError", "denied"))
    assert "建议" in err.user_message


# ==================== chat：超时/重试注入 ====================


class _FakeCompletion:
    """替身 litellm.acompletion：记录 kwargs，按脚本返回或抛错。"""

    def __init__(self, script: list[Any]):
        self.script = list(script)
        self.calls: list[dict] = []

    async def __call__(self, **kwargs):
        self.calls.append(kwargs)
        action = self.script.pop(0)
        if isinstance(action, Exception):
            raise action
        return action


def _fake_response(text: str = "优化结果", total_tokens: int = 42, finish_reason: str = "stop"):
    usage = {"total_tokens": total_tokens, "prompt_tokens": 30, "completion_tokens": 12}
    # 注意：类体内不能写 finish_reason = finish_reason——类体作用域看不到同名函数参数
    reason = finish_reason

    class _Usage:
        pass

    u = _Usage()
    for k, v in usage.items():
        setattr(u, k, v)

    class _Msg:
        content = text

    class _Choice:
        message = _Msg()
        finish_reason = reason

    class _Resp:
        choices = [_Choice()]
        usage = u

    return _Resp()


def test_chat_injects_timeout_and_retries(monkeypatch):
    fake = _FakeCompletion([_fake_response()])
    monkeypatch.setattr("litellm.acompletion", fake)

    result = asyncio.run(
        LiteLLMGateway().chat(
            model_string="deepseek/deepseek-flash",
            messages=[{"role": "user", "content": "hi"}],
            api_key="sk-secret-1234",
            api_base="https://api.deepseek.com",
            temperature=0.2,
            max_tokens=1024,
            timeout_s=45,
            retries=3,
        )
    )

    call = fake.calls[0]
    assert call["timeout"] == 45.0, "超时必须显式注入，不能依赖 SDK 默认 600s"
    assert call["num_retries"] == 3
    assert call["model"] == "deepseek/deepseek-flash"
    assert result.text == "优化结果"
    assert result.total_tokens == 42
    assert result.finish_reason == "stop"
    assert result.truncated is False


def test_chat_exposes_truncation_when_finish_reason_is_length(monkeypatch):
    """输出达到 max_tokens 上限（finish_reason=length）必须可见——
    思考模式模型的思考 token 也计入 max_tokens，静默截断会让优化结果不完整。"""
    fake = _FakeCompletion([_fake_response(finish_reason="length")])
    monkeypatch.setattr("litellm.acompletion", fake)

    result = asyncio.run(
        LiteLLMGateway().chat(
            model_string="deepseek/deepseek-flash",
            messages=[{"role": "user", "content": "hi"}],
            max_tokens=10,
        )
    )

    assert result.finish_reason == "length"
    assert result.truncated is True


def test_chat_failure_raises_redacted_llm_error(monkeypatch):
    fake = _FakeCompletion([_exc("AuthenticationError", "Your api key: sk-secret-1234 is invalid")])
    monkeypatch.setattr("litellm.acompletion", fake)

    with pytest.raises(LLMError) as excinfo:
        asyncio.run(
            LiteLLMGateway().chat(
                model_string="deepseek/deepseek-flash",
                messages=[{"role": "user", "content": "hi"}],
                api_key="sk-secret-1234",
            )
        )
    assert excinfo.value.code == CODE_AUTH
    assert "sk-secret-1234" not in excinfo.value.message


def test_chat_falls_back_to_openai_compat_for_unregistered_prefix(monkeypatch):
    """doubao 前缀未收录时：第一次报 Unsupported Provider → 回退 openai/ + 方舟端点。"""
    spec = get_provider_spec("doubao")
    assert spec is not None and spec.openai_compat_base, "注册表必须为 doubao 提供 openai 兼容兜底"

    fake = _FakeCompletion([
        _exc("BadRequestError", "Unsupported Provider for model doubao/doubao-pro"),
        _fake_response(text="fallback ok"),
    ])
    monkeypatch.setattr("litellm.acompletion", fake)

    result = asyncio.run(
        LiteLLMGateway().chat(
            model_string="doubao/doubao-pro",
            messages=[{"role": "user", "content": "hi"}],
            api_key="ark-key",
        )
    )

    assert result.text == "fallback ok"
    retry_call = fake.calls[1]
    assert retry_call["model"] == "openai/doubao-pro"
    assert retry_call["api_base"] == spec.openai_compat_base


def test_chat_without_fallback_spec_raises_unsupported(monkeypatch):
    """没有兼容兜底的前缀：Unsupported Provider 直接抛出，不重试。"""
    fake = _FakeCompletion([_exc("BadRequestError", "Unsupported Provider for model foo/bar")])
    monkeypatch.setattr("litellm.acompletion", fake)

    with pytest.raises(LLMError) as excinfo:
        asyncio.run(
            LiteLLMGateway().chat(
                model_string="foo/bar",
                messages=[{"role": "user", "content": "hi"}],
            )
        )
    assert excinfo.value.code == CODE_UNSUPPORTED_PROVIDER
    assert len(fake.calls) == 1


def test_chat_wraps_full_model_string_for_custom_gateway(monkeypatch):
    """2026-10-22 实测踩坑回归：自定义网关的模型 ID 自带斜杠（group/xxx）时，
    LiteLLM 把首段误当服务商；配置了 api_base 就必须整串包进 openai/ 重试一次。"""
    fake = _FakeCompletion([
        _exc("BadRequestError", "LLM Provider NOT provided. You passed model=group/deepseek-v4.1-flash"),
        _fake_response(text="custom ok"),
    ])
    monkeypatch.setattr("litellm.acompletion", fake)

    result = asyncio.run(
        LiteLLMGateway().chat(
            model_string="group/deepseek-v4.1-flash",
            messages=[{"role": "user", "content": "hi"}],
            api_key="local-key",
            api_base="http://127.0.0.1:9999/v1",
        )
    )

    assert result.text == "custom ok"
    retry_call = fake.calls[1]
    assert retry_call["model"] == "openai/group/deepseek-v4.1-flash", "必须整串包 openai/，只剥调用层前缀"
    assert retry_call["api_base"] == "http://127.0.0.1:9999/v1"


# ==================== 验证类 ====================


def test_verify_model_success_and_failure(monkeypatch):
    gateway = LiteLLMGateway()

    ok_fake = _FakeCompletion([_fake_response(text="pong")])
    monkeypatch.setattr("litellm.acompletion", ok_fake)
    ok = asyncio.run(gateway.verify_model(model_string="deepseek/deepseek-flash", api_key="k"))
    assert ok.ok and ok.error_code is None

    fake = _FakeCompletion([_exc("NotFoundError", "model not found", status=404)])
    monkeypatch.setattr("litellm.acompletion", fake)
    bad = asyncio.run(gateway.verify_model(model_string="deepseek/nope", api_key="k"))
    assert not bad.ok
    assert bad.error_code == CODE_MODEL_NOT_FOUND
    assert bad.message  # 面向用户的中文建议存在


def test_verify_profile_tolerates_model_not_found(monkeypatch):
    """档案级探测用建议模型名：模型不存在不代表凭据有问题。"""
    from app.llm.gateway import VerifyOutcome

    gateway = LiteLLMGateway()

    async def fake_verify_model(**kwargs):
        return VerifyOutcome(ok=False, error_code=CODE_MODEL_NOT_FOUND, message="模型不存在", latency_ms=1)

    async def fake_list_models(**kwargs):
        return ["m1"], None

    monkeypatch.setattr(gateway, "verify_model", fake_verify_model)
    monkeypatch.setattr(gateway, "list_remote_models", fake_list_models)

    spec = get_provider_spec("deepseek")
    outcome = asyncio.run(gateway.verify_profile(spec=spec, api_key="k"))
    assert outcome.ok
    assert outcome.remote_models == ["m1"]


# ==================== 模型列表拉取 ====================


class _FakeResp:
    def __init__(self, payload):
        self._payload = payload

    def raise_for_status(self):
        return None

    def json(self):
        return self._payload


class _FakeAsyncClient:
    def __init__(self, payload):
        self._payload = payload
        self.requested_url = None
        self.requested_headers = None

    async def __aenter__(self):
        return self

    async def __aexit__(self, *args):
        return False

    async def get(self, url, headers=None):
        self.requested_url = url
        self.requested_headers = headers or {}
        return _FakeResp(self._payload)


def test_list_remote_models_parses_and_sorts(monkeypatch):
    client = _FakeAsyncClient({"data": [{"id": "deepseek-v4-pro"}, {"id": "deepseek-flash"}, {"id": "deepseek-flash"}]})
    monkeypatch.setattr(gateway_module.httpx, "AsyncClient", lambda timeout: client)

    models, error = asyncio.run(
        gateway_module.LiteLLMGateway().list_remote_models(api_key="sk-k", api_base="https://api.deepseek.com")
    )

    assert error is None
    assert models == ["deepseek-flash", "deepseek-v4-pro"]
    assert client.requested_url == "https://api.deepseek.com/models"
    assert client.requested_headers["Authorization"] == "Bearer sk-k"


def test_list_remote_models_failure_returns_redacted_reason(monkeypatch):
    class _BrokenClient:
        def __init__(self, **kwargs):
            pass

        async def __aenter__(self):
            raise _exc("AuthenticationError", "Your api key: sk-secret-1234 invalid", status=401)

        async def __aexit__(self, *args):
            return False

    monkeypatch.setattr(gateway_module.httpx, "AsyncClient", lambda timeout: _BrokenClient())

    models, error = asyncio.run(
        gateway_module.LiteLLMGateway().list_remote_models(api_key="sk-secret-1234", api_base="https://x")
    )
    assert models is None
    assert error and "sk-secret-1234" not in error


def test_is_valid_model_string():
    assert is_valid_model_string("deepseek/deepseek-flash")
    assert is_valid_model_string("qwen3:14b")
    assert not is_valid_model_string("")
    assert not is_valid_model_string("bad model name with spaces")
