"""
AI Prompt Presets Module
Manages prompt templates for text optimization
"""
from app.prompts.models import (
    PromptCategory,
    PromptPreset,
    PromptPresetCreate,
    PromptPresetUpdate
)
from app.prompts.manager import PromptManager
from app.prompts.builtin_presets import BUILTIN_PRESETS

__all__ = [
    "PromptCategory",
    "PromptPreset",
    "PromptPresetCreate",
    "PromptPresetUpdate",
    "PromptManager",
    "BUILTIN_PRESETS"
]
