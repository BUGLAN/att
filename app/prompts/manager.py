"""
Prompt Preset Manager
Manages prompt presets - loads directly from prompts/ directory
"""
import json
from datetime import datetime
from typing import Optional, List
from pathlib import Path

from app.prompts.models import (
    PromptCategory,
    PromptPreset,
    PromptPresetCreate,
    PromptPresetUpdate
)
from app.utils.logging import get_logger

logger = get_logger(__name__)

# Prompts directory in project root
PROMPTS_DIR = Path(__file__).parent.parent.parent / "prompts"


class PromptManager:
    """Manages prompt presets loaded from prompts/ directory."""

    def __init__(self, data_dir: str = "data/prompts"):
        self.data_dir = Path(data_dir)
        self.presets_file = self.data_dir / "presets.json"
        self._ensure_data_dir()
        self._presets: dict[str, PromptPreset] = {}
        self._default_preset_id: Optional[str] = None
        self._last_used_preset_id: Optional[str] = None
        self._load_presets()

    def _ensure_data_dir(self) -> None:
        """Ensure data directory exists."""
        self.data_dir.mkdir(parents=True, exist_ok=True)

    def _load_presets(self) -> None:
        """Load presets from prompts/ directory and custom presets from JSON."""
        # Scan prompts/ directory for .txt files
        if PROMPTS_DIR.exists():
            for txt_file in sorted(PROMPTS_DIR.glob("*.txt")):
                try:
                    with open(txt_file, "r", encoding="utf-8") as f:
                        content = f.read().strip()

                    # Use filename (without .txt) as preset name
                    name = txt_file.stem
                    preset_id = f"preset-{name}"

                    preset = PromptPreset(
                        id=preset_id,
                        name=name,
                        description=f"从 {txt_file.name} 加载",
                        category=PromptCategory.GENERAL,
                        prompt_template=content,
                        is_builtin=True,
                        created_at=datetime.now()
                    )
                    self._presets[preset_id] = preset
                    logger.info(f"Loaded preset from file: {txt_file.name}")

                    # Set first preset as default
                    if self._default_preset_id is None:
                        self._default_preset_id = preset_id

                except Exception as e:
                    logger.error(f"Failed to load preset {txt_file}: {e}")

        # Load custom presets from JSON file
        if self.presets_file.exists():
            try:
                with open(self.presets_file, "r", encoding="utf-8") as f:
                    data = json.load(f)

                for preset_data in data.get("presets", []):
                    preset = PromptPreset(**preset_data)
                    self._presets[preset.id] = preset

                # Load settings
                if data.get("default_preset_id"):
                    self._default_preset_id = data.get("default_preset_id")
                if data.get("last_used_preset_id"):
                    self._last_used_preset_id = data.get("last_used_preset_id")

                logger.info(f"Loaded custom presets", count=len(data.get("presets", [])))
            except Exception as e:
                logger.error(f"Failed to load presets file: {e}")

    def _save_presets(self) -> None:
        """Save custom presets to file."""
        # Only save custom presets (not file-based presets)
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

    def get_default(self) -> Optional[PromptPreset]:
        """Get default preset."""
        if self._default_preset_id:
            return self._presets.get(self._default_preset_id)
        # Fallback to first preset
        if self._presets:
            return next(iter(self._presets.values()))
        return None

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

    def reload_from_files(self) -> None:
        """Reload presets from prompts/ directory (for hot reload)."""
        # Remove all file-based presets
        self._presets = {
            k: v for k, v in self._presets.items() if not v.is_builtin
        }
        self._default_preset_id = None

        # Re-scan prompts directory
        if PROMPTS_DIR.exists():
            for txt_file in sorted(PROMPTS_DIR.glob("*.txt")):
                try:
                    with open(txt_file, "r", encoding="utf-8") as f:
                        content = f.read().strip()

                    name = txt_file.stem
                    preset_id = f"preset-{name}"

                    preset = PromptPreset(
                        id=preset_id,
                        name=name,
                        description=f"从 {txt_file.name} 加载",
                        category=PromptCategory.GENERAL,
                        prompt_template=content,
                        is_builtin=True,
                        created_at=datetime.now()
                    )
                    self._presets[preset_id] = preset
                    logger.info(f"Reloaded preset from file: {txt_file.name}")

                    if self._default_preset_id is None:
                        self._default_preset_id = preset_id

                except Exception as e:
                    logger.error(f"Failed to reload preset {txt_file}: {e}")


# Global instance
prompt_manager = PromptManager()
