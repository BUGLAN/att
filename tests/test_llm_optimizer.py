"""TextOptimizer 回归测试（v2：LiteLLM 网关 + LLMStore）。

v1 时代这里锁的是「静默降级要上报 fallback_reason」；v2 移除了 .env 内置 key 兜底
（docs/LLM配置模块重设计.md §9.2），锁死的点反转为：

1. 网关失败时必须**抛出 LLMError**，绝不静默换成别的 key 再试（失败可见）；
2. 成功结果带模型串与真实 token 用量；
3. 未配置任何模型时抛 model_not_found，文案可指导用户去设置页；
4. 调用参数（temperature/max_tokens/timeout/retries）来自模型条目配置。
"""
import asyncio
from types import SimpleNamespace

import pytest

from app.llm.errors import CODE_AUTH, CODE_MODEL_NOT_FOUND, LLMError
from app.llm.gateway import GatewayChatResult
from app.llm.models import ModelEntry, ModelParams, ProviderProfile
from app.llm.optimizer import TextOptimizer


class _FakeStore:
    def __init__(self, entry=None, profile=None, key="sk-secret-1234", error: Exception | None = None):
        self.entry = entry or ModelEntry(
            id="mdl-1",
            profile_id="prof-1",
            model="deepseek/deepseek-flash",
            display_name="DeepSeek Flash",
            params=ModelParams(temperature=0.2, max_tokens=2048, timeout_s=33, retries=1),
        )
        self.profile = profile or ProviderProfile(
            id="prof-1",
            provider="deepseek",
            name="DeepSeek 官方",
            api_base="https://api.deepseek.com",
        )
        self.key = key
        self.error = error

    def resolve_runtime(self, model_id=None):
        if self.error:
            raise self.error
        return self.entry, self.profile, self.key


class _FakeGateway:
    def __init__(self, result=None, error: Exception | None = None):
        self.result = result or GatewayChatResult(
            text="优化后的文本", model_string="deepseek/deepseek-flash",
            latency_ms=800, prompt_tokens=100, completion_tokens=20, total_tokens=120,
        )
        self.error = error
        self.kwargs = None

    async def chat(self, **kwargs):
        self.kwargs = kwargs
        if self.error:
            raise self.error
        return self.result


def _patch(monkeypatch, store, gateway):
    import app.llm.gateway as gateway_module
    import app.llm.store as store_module

    monkeypatch.setattr(store_module, "get_llm_store", lambda: store)
    monkeypatch.setattr(gateway_module, "get_llm_gateway", lambda: gateway)
    # 预设管理器指向仓库内置 prompts/，这里直接锁死默认模板路径，避免环境漂移
    return store, gateway


def test_optimize_success_returns_result_with_usage(monkeypatch):
    gateway = _FakeGateway()
    _patch(monkeypatch, _FakeStore(), gateway)

    result = asyncio.run(TextOptimizer().optimize("原始文本", preset_id=None))

    assert result.optimized_text == "优化后的文本"
    assert result.tokens_used == 120
    assert result.model_string == "deepseek/deepseek-flash"
    assert result.truncated is False
    # 调用参数必须来自模型条目配置，而不是 SDK 默认值
    assert gateway.kwargs["temperature"] == 0.2
    assert gateway.kwargs["max_tokens"] == 2048
    assert gateway.kwargs["timeout_s"] == 33, "超时必须来自模型配置（已知问题 #4）"
    assert gateway.kwargs["retries"] == 1
    assert gateway.kwargs["api_key"] == "sk-secret-1234"
    assert "原始文本" in gateway.kwargs["messages"][0]["content"]


def test_optimize_failure_raises_visible_error_without_fallback(monkeypatch):
    """v1 的静默兜底已删除：网关失败必须直接抛错，绝不偷偷换 key 重试。"""
    error = LLMError(CODE_AUTH, "鉴权失败：***")
    gateway = _FakeGateway(error=error)
    _patch(monkeypatch, _FakeStore(), gateway)

    with pytest.raises(LLMError) as excinfo:
        asyncio.run(TextOptimizer().optimize("原始文本"))

    assert excinfo.value.code == CODE_AUTH
    assert excinfo.value is error


def test_optimize_without_any_model_guides_user(monkeypatch):
    store = _FakeStore(error=LLMError(CODE_MODEL_NOT_FOUND, "尚未配置可用的 LLM 模型，请在设置页添加"))
    _patch(monkeypatch, store, _FakeGateway())

    with pytest.raises(LLMError) as excinfo:
        asyncio.run(TextOptimizer().optimize("原始文本"))

    assert excinfo.value.code == CODE_MODEL_NOT_FOUND
    assert "设置页" in excinfo.value.user_message


def test_optimize_empty_text_short_circuits(monkeypatch):
    gateway = _FakeGateway()
    _patch(monkeypatch, _FakeStore(), gateway)

    result = asyncio.run(TextOptimizer().optimize("   "))

    assert result.optimized_text == "   "
    assert gateway.kwargs is None, "空文本不应发起任何请求"


def test_optimize_uses_named_preset(monkeypatch):
    """传了 preset_id 时必须真的用预设渲染（而不是每次都走默认模板）。"""
    gateway = _FakeGateway()
    _patch(monkeypatch, _FakeStore(), gateway)

    from app.prompts import prompt_manager

    presets = prompt_manager.get_all()
    if not presets:
        pytest.skip("仓库无内置预设，跳过")
    preset = next((p for p in presets if "{text}" in p.prompt_template), presets[0])

    asyncio.run(TextOptimizer().optimize("会议内容", preset_id=preset.id))

    rendered_head = preset.prompt_template.split("{text}")[0][:20]
    assert rendered_head in gateway.kwargs["messages"][0]["content"]
