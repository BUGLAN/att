"""Agent 对话端点 `POST /api/ai-actions/chat` 的行为测试。

覆盖：正常返回、多轮上下文拼接、LLM 未配置、LLM 抛错转 502、请求体错误。
全部使用假 gateway / 假 provider / 假 service，**禁止真调外部 LLM API**。
"""
import json

import pytest
from fastapi import FastAPI
from fastapi.testclient import TestClient

from app.ai_actions import routes as ai_action_routes
from app.ai_actions.routes import LLM_NOT_CONFIGURED_MESSAGE
from app.ai_actions.routes import router as ai_action_router
from app.ai_actions.service import AIActionService, LLMNotConfiguredError


class StubProvider:
    """假 LLM provider：只记录收到的 prompt，不发起任何网络请求。"""

    def __init__(self, output: str = '{"answer": "好的"}', error: Exception | None = None):
        self.output = output
        self.error = error
        self.calls: list[dict] = []

    async def optimize(self, text: str, prompt_template: str) -> str:
        self.calls.append({"text": text, "prompt_template": prompt_template})
        if self.error is not None:
            raise self.error
        return self.output


class StubGateway:
    """假 LLM gateway：get_default_provider 返回预设 provider（可为 None 表示未配置）。"""

    def __init__(self, provider=None):
        self.provider = provider

    def get_default_provider(self):
        return self.provider


class StubService:
    """假 AIActionService：可让 chat 抛出任意异常，验证路由层的错误映射。"""

    def __init__(self, error: Exception | None = None):
        self.error = error
        self.calls: list[list] = []

    async def chat(self, messages, metadata=None) -> dict:
        self.calls.append(list(messages))
        if self.error is not None:
            raise self.error
        return {"action_type": "chat", "answer": "stub", "messages": [], "metadata": metadata or {}}


def build_client(monkeypatch, service) -> TestClient:
    """构造只挂载 ai-actions 路由的测试应用（不触碰 app.main 的 lifespan）。"""

    # monkeypatch 会在用例结束后自动还原模块级单例，避免污染其他测试
    monkeypatch.setattr(ai_action_routes, "ai_action_service", service)
    test_app = FastAPI()
    test_app.include_router(ai_action_router)
    return TestClient(test_app)


def build_real_service(provider) -> AIActionService:
    """用真实 AIActionService + 假 gateway 组装服务（chat 不涉及生词本，故传 None）。"""

    return AIActionService(llm_gateway=StubGateway(provider), wordbook_manager=None)


def test_chat_returns_answer_and_echoes_context(monkeypatch):
    """正常返回：解析模型 JSON 输出为 answer，并回显上下文。"""
    provider = StubProvider(output='{"answer": "你好，我是 Agent。", "explanations": ["来自假模型"]}')
    client = build_client(monkeypatch, build_real_service(provider))

    response = client.post("/api/ai-actions/chat", json={
        "messages": [{"role": "user", "content": "你好"}]
    })

    assert response.status_code == 200
    payload = response.json()
    assert payload["action_type"] == "chat"
    assert payload["answer"] == "你好，我是 Agent。"
    assert payload["explanations"] == ["来自假模型"]
    assert payload["messages"] == [{"role": "user", "content": "你好"}]
    assert payload["source_text"] == "用户：你好"
    # 复用的是既有 CHAT prompt 模板
    assert "桌面端 AI 划词助手" in provider.calls[0]["prompt_template"]


def test_chat_joins_multi_turn_context_in_order(monkeypatch):
    """多轮上下文：历史 user/assistant 与最新 user 按原顺序拼接进同一次 prompt。"""
    provider = StubProvider(output="第二答")
    client = build_client(monkeypatch, build_real_service(provider))

    response = client.post("/api/ai-actions/chat", json={
        "messages": [
            {"role": "user", "content": "第一问"},
            {"role": "assistant", "content": "第一答"},
            {"role": "user", "content": "第二问"},
        ]
    })

    assert response.status_code == 200
    # 纯文本输出也会被归一成 answer
    assert response.json()["answer"] == "第二答"

    transcript = provider.calls[0]["text"]
    assert transcript == "用户：第一问\n助手：第一答\n用户：第二问"
    assert transcript.index("第一问") < transcript.index("第一答") < transcript.index("第二问")
    assert response.json()["messages"] == [
        {"role": "user", "content": "第一问"},
        {"role": "assistant", "content": "第一答"},
        {"role": "user", "content": "第二问"},
    ]


def test_chat_without_llm_config_returns_readable_502(monkeypatch):
    """LLM 未配置：502 + 可读中文 detail，而不是 500 裸抛。"""
    client = build_client(monkeypatch, build_real_service(None))

    response = client.post("/api/ai-actions/chat", json={
        "messages": [{"role": "user", "content": "你好"}]
    })

    assert response.status_code == 502
    assert response.json()["detail"] == LLM_NOT_CONFIGURED_MESSAGE
    assert "LLM 配置" in response.json()["detail"]


def test_chat_llm_failure_is_converted_to_502(monkeypatch):
    """LLM 调用抛错：转换成 502 + 中文 detail（保留原始错误信息）。"""
    provider = StubProvider(error=RuntimeError("上游连接超时"))
    client = build_client(monkeypatch, build_real_service(provider))

    response = client.post("/api/ai-actions/chat", json={
        "messages": [{"role": "user", "content": "你好"}]
    })

    assert response.status_code == 502
    detail = response.json()["detail"]
    assert detail.startswith("AI 对话调用失败：")
    assert "上游连接超时" in detail


def test_chat_llm_not_configured_error_from_service_is_502(monkeypatch):
    """假 service 抛出 LLMNotConfiguredError 时同样映射为 502 中文提示。"""
    client = build_client(monkeypatch, StubService(error=LLMNotConfiguredError("未配置默认 LLM provider")))

    response = client.post("/api/ai-actions/chat", json={
        "messages": [{"role": "user", "content": "你好"}]
    })

    assert response.status_code == 502
    assert response.json()["detail"] == LLM_NOT_CONFIGURED_MESSAGE


@pytest.mark.parametrize("messages", [
    [],
    [{"role": "system", "content": "你不该出现在这里"}],
    [{"role": "user", "content": "   "}],
])
def test_chat_invalid_messages_return_400(monkeypatch, messages):
    """请求体语义错误：400 + 可读中文 detail。"""
    client = build_client(monkeypatch, build_real_service(StubProvider()))

    response = client.post("/api/ai-actions/chat", json={"messages": messages})

    assert response.status_code == 400
    assert response.json()["detail"]


def test_chat_service_not_initialized_returns_503(monkeypatch):
    """服务尚未初始化：503 明确提示，而不是 AttributeError 500。"""
    client = build_client(monkeypatch, None)

    response = client.post("/api/ai-actions/chat", json={
        "messages": [{"role": "user", "content": "你好"}]
    })

    assert response.status_code == 503
    assert "尚未初始化" in response.json()["detail"]


def test_chat_prompt_reuses_action_prompt_template(monkeypatch):
    """拼接文本必须作为 {text} 传入既有 CHAT prompt 模板（复用而非另起一套 provider 体系）。"""
    provider = StubProvider(output=json.dumps({"answer": "ok"}, ensure_ascii=False))
    client = build_client(monkeypatch, build_real_service(provider))

    client.post("/api/ai-actions/chat", json={
        "messages": [
            {"role": "assistant", "content": "前情"},
            {"role": "user", "content": "追问"},
        ]
    })

    call = provider.calls[0]
    assert "{text}" in call["prompt_template"]
    assert call["text"] == "助手：前情\n用户：追问"
