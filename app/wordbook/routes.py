"""生词本 API 路由。"""
from typing import Optional

from fastapi import APIRouter, HTTPException, Query
from pydantic import BaseModel, Field

from app.wordbook.manager import WordbookManager
from app.wordbook.models import WordbookEntryType


router = APIRouter(prefix="/api/wordbook", tags=["wordbook"])
wordbook_manager: Optional[WordbookManager] = None


class WordbookCreateRequest(BaseModel):
    """创建生词条目的请求模型。"""

    source_text: str = Field(min_length=1)
    normalized_text: str = Field(min_length=1)
    language: str = "unknown"
    entry_type: WordbookEntryType = WordbookEntryType.WORD
    translation: str = Field(min_length=1)
    explanations: list[str] = Field(default_factory=list)
    examples: list[str] = Field(default_factory=list)
    source_app: str = ""
    source_context: str = ""
    metadata: Optional[dict] = None


class WordbookUpdateRequest(BaseModel):
    """更新生词条目的请求模型。"""

    translation: Optional[str] = None
    explanations: Optional[list[str]] = None
    examples: Optional[list[str]] = None
    is_favorite: Optional[bool] = None
    metadata: Optional[dict] = None


def init_wordbook_manager(manager: WordbookManager) -> None:
    """初始化共享生词本管理器。"""

    global wordbook_manager
    wordbook_manager = manager


@router.get("", response_model=list[dict])
async def list_wordbook_entries(
    query: Optional[str] = None,
    entry_type: Optional[str] = Query(default=None, pattern="^(word|phrase|sentence)?$"),
    favorite_only: bool = False,
    limit: int = Query(default=100, ge=1, le=500),
) -> list[dict]:
    """查询生词本条目。"""

    resolved_type = WordbookEntryType(entry_type) if entry_type else None
    items = await wordbook_manager.list_entries(
        query=query,
        entry_type=resolved_type,
        favorite_only=favorite_only,
        limit=limit,
    )
    return [item.to_dict() for item in items]


@router.post("", response_model=dict)
async def create_wordbook_entry(request: WordbookCreateRequest) -> dict:
    """创建或追加生词条目。"""

    item = await wordbook_manager.capture_entry(
        source_text=request.source_text,
        normalized_text=request.normalized_text,
        language=request.language,
        entry_type=request.entry_type,
        translation=request.translation,
        explanations=request.explanations,
        examples=request.examples,
        source_app=request.source_app,
        source_context=request.source_context,
        metadata=request.metadata,
    )
    return item.to_dict()


@router.get("/stats")
async def get_wordbook_stats() -> dict:
    """返回生词本统计。"""

    return await wordbook_manager.get_stats()


@router.get("/{entry_id}", response_model=dict)
async def get_wordbook_entry(entry_id: str) -> dict:
    """读取单条词条。"""

    item = await wordbook_manager.get_entry(entry_id)
    if not item:
        raise HTTPException(status_code=404, detail="Wordbook entry not found")
    return item.to_dict()


@router.put("/{entry_id}", response_model=dict)
async def update_wordbook_entry(entry_id: str, request: WordbookUpdateRequest) -> dict:
    """更新词条内容。"""

    item = await wordbook_manager.update_entry(
        entry_id,
        translation=request.translation,
        explanations=request.explanations,
        examples=request.examples,
        is_favorite=request.is_favorite,
        metadata=request.metadata,
    )
    if not item:
        raise HTTPException(status_code=404, detail="Wordbook entry not found")
    return item.to_dict()
