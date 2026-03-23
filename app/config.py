"""
Application configuration management using Pydantic Settings
"""
from pydantic_settings import BaseSettings
from pydantic import Field
from typing import Optional
from pathlib import Path
import json
import platform


def get_default_hotkey() -> dict:
    """
    Get default hotkey based on platform.

    Returns:
        Hotkey config dict with modifiers, key, and display fields
    """
    if platform.system() == "Darwin":  # macOS
        return {
            "modifiers": ["alt", "cmd"],
            "key": None,
            "display": "⌥⌘"
        }
    else:  # Windows/Linux
        return {
            "modifiers": ["alt", "ctrl"],
            "key": None,
            "display": "Alt+Ctrl"
        }


class UserSettings:
    """
    User-configurable settings stored in data/settings.json
    """
    DEFAULT_SETTINGS = {
        "whisper_model": "large-v3",
        "whisper_device": "auto",
        "pause_audio_on_recording": True,
        "auto_optimize": False,
        "hotkey": None  # Will be set dynamically based on platform
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
                    # Set platform-specific default hotkey if not set
                    if settings.get("hotkey") is None:
                        settings["hotkey"] = get_default_hotkey()
                    return settings
            except (json.JSONDecodeError, IOError):
                settings = self.DEFAULT_SETTINGS.copy()
                settings["hotkey"] = get_default_hotkey()
                return settings
        settings = self.DEFAULT_SETTINGS.copy()
        settings["hotkey"] = get_default_hotkey()
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
