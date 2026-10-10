"""
Prompt preset manager backed only by prompts/*.txt.

预设是只读的：唯一真相源是 prompts/*.txt 文件，改预设 = 改文件。
历史上提供过的 create/update/delete 会主动抛 ValueError（恒 400），
已随写接口一并删除，避免「接口看着齐全、一写就失败」的僵尸契约。
"""
from datetime import datetime
from pathlib import Path
from typing import List, Optional

from app.prompts.models import PromptCategory, PromptPreset
from app.utils.logging import get_logger

logger = get_logger(__name__)

PROMPTS_DIR = Path(__file__).parent.parent.parent / "prompts"
DEFAULT_PROMPT_NAME = "通用优化"


def build_preset_id(name: str) -> str:
    """Build a stable preset id from a prompt filename stem."""
    return f"preset-{name}"


class PromptManager:
    """Reads prompt presets directly from prompts/*.txt."""

    def __init__(self) -> None:
        self._presets: dict[str, PromptPreset] = {}
        self.reload_from_files()

    def reload_from_files(self) -> None:
        """Reload prompt presets from disk."""
        self._presets = {}

        if not PROMPTS_DIR.exists():
            logger.warning("Prompts directory not found", path=str(PROMPTS_DIR))
            return

        for txt_file in sorted(PROMPTS_DIR.glob("*.txt"), key=lambda item: item.stem):
            try:
                content = txt_file.read_text(encoding="utf-8").strip()
                if not content:
                    logger.warning("Skipping empty prompt file", file=txt_file.name)
                    continue

                name = txt_file.stem
                preset_id = build_preset_id(name)
                modified_at = datetime.fromtimestamp(txt_file.stat().st_mtime)

                self._presets[preset_id] = PromptPreset(
                    id=preset_id,
                    name=name,
                    description=f"来自 prompts/{txt_file.name}",
                    category=PromptCategory.GENERAL,
                    prompt_template=content,
                    is_builtin=True,
                    created_at=modified_at,
                    updated_at=modified_at,
                )
            except Exception as error:
                logger.error("Failed to load prompt file", file=str(txt_file), error=str(error))

        logger.info("Loaded prompt presets", count=len(self._presets))

    def get_all(self) -> List[PromptPreset]:
        """Return all prompt presets from files."""
        return sorted(self._presets.values(), key=lambda preset: preset.name)

    def get_by_id(self, preset_id: str) -> Optional[PromptPreset]:
        """Return preset by id."""
        return self._presets.get(preset_id)

    def get_default_id(self) -> Optional[str]:
        """Return the fallback default preset id."""
        preferred_id = build_preset_id(DEFAULT_PROMPT_NAME)
        if preferred_id in self._presets:
            return preferred_id

        presets = self.get_all()
        return presets[0].id if presets else None

    def get_default(self) -> Optional[PromptPreset]:
        """Return the fallback default preset."""
        default_id = self.get_default_id()
        return self._presets.get(default_id) if default_id else None

    def set_default(self, preset_id: str) -> bool:
        """Validate that a preset id exists."""
        return preset_id in self._presets

    def render_prompt(self, preset_id: Optional[str], text: str) -> Optional[str]:
        """Render a prompt template with text."""
        preset = self.get_by_id(preset_id) if preset_id else self.get_default()
        if not preset:
            return None

        template = preset.prompt_template
        if "{text}" in template:
            return template.format(text=text)

        return f"{template.rstrip()}\n\n{text}".strip()


prompt_manager = PromptManager()
