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


# ==================== 看门狗：区分首 token / 块间空闲 ====================


def _patch_open_stream(monkeypatch, gateway, stream):
    """把网关的「开流」替换成直接返回预制的假流（绕过 litellm）。"""
    import app.llm.gateway as gateway_module

    async def fake_open(**kwargs):
        return stream

    monkeypatch.setattr(gateway, "_open_stream_once", fake_open)
    return gateway_module


class _HangingStream:
    """开流后永远不产出第一条增量。"""

    def __init__(self):
        self.closed = False

    def __aiter__(self):
        return self

    async def __anext__(self):
        await asyncio.sleep(30)
        raise StopAsyncIteration

    async def aclose(self):
        self.closed = True


class _StallAfterFirstStream:
    """产出第一块后卡死（相邻增量之间的空闲超预算）。"""

    def __init__(self):
        self._served = False
        self.closed = False

    def __aiter__(self):
        return self

    async def __anext__(self):
        if not self._served:
            self._served = True
            return _chunk("第一块")
        await asyncio.sleep(30)
        raise StopAsyncIteration

    async def aclose(self):
        self.closed = True


def test_watchdog_first_token_timeout_raises_retryable_timeout(monkeypatch):
    """首 token 预算耗尽 → 可重试的 timeout，且必须关闭底层流。"""
    from app.llm.errors import CODE_TIMEOUT, LLMError

    hanging = _HangingStream()
    gateway = LiteLLMGateway()
    _patch_open_stream(monkeypatch, gateway, hanging)

    async def run():
        async for _chunk_ in gateway.chat_stream(
            model_string="deepseek/deepseek-flash",
            messages=[{"role": "user", "content": "hi"}],
            timeout_s=45,
            first_token_timeout_s=0,
            chunk_idle_timeout_s=45,
        ):
            pass

    try:
        asyncio.run(run())
    except LLMError as exc:
        # 看门狗抛出的分级不能被 chat_stream 的 classify_exception 洗成 unknown
        assert exc.code == CODE_TIMEOUT
        assert exc.retryable is True
        assert "首条增量前" in exc.user_message
    else:
        raise AssertionError("首 token 卡死必须抛 LLMError(timeout)")
    assert hanging.closed is True


def test_watchdog_chunk_idle_timeout_keeps_first_chunk(monkeypatch):
    """相邻增量之间卡死：第一块已经交付，空闲预算耗尽后抛 timeout。"""
    from app.llm.errors import CODE_TIMEOUT, LLMError

    stall = _StallAfterFirstStream()
    gateway = LiteLLMGateway()
    _patch_open_stream(monkeypatch, gateway, stall)

    received: list[str] = []

    async def run():
        async for chunk in gateway.chat_stream(
            model_string="deepseek/deepseek-flash",
            messages=[{"role": "user", "content": "hi"}],
            timeout_s=45,
            first_token_timeout_s=45,
            chunk_idle_timeout_s=0,
        ):
            received.append(chunk.text)

    try:
        asyncio.run(run())
    except LLMError as exc:
        assert exc.code == CODE_TIMEOUT
        assert "相邻增量之间" in exc.user_message
    else:
        raise AssertionError("块间卡死必须抛 LLMError(timeout)")
    assert received == ["第一块"]
    assert stall.closed is True


def test_optimizer_stream_passes_stage_timeouts(monkeypatch):
    """流式路径必须把派生出的阶段预算传给网关（否则看门狗形同虚设）。"""
    import app.llm.store as store_module
    import app.llm.gateway as gateway_module

    store = _FakeStore()
    gateway = _FakeStreamingGateway()
    monkeypatch.setattr(store_module, "get_llm_store", lambda: store)
    monkeypatch.setattr(gateway_module, "get_llm_gateway", lambda: gateway)

    async def collect():
        return [event async for event in TextOptimizer().optimize_stream("字" * 2000)]

    asyncio.run(collect())
    # 配置 33s，2000 字：总预算 = max(33, 20+60) = 80；首 token = min(80, 130) = 80；空闲 = 30
    assert gateway.kwargs["timeout_s"] == 80
    assert gateway.kwargs["first_token_timeout_s"] == 80
    assert gateway.kwargs["chunk_idle_timeout_s"] == 30


def test_optimizer_stream_done_result_is_cleaned(monkeypatch):
    """增量按原文推送，最终 done 的落盘文本必须是清洗后的。"""
    import app.llm.store as store_module
    import app.llm.gateway as gateway_module

    class _NoisyGateway:
        def __init__(self):
            self.kwargs = None

        async def chat_stream(self, **kwargs):
            self.kwargs = kwargs
            yield GatewayStreamChunk(text="以下是整理后的内容：\n", model_string="deepseek/deepseek-flash")
            yield GatewayStreamChunk(text="正文内容。", model_string="deepseek/deepseek-flash")
            yield GatewayStreamChunk(text="", model_string="deepseek/deepseek-flash", finish_reason="stop")

    gateway = _NoisyGateway()
    monkeypatch.setattr(store_module, "get_llm_store", lambda: _FakeStore())
    monkeypatch.setattr(gateway_module, "get_llm_gateway", lambda: gateway)

    async def collect():
        return [event async for event in TextOptimizer().optimize_stream("原始文本")]

    events = asyncio.run(collect())
    deltas = "".join(e.text for e in events if e.type == "delta")
    assert deltas == "以下是整理后的内容：\n正文内容。"
    assert events[-1].type == "done"
    assert events[-1].result.optimized_text == "正文内容。"
