"""流式 Gateway / Optimizer 回归测试。

锁死的点：
- chat_stream 逐块解析 content 与 usage，并显式注入 timeout/num_retries/stream；
- optimize_stream 产出 delta 事件，最后给出带用量与 finish_reason 的 done。
"""
import asyncio
from types import SimpleNamespace

from app.llm.gateway import GatewayStreamChunk, LiteLLMGateway
from app.llm.models import ModelEntry, ModelParams, ProviderProfile
from app.llm.optimizer import TextOptimizer


class _FakeStream:
    def __init__(self, chunks):
        self._chunks = list(chunks)

    def __aiter__(self):
        return self

    async def __anext__(self):
        if not self._chunks:
            raise StopAsyncIteration
        return self._chunks.pop(0)


class _FakeCompletion:
    def __init__(self, chunks):
        self.chunks = chunks
        self.calls = []

    async def __call__(self, **kwargs):
        self.calls.append(kwargs)
        return _FakeStream(self.chunks)


def _chunk(text="", finish_reason=None, usage=None):
    delta = SimpleNamespace(content=text)
    choice = SimpleNamespace(delta=delta, finish_reason=finish_reason)
    return SimpleNamespace(choices=[choice], usage=usage)


def test_gateway_chat_stream_parses_deltas_and_usage(monkeypatch):
    fake = _FakeCompletion([
        _chunk("优化"),
        _chunk("后的文本"),
        _chunk(
            "",
            finish_reason="stop",
            usage=SimpleNamespace(
                prompt_tokens=100,
                completion_tokens=20,
                total_tokens=120,
                completion_tokens_details=SimpleNamespace(reasoning_tokens=7),
            ),
        ),
    ])
    monkeypatch.setattr("litellm.acompletion", fake)

    async def collect():
        return [
            chunk
            async for chunk in LiteLLMGateway().chat_stream(
                model_string="deepseek/deepseek-flash",
                messages=[{"role": "user", "content": "hi"}],
                api_key="sk-secret-1234",
                api_base="https://api.deepseek.com",
                temperature=0.2,
                max_tokens=1024,
                timeout_s=45,
                retries=3,
            )
        ]

    chunks = asyncio.run(collect())
    assert "".join(chunk.text for chunk in chunks) == "优化后的文本"
    assert chunks[-1].finish_reason == "stop"
    assert chunks[-1].total_tokens == 120
    assert chunks[-1].reasoning_tokens == 7

    call = fake.calls[0]
    assert call["stream"] is True
    assert call["timeout"] == 45.0
    assert call["num_retries"] == 3
    assert call["max_tokens"] == 1024


class _FakeStore:
    def __init__(self):
        self.entry = ModelEntry(
            id="mdl-1",
            profile_id="prof-1",
            model="deepseek/deepseek-flash",
            params=ModelParams(temperature=0.2, max_tokens=2048, timeout_s=33, retries=1),
        )
        self.profile = ProviderProfile(
            id="prof-1",
            provider="deepseek",
            name="DeepSeek",
            api_base="https://api.deepseek.com",
        )

    def resolve_runtime(self, model_id=None):
        return self.entry, self.profile, "sk-secret-1234"


class _FakeStreamingGateway:
    def __init__(self):
        self.kwargs = None

    async def chat_stream(self, **kwargs):
        self.kwargs = kwargs
        yield GatewayStreamChunk(text="优化", model_string="deepseek/deepseek-flash")
        yield GatewayStreamChunk(text="后的文本", model_string="deepseek/deepseek-flash")
        yield GatewayStreamChunk(
            text="",
            model_string="deepseek/deepseek-flash",
            finish_reason="stop",
            prompt_tokens=100,
            completion_tokens=20,
            total_tokens=120,
        )


def test_optimizer_stream_emits_deltas_and_done(monkeypatch):
    import app.llm.store as store_module
    import app.llm.gateway as gateway_module

    store = _FakeStore()
    gateway = _FakeStreamingGateway()
    monkeypatch.setattr(store_module, "get_llm_store", lambda: store)
    monkeypatch.setattr(gateway_module, "get_llm_gateway", lambda: gateway)

    async def collect():
        return [
            event
            async for event in TextOptimizer().optimize_stream("原始文本", preset_id=None)
        ]

    events = asyncio.run(collect())
    assert [event.type for event in events] == ["delta", "delta", "done"]
    assert events[0].text == "优化"
    assert events[1].text == "后的文本"
    result = events[-1].result
    assert result.optimized_text == "优化后的文本"
    assert result.tokens_used == 120
    assert result.truncated is False
    assert gateway.kwargs["max_tokens"] == 2048
    assert gateway.kwargs["timeout_s"] == 33
    assert gateway.kwargs["retries"] == 1


def test_optimizer_stream_passes_reasoning_effort(monkeypatch):
    """模型条目里的思考强度必须一路传到网关（否则 UI 上的开关是装饰品）。"""
    import app.llm.store as store_module
    import app.llm.gateway as gateway_module

    store = _FakeStore()
    store.entry.params.reasoning_effort = "none"
    gateway = _FakeStreamingGateway()
    monkeypatch.setattr(store_module, "get_llm_store", lambda: store)
    monkeypatch.setattr(gateway_module, "get_llm_gateway", lambda: gateway)

    async def collect():
        return [event async for event in TextOptimizer().optimize_stream("原始文本")]

    asyncio.run(collect())
    assert gateway.kwargs["reasoning_effort"] == "none"


def test_optimizer_stream_raises_non_retryable_on_empty_truncated_output(monkeypatch):
    """思考 token 吃满 max_tokens → 正文为空 + finish_reason=length。

    这是确定性失败：重跑同一请求只会再烧一次思考 token（实测 22s → 41s），
    所以必须直接抛不可重试错误，让调用方跳过「降级非流式」。
    """
    import app.llm.store as store_module
    import app.llm.gateway as gateway_module
    from app.llm.errors import CODE_OUTPUT_TRUNCATED, LLMError

    class _ThinkingGateway:
        async def chat_stream(self, **kwargs):
            yield GatewayStreamChunk(
                text="",
                model_string="deepseek/deepseek-flash",
                finish_reason="length",
                prompt_tokens=504,
                completion_tokens=4096,
                total_tokens=4600,
                reasoning_tokens=4096,
            )

    monkeypatch.setattr(store_module, "get_llm_store", lambda: _FakeStore())
    monkeypatch.setattr(gateway_module, "get_llm_gateway", lambda: _ThinkingGateway())

    async def collect():
        return [event async for event in TextOptimizer().optimize_stream("原始文本")]

    try:
        asyncio.run(collect())
    except LLMError as exc:
        assert exc.code == CODE_OUTPUT_TRUNCATED
        assert exc.retryable is False
        assert "思考" in exc.user_message
    else:
        raise AssertionError("空正文 + length 必须抛 LLMError，不能静默返回空结果")
