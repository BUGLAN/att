"""
Application configuration management using Pydantic Settings
"""
from pydantic_settings import BaseSettings
from pydantic import Field
from typing import Optional
from pathlib import Path


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
    whisper_device: str = "auto"  # auto, cuda, cpu
    whisper_compute_type: str = "float16"  # float16, int8_float16, int8

    # DeepSeek API
    deepseek_api_key: str = ""
    deepseek_model: str = "deepseek-chat"
    deepseek_max_tokens: int = 4096
    deepseek_temperature: float = 0.3
    deepseek_base_url: str = "https://api.deepseek.com/v1"

    # Processing
    max_upload_size: int = 500 * 1024 * 1024  # 500MB
    chunk_duration: int = 30  # seconds

    # Database
    database_url: str = "sqlite+aiosqlite:///data/fragments.db"

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

    class Config:
        env_file = ".env"
        env_file_encoding = "utf-8"


# Global settings instance
settings = Settings()
