"""AI 动作 API 路由。"""
from typing import Optional

from fastapi import APIRouter
from pydantic import BaseModel, Field

from app.ai_actions.models import AIActionType
from app.ai_actions.service import AIActionService


router = APIRouter(prefix="/api/ai-actions", tags=["ai-actions"])
ai_action_service: Optional[AIActionService] = None


class AIActionRequest(BaseModel):
    """AI 动作请求。"""

    source_text: str = Field(min_length=1)
    source_app: str = ""
    source_context: str = ""
    auto_add_to_wordbook: bool = True
    metadata: Optional[dict] = None


def init_ai_action_service(service: AIActionService) -> None:
    """初始化共享服务。"""

    global ai_action_service
    ai_action_service = service


async def _run(action_type: AIActionType, request: AIActionRequest) -> dict:
    return await ai_action_service.run_action_dict(
        action_type=action_type,
        source_text=request.source_text,
        source_app=request.source_app,
        source_context=request.source_context,
        auto_add_to_wordbook=request.auto_add_to_wordbook,
        metadata=request.metadata,
    )


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
