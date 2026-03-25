"""
Prompt Preset Manager
Manages prompt presets storage and CRUD operations
"""
import json
import os
from datetime import datetime
from typing import Optional, List
from pathlib import Path

from app.prompts.models import (
    PromptCategory,
    PromptPreset,
    PromptPresetCreate,
    PromptPresetUpdate
)
from app.prompts.builtin_presets import BUILTIN_PRESETS
from app.utils.logging import get_logger

logger = get_logger(__name__)


class PromptManager:
    """Manages prompt presets with JSON file storage."""

    def __init__(self, data_dir: str = "data/prompts"):
        self.data_dir = Path(data_dir)
        self.presets_file = self.data_dir / "presets.json"
        self._ensure_data_dir()
        self._presets: dict[str, PromptPreset] = {}
        self._default_preset_id: str = "builtin-general"
        self._last_used_preset_id: Optional[str] = None
        self._load_presets()

    def _ensure_data_dir(self) -> None:
        """Ensure data directory exists."""
        self.data_dir.mkdir(parents=True, exist_ok=True)

    def _load_presets(self) -> None:
        """Load presets from file and merge with builtins."""
        # Load built-in presets first
        for preset_data in BUILTIN_PRESETS.values():
            preset = PromptPreset(
                **preset_data,
                created_at=datetime.now()
            )
            self._presets[preset.id] = preset

        # Load custom presets from file
        if self.presets_file.exists():
            try:
                with open(self.presets_file, "r", encoding="utf-8") as f:
                    data = json.load(f)

                # Load custom presets
                for preset_data in data.get("presets", []):
                    preset = PromptPreset(**preset_data)
                    self._presets[preset.id] = preset

                # Load settings
                self._default_preset_id = data.get(
                    "default_preset_id",
                    "builtin-general"
                )
                self._last_used_preset_id = data.get("last_used_preset_id")

                logger.info(
                    "Loaded presets from file",
                    total=len(self._presets),
                    custom=len(data.get("presets", []))
                )
            except Exception as e:
                logger.error("Failed to load presets file", error=str(e))

    def _save_presets(self) -> None:
        """Save custom presets to file."""
        # Only save custom presets (not builtins)
        custom_presets = [
            p.model_dump()
            for p in self._presets.values()
            if not p.is_builtin
        ]

        data = {
            "presets": custom_presets,
            "default_preset_id": self._default_preset_id,
            "last_used_preset_id": self._last_used_preset_id
        }

        try:
            with open(self.presets_file, "w", encoding="utf-8") as f:
                json.dump(data, f, ensure_ascii=False, indent=2, default=str)
            logger.info("Saved presets to file", custom_count=len(custom_presets))
        except Exception as e:
            logger.error("Failed to save presets file", error=str(e))
            raise

    def get_all(self) -> List[PromptPreset]:
        """Get all presets."""
        return list(self._presets.values())

    def get_by_id(self, preset_id: str) -> Optional[PromptPreset]:
        """Get preset by ID."""
        return self._presets.get(preset_id)

    def get_default(self) -> PromptPreset:
        """Get default preset."""
        preset = self._presets.get(self._default_preset_id)
        if not preset:
            # Fallback to general preset
            preset = self._presets.get("builtin-general")
        return preset

    def get_last_used(self) -> Optional[PromptPreset]:
        """Get last used preset."""
        if self._last_used_preset_id:
            return self._presets.get(self._last_used_preset_id)
        return None

    def create(self, create_data: PromptPresetCreate) -> PromptPreset:
        """Create a new custom preset."""
        # Generate unique ID
        timestamp = int(datetime.now().timestamp() * 1000)
        preset_id = f"custom-{timestamp}"

        preset = PromptPreset(
            id=preset_id,
            name=create_data.name,
            description=create_data.description,
            category=create_data.category,
            prompt_template=create_data.prompt_template,
            is_builtin=False,
            created_at=datetime.now()
        )

        self._presets[preset_id] = preset
        self._save_presets()

        logger.info("Created preset", preset_id=preset_id, name=preset.name)
        return preset

    def update(
        self,
        preset_id: str,
        update_data: PromptPresetUpdate
    ) -> Optional[PromptPreset]:
        """Update an existing preset."""
        preset = self._presets.get(preset_id)
        if not preset:
            return None

        # Cannot update built-in presets
        if preset.is_builtin:
            raise ValueError("Cannot update built-in preset")

        # Update fields
        update_dict = update_data.model_dump(exclude_unset=True)
        for key, value in update_dict.items():
            if value is not None:
                setattr(preset, key, value)

        preset.updated_at = datetime.now()
        self._save_presets()

        logger.info("Updated preset", preset_id=preset_id)
        return preset

    def delete(self, preset_id: str) -> bool:
        """Delete a custom preset."""
        preset = self._presets.get(preset_id)
        if not preset:
            return False

        # Cannot delete built-in presets
        if preset.is_builtin:
            raise ValueError("Cannot delete built-in preset")

        del self._presets[preset_id]
        self._save_presets()

        logger.info("Deleted preset", preset_id=preset_id)
        return True

    def set_default(self, preset_id: str) -> bool:
        """Set default preset."""
        if preset_id not in self._presets:
            return False

        self._default_preset_id = preset_id
        self._save_presets()

        logger.info("Set default preset", preset_id=preset_id)
        return True

    def set_last_used(self, preset_id: str) -> bool:
        """Set last used preset."""
        if preset_id not in self._presets:
            return False

        self._last_used_preset_id = preset_id
        self._save_presets()

        return True

    def render_prompt(self, preset_id: str, text: str) -> Optional[str]:
        """Render prompt template with text."""
        preset = self._presets.get(preset_id)
        if not preset:
            return None

        try:
            return preset.prompt_template.format(text=text)
        except KeyError:
            # Template doesn't have {text} placeholder
            return preset.prompt_template


# Global instance
prompt_manager = PromptManager()
