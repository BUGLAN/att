"""
Clipboard API routes.
"""
from typing import List, Optional

from fastapi import APIRouter, HTTPException, Query
from fastapi.responses import FileResponse
from pydantic import BaseModel, Field

from app.clipboard.manager import ClipboardManager
from app.clipboard.models import ClipboardItemType


router = APIRouter(prefix="/api/clipboard", tags=["clipboard"])
clipboard_manager: Optional[ClipboardManager] = None


class ClipboardCreateRequest(BaseModel):
    """Request model for creating a clipboard item."""
    content: str = Field(min_length=1)
    source: str = "manual"
    item_type: Optional[ClipboardItemType] = None
    title: Optional[str] = None
    summary: Optional[str] = None
    metadata: Optional[dict] = None


class ClipboardUpdateRequest(BaseModel):
    """Request model for updating a clipboard item."""
    content: Optional[str] = None
    title: Optional[str] = None
    summary: Optional[str] = None
    is_favorite: Optional[bool] = None
    is_pinned: Optional[bool] = None


def init_clipboard_manager(manager: ClipboardManager) -> None:
    """Initialize shared clipboard manager."""
    global clipboard_manager
    clipboard_manager = manager


@router.get("/items", response_model=List[dict])
async def list_clipboard_items(
    query: Optional[str] = None,
    item_type: Optional[str] = Query(default=None, pattern="^(text|link|code|file|image)?$"),
    favorite_only: bool = False,
    pinned_only: bool = False,
    limit: int = Query(default=100, ge=1, le=500)
) -> List[dict]:
    """List clipboard items with optional filters."""
    resolved_type = ClipboardItemType(item_type) if item_type else None
    items = await clipboard_manager.list_items(
        query=query,
        item_type=resolved_type,
        favorite_only=favorite_only,
        pinned_only=pinned_only,
        limit=limit
    )
    return [item.to_dict() for item in items]


@router.get("/stats")
async def get_clipboard_stats() -> dict:
    """Get clipboard statistics."""
    return await clipboard_manager.get_stats()


@router.post("/items", response_model=dict)
async def create_clipboard_item(request: ClipboardCreateRequest) -> dict:
    """Create a clipboard item."""
    item = await clipboard_manager.capture_item(
        content=request.content,
        source=request.source,
        item_type=request.item_type,
        title=request.title,
        summary=request.summary,
        metadata=request.metadata
    )
    return item.to_dict()


@router.get("/items/{item_id}", response_model=dict)
async def get_clipboard_item(item_id: str) -> dict:
    """Get a clipboard item by ID."""
    item = await clipboard_manager.get_item(item_id)
    if not item:
        raise HTTPException(status_code=404, detail="Clipboard item not found")
    return item.to_dict()


@router.get("/items/{item_id}/image")
async def get_clipboard_item_image(item_id: str):
    """Get clipboard image content by item ID."""
    image_path = await clipboard_manager.get_item_image_path(item_id)
    if not image_path:
        raise HTTPException(status_code=404, detail="Clipboard image not found")

    # 纵深防御：即使上层解析逻辑被绕过，这里也不允许把 data/clipboard/images/ 之外的文件发出去
    if not clipboard_manager.is_allowed_image_path(image_path):
        raise HTTPException(status_code=404, detail="Clipboard image not found")

    return FileResponse(str(image_path))


@router.put("/items/{item_id}", response_model=dict)
async def update_clipboard_item(item_id: str, request: ClipboardUpdateRequest) -> dict:
    """Update a clipboard item."""
    item = await clipboard_manager.update_item(
        item_id,
        content=request.content,
        title=request.title,
        summary=request.summary,
        is_favorite=request.is_favorite,
        is_pinned=request.is_pinned
    )
    if not item:
        raise HTTPException(status_code=404, detail="Clipboard item not found")
    return item.to_dict()


@router.delete("/items/{item_id}")
async def delete_clipboard_item(item_id: str) -> dict:
    """Delete a clipboard item."""
    success = await clipboard_manager.delete_item(item_id)
    if not success:
        raise HTTPException(status_code=404, detail="Clipboard item not found")
    return {"status": "deleted", "item_id": item_id}
