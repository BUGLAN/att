"""
Application configuration management using Pydantic Settings
"""
from pydantic_settings import BaseSettings
from pydantic import Field
from typing import Optional
from pathlib import Path
import json
import platform


VALID_HOTKEY_MODIFIERS = {"cmd", "alt", "ctrl", "shift", "win"}
VALID_HOTKEY_KEYS = set("abcdefghijklmnopqrstuvwxyz0123456789")
VALID_CLIPBOARD_TRIGGER_MODES = {"mouse_middle", "mouse_left_right_chord"}
VALID_CLIPBOARD_RETRIGGER_ACTIONS = {"toggle", "show_only"}
VALID_CLIPBOARD_ENTER_BEHAVIORS = {"copy_and_paste", "copy_only"}


def get_default_hotkey() -> dict:
    """
    Get default hotkey based on platform.

    Returns:
        Hotkey config dict with modifiers, key, and display fields
    """
    system_name = platform.system()

    if system_name == "Darwin":  # macOS
        return {
            "modifiers": ["cmd", "shift"],
            "key": "r",
            "display": "Cmd+Shift+R"
        }
    if system_name == "Windows":
        return {
            "modifiers": ["win", "alt"],
            "key": None,
            "display": "Win+Alt"
        }

    return {
        "modifiers": ["ctrl", "shift"],
        "key": "r",
        "display": "Ctrl+Shift+R"
    }


def is_valid_hotkey(hotkey: dict) -> bool:
    """Validate a persisted hotkey configuration."""
    if not isinstance(hotkey, dict):
        return False

    modifiers = hotkey.get("modifiers", [])
    key = hotkey.get("key")

    if not isinstance(modifiers, list) or not modifiers:
        return False

    if not all(modifier in VALID_HOTKEY_MODIFIERS for modifier in modifiers):
        return False

    if key is None:
        return len(modifiers) >= 2

    if not isinstance(key, str) or len(key) != 1:
        return False

    return key.lower() in VALID_HOTKEY_KEYS


def get_default_clipboard_overlay_settings() -> dict:
    """Return default clipboard overlay settings."""
    return {
        "clipboard_overlay_trigger": "mouse_middle",
        "clipboard_overlay_retrigger_action": "toggle",
        "clipboard_overlay_enter_behavior": "copy_and_paste",
        "clipboard_overlay_auto_focus_search": True,
        "clipboard_overlay_close_on_click_copy": False,
    }


def normalize_clipboard_overlay_settings(payload: Optional[dict]) -> dict:
    """Normalize clipboard overlay settings against supported values."""
    defaults = get_default_clipboard_overlay_settings()
    source = payload or {}
    normalized = dict(source)

    trigger = source.get("clipboard_overlay_trigger")
    normalized["clipboard_overlay_trigger"] = (
        trigger if trigger in VALID_CLIPBOARD_TRIGGER_MODES
        else defaults["clipboard_overlay_trigger"]
    )

    retrigger_action = source.get("clipboard_overlay_retrigger_action")
    normalized["clipboard_overlay_retrigger_action"] = (
        retrigger_action if retrigger_action in VALID_CLIPBOARD_RETRIGGER_ACTIONS
        else defaults["clipboard_overlay_retrigger_action"]
    )

    enter_behavior = source.get("clipboard_overlay_enter_behavior")
    normalized["clipboard_overlay_enter_behavior"] = (
        enter_behavior if enter_behavior in VALID_CLIPBOARD_ENTER_BEHAVIORS
        else defaults["clipboard_overlay_enter_behavior"]
    )

    auto_focus_search = source.get("clipboard_overlay_auto_focus_search")
    normalized["clipboard_overlay_auto_focus_search"] = (
        auto_focus_search
        if isinstance(auto_focus_search, bool)
        else defaults["clipboard_overlay_auto_focus_search"]
    )

    close_on_click_copy = source.get("clipboard_overlay_close_on_click_copy")
    normalized["clipboard_overlay_close_on_click_copy"] = (
        close_on_click_copy
        if isinstance(close_on_click_copy, bool)
        else defaults["clipboard_overlay_close_on_click_copy"]
    )

    return normalized


class UserSettings:
    """
    User-configurable settings stored in data/settings.json
    """
    DEFAULT_SETTINGS = {
        "whisper_model": "large-v3",
        "whisper_device": "auto",
        "pause_audio_on_recording": True,
        "auto_optimize": False,
        "auto_copy_to_clipboard": True,  # 自动复制转码文本到剪贴板
        "hotkey": None,  # Will be set dynamically based on platform
        "clipboard_overlay_trigger": "mouse_middle",
        "clipboard_overlay_retrigger_action": "toggle",
        "clipboard_overlay_enter_behavior": "copy_and_paste",
        "clipboard_overlay_auto_focus_search": True,
        "clipboard_overlay_close_on_click_copy": False,
        # Recording mode settings
        "recording_mode": "single",     # single or multi
        "silence_threshold": 1.5,       # 静音阈值（秒）
        "silence_level": -40,           # 静音音量阈值（dB）
        # Prompt preset settings
        "default_preset_id": None,
        "last_used_preset_id": None
    }

    def __init__(self, settings_path: Path):
        self.settings_path = settings_path
        self._settings = self._load()

    def _load(self) -> dict:
        """Load settings from file or create default."""
        if self.settings_path.exists():
            try:
                with open(self.settings_path, 'r', encoding='utf-8') as f:
                    loaded = json.load(f)
                    # Merge with defaults to ensure all keys exist
                    settings = {**self.DEFAULT_SETTINGS, **loaded}
                    # Normalize to a registerable hotkey on load.
                    if not is_valid_hotkey(settings.get("hotkey")):
                        settings["hotkey"] = get_default_hotkey()
                    settings = normalize_clipboard_overlay_settings(settings)
                    return settings
            except (json.JSONDecodeError, IOError):
                settings = self.DEFAULT_SETTINGS.copy()
                settings["hotkey"] = get_default_hotkey()
                settings = normalize_clipboard_overlay_settings(settings)
                return settings
        settings = self.DEFAULT_SETTINGS.copy()
        settings["hotkey"] = get_default_hotkey()
        settings = normalize_clipboard_overlay_settings(settings)
        return settings

    def _save(self) -> None:
        """Save settings to file."""
        self.settings_path.parent.mkdir(parents=True, exist_ok=True)
        with open(self.settings_path, 'w', encoding='utf-8') as f:
            json.dump(self._settings, f, indent=2, ensure_ascii=False)

    def get(self, key: str, default=None):
        """Get a setting value."""
        return self._settings.get(key, default)

    def set(self, key: str, value) -> None:
        """Set a setting value and save."""
        self._settings[key] = value
        self._save()

    def update(self, updates: dict) -> None:
        """Update multiple settings and save."""
        self._settings.update(updates)
        self._save()

    def get_all(self) -> dict:
        """Get all settings."""
        return self._settings.copy()


class Settings(BaseSettings):
    """Application settings loaded from environment variables"""

    # Application
    app_name: str = "Audio-to-Text API"
    debug: bool = False

    # Paths
    data_dir: str = "data"
    models_dir: str = "models"

    # Whisper Configuration
    whisper_model: str = "large-v3"
    whisper_device: str = "auto"  # auto, cuda, cpu, coreml
    whisper_compute_type: str = "int8"  # float16 (CUDA/CoreML), int8_float16, int8 (CPU)

    # DeepSeek API
    deepseek_api_key: str = ""
    deepseek_model: str = "deepseek-chat"
    deepseek_max_tokens: int = 4096
    deepseek_temperature: float = 0.3
    deepseek_base_url: str = "https://api.deepseek.com/v1"

    # Processing
    max_upload_size: int = 500 * 1024 * 1024  # 500MB
    chunk_duration: int = 30  # seconds

    # Audio Processing
    supported_formats: set = Field(
        default={'wav', 'mp3', 'm4a', 'flac', 'ogg', 'webm'}
    )
    target_sample_rate: int = 16000
    target_channels: int = 1

    @property
    def data_path(self) -> Path:
        return Path(self.data_dir)

    @property
    def models_path(self) -> Path:
        return Path(self.models_dir)

    @property
    def audio_temp_path(self) -> Path:
        return self.data_path / "audio" / "temp"

    @property
    def audio_permanent_path(self) -> Path:
        return self.data_path / "audio" / "permanent"

    @property
    def records_path(self) -> Path:
        """Path for JSON record files"""
        return self.data_path / "records"

    @property
    def audio_records_path(self) -> Path:
        """Path for WAV audio files"""
        return self.data_path / "audio" / "records"

    class Config:
        env_file = ".env"
        env_file_encoding = "utf-8"


# Global settings instance
settings = Settings()

# User settings instance (for configurable options)
user_settings = UserSettings(settings.data_path / "settings.json")
