"""LLM 优化「静默兜底」回归测试。

背景（真实踩坑）：用户配置的 LLM 调用失败时，``TextOptimizer.optimize`` 会
回退到 ``.env`` 里的内置 DeepSeek key 继续完成优化。降级本身是合理的容错，
但原来的实现只打一条 warning 日志，调用方完全不知情——于是「设置页里的 key
配错了」被静默掩盖成「优化成功」，用户以为 key 生效了。

锁死的点：
1. 网关调用失败时仍完成优化，但 ``OptimizationResult.fallback_reason`` 必须有值；
2. 回填的降级原因里密钥必须已脱敏（AGENTS.md §3.4）；
3. 网关成功时 ``fallback_reason`` 必须为空，且不得触发 legacy 请求。
"""
import asyncio

from app.llm import gateway as gateway_module
from app.llm import optimizer as optimizer_module
from app.llm.optimizer import TextOptimizer


class _FakeResponse:
    def __init__(self, text: str):
        self._text = text

    def raise_for_status(self) -> None:
        return None

    def json(self) -> dict:
        return {
            "choices": [{"message": {"content": self._text}}],
            "usage": {"total_tokens": 12},
        }


class _FakeHttpClient:
    """替代 httpx.AsyncClient，记录 legacy 请求并返回固定补全结果。"""

    def __init__(self, text: str = "内置 key 优化结果"):
        self.text = text
        self.calls = []

    async def post(self, url, json=None):
        self.calls.append({"url": url, "json": json})
        return _FakeResponse(self.text)


class _FailingProvider:
    def __init__(self, error: Exception):
        self._error = error

    async def optimize(self, text: str, prompt: str) -> str:
        raise self._error


class _OkProvider:
    async def optimize(self, text: str, prompt: str) -> str:
        return "网关优化结果"


class _FakeGateway:
    def __init__(self, provider):
        self._provider = provider

    def get_provider(self, config_id: str):
        return self._provider


def _make_optimizer(monkeypatch, provider, api_key: str = "sk-env-builtin-key"):
    optimizer = TextOptimizer(api_key=api_key)
    fake_client = _FakeHttpClient()
    monkeypatch.setattr(optimizer, "_client", fake_client)
    monkeypatch.setattr(optimizer_module, "_get_default_llm_config_id", lambda: "cfg-1")
    monkeypatch.setattr(gateway_module, "get_gateway", lambda: _FakeGateway(provider))
    return optimizer, fake_client


def test_gateway_failure_falls_back_and_reports_redacted_reason(monkeypatch):
    error = RuntimeError(
        "Error code: 401 - {'message': 'Authentication Fails, Your api key: 123 is invalid'}"
    )
    optimizer, client = _make_optimizer(monkeypatch, _FailingProvider(error))

    result = asyncio.run(optimizer.optimize("原始文本"))

    assert result.optimized_text == "内置 key 优化结果"
    assert len(client.calls) == 1, "降级后必须落到内置 key 的 legacy 请求"
    assert result.fallback_reason, "降级原因必须回填，否则调用方无法提示用户"
    assert "123" not in result.fallback_reason, "降级原因里的密钥必须脱敏"
    assert "***" in result.fallback_reason


def test_gateway_success_has_no_fallback_reason(monkeypatch):
    optimizer, client = _make_optimizer(monkeypatch, _OkProvider())

    result = asyncio.run(optimizer.optimize("原始文本"))

    assert result.optimized_text == "网关优化结果"
    assert result.fallback_reason is None
    assert client.calls == [], "网关成功时不应再走 legacy 请求"


def test_missing_provider_also_reports_degradation(monkeypatch):
    """配置存在但 provider 建不起来（已删除/解密失败/不支持）时同样是「配置没生效」，
    也必须回传降级原因，不能静默换用内置 key。"""
    class _NoneGateway:
        def get_provider(self, config_id):
            return None

    optimizer = TextOptimizer(api_key="sk-env-builtin-key")
    fake_client = _FakeHttpClient()
    monkeypatch.setattr(optimizer, "_client", fake_client)
    monkeypatch.setattr(optimizer_module, "_get_default_llm_config_id", lambda: "cfg-missing")
    monkeypatch.setattr(gateway_module, "get_gateway", lambda: _NoneGateway())

    result = asyncio.run(optimizer.optimize("原始文本"))

    assert result.optimized_text == "内置 key 优化结果"
    assert result.fallback_reason, "provider 缺失也必须上报降级"
    assert "cfg-missing" in result.fallback_reason
    assert len(fake_client.calls) == 1
