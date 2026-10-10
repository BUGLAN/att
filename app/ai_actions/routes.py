"""AI 动作 API 路由。"""
from typing import List, Optional

from fastapi import APIRouter, HTTPException
from pydantic import BaseModel, Field

from app.ai_actions.models import AIActionType
from app.ai_actions.service import AIActionService, LLMNotConfiguredError
from app.utils.logging import get_logger

logger = get_logger(__name__)


router = APIRouter(prefix="/api/ai-actions", tags=["ai-actions"])
ai_action_service: Optional[AIActionService] = None


# 未配置 LLM 时的统一中文提示（前端 Toast 直接展示）
LLM_NOT_CONFIGURED_MESSAGE = (
    "未配置可用的 LLM 模型：请先到「设置 → LLM 配置」新增一条配置，"
    "填写 API Key 并设为默认后再试。"
)


class AIActionRequest(BaseModel):
    """AI 动作请求。"""

    source_text: str = Field(min_length=1)
    source_app: str = ""
    source_context: str = ""
    auto_add_to_wordbook: bool = True
    metadata: Optional[dict] = None


class ChatMessage(BaseModel):
    """单条对话消息。"""

    role: str
    content: str


class ChatRequest(BaseModel):
    """Agent 对话请求：messages 携带完整多轮上下文，最后一条通常是用户输入。"""

    messages: List[ChatMessage] = Field(default_factory=list)
    metadata: Optional[dict] = None


def init_ai_action_service(service: AIActionService) -> None:
    """初始化共享服务。"""

    global ai_action_service
    ai_action_service = service


def _require_ai_action_service() -> AIActionService:
    """取共享服务，未初始化时给出可读提示而不是 AttributeError。"""

    if ai_action_service is None:
        raise HTTPException(status_code=503, detail="AI 动作服务尚未初始化，请稍后重试")
    return ai_action_service


async def _run(action_type: AIActionType, request: AIActionRequest) -> dict:
    return await ai_action_service.run_action_dict(
        action_type=action_type,
        source_text=request.source_text,
        source_app=request.source_app,
        source_context=request.source_context,
        auto_add_to_wordbook=request.auto_add_to_wordbook,
        metadata=request.metadata,
    )


@router.post("/chat", response_model=dict)
async def chat_with_agent(request: ChatRequest) -> dict:
    """Agent 多轮对话：复用默认 LLM provider，失败统一转成可读中文 502。"""

    service = _require_ai_action_service()

    try:
        return await service.chat(request.messages, metadata=request.metadata)
    except LLMNotConfiguredError as error:
        logger.warning("Agent 对话缺少可用的 LLM 配置", error=str(error))
        raise HTTPException(status_code=502, detail=LLM_NOT_CONFIGURED_MESSAGE) from error
    except ValueError as error:
        # 请求体语义错误（空 messages、非法 role、空 content）：400 而不是 500
        raise HTTPException(status_code=400, detail=str(error)) from error
    except HTTPException:
        raise
    except Exception as error:  # noqa: BLE001 - 上游 LLM 异常统一转 502，避免 500 裸抛
        logger.error("Agent 对话调用失败", error=str(error), exc_info=True)
        raise HTTPException(status_code=502, detail=f"AI 对话调用失败：{error}") from error


@router.post("/translate", response_model=dict)
async def translate_text(request: AIActionRequest) -> dict:
    """运行翻译动作。"""

    return await _run(AIActionType.TRANSLATE, request)


@router.post("/explain", response_model=dict)
async def explain_text(request: AIActionRequest) -> dict:
    """运行解释动作。"""

    return await _run(AIActionType.EXPLAIN, request)


@router.post("/summarize", response_model=dict)
async def summarize_text(request: AIActionRequest) -> dict:
    """运行总结动作。"""

    return await _run(AIActionType.SUMMARIZE, request)


@router.post("/rewrite", response_model=dict)
async def rewrite_text(request: AIActionRequest) -> dict:
    """运行改写动作。"""

    return await _run(AIActionType.REWRITE, request)
