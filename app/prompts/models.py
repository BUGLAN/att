"""
Prompt Preset Models
Data models for AI optimization prompt templates
"""
from pydantic import BaseModel
from typing import Optional, List
from enum import Enum
from datetime import datetime


class PromptCategory(str, Enum):
    """Prompt preset categories."""
    GENERAL = "general"        # 通用
    CODING = "coding"          # 程序员
    NOVEL = "novel"           # 小说
    MEETING = "meeting"        # 会议
    MARKETING = "marketing"    # 文案
    CUSTOM = "custom"          # 自定义


class PromptPreset(BaseModel):
    """Prompt preset configuration."""
    id: str                           # 预设ID
    name: str                         # 预设名称
    description: Optional[str]        # 描述
    category: PromptCategory          # 分类
    prompt_template: str              # 提示词模板，含 {text} 占位符
    is_builtin: bool = False          # 是否内置预设
    created_at: datetime
    updated_at: Optional[datetime] = None
