"""
Application configuration management using Pydantic Settings
"""
from pydantic_settings import BaseSettings, SettingsConfigDict
from typing import Optional
from pathlib import Path
import logging
import os
import platform

from app.repositories.json_store import (
    ensure_dict,
    get_thread_lock,
    read_json_lenient,
    write_json_atomic,
)

logger = logging.getLogger(__name__)


VALID_HOTKEY_MODIFIERS = {"cmd", "alt", "ctrl", "shift", "win"}
VALID_HOTKEY_KEYS = set("abcdefghijklmnopqrstuvwxyz0123456789")

# 已下线功能的僵尸配置键：历轮删除「划词工具栏」「桌面鼠标触发」「统一浮层
# （含划词面板）」后，这里集中登记它们的全部配置键，
# 确保这些键不再出现在默认值、归一化结果与 PUT 白名单中。
RETIRED_SETTINGS_KEYS = frozenset({
    "selection_toolbar_enabled",
    "selection_toolbar_trigger_delay_ms",
    "selection_toolbar_fallback_to_clipboard",
    "selection_toolbar_show_examples",
    "selection_toolbar_auto_add_to_wordbook",
    "clipboard_overlay_trigger",
    "clipboard_overlay_retrigger_action",
    "clipboard_overlay_enter_behavior",
    "clipboard_overlay_auto_focus_search",
    "clipboard_overlay_close_on_click_copy",
})


def is_windows_host() -> bool:
    """判断当前 Python 进程是否运行在 Windows。"""
    return platform.system() == "Windows"


def is_wsl_environment() -> bool:
    """判断当前 Linux 进程是否运行在 WSL。"""
    if platform.system() != "Linux":
        return False

    if os.getenv("WSL_INTEROP") or os.getenv("WSL_DISTRO_NAME"):
        return True

    probe_files = (
        Path("/proc/sys/kernel/osrelease"),
        Path("/proc/version"),
    )
    for probe_file in probe_files:
        try:
            content = probe_file.read_text(encoding="utf-8").lower()
        except OSError:
            continue

        if "microsoft" in content or "wsl" in content:
            return True

    return False


def get_runtime_environment() -> str:
    """返回当前后端运行环境标识。"""
    if is_wsl_environment():
        return "wsl2"

    system_name = platform.system()
    if system_name == "Windows":
        return "windows"

    if system_name == "Darwin":
        return "macos"

    if system_name == "Linux":
        return "linux"

    return system_name.lower() or "unknown"


def supports_local_vllm_streaming() -> bool:
    """判断当前环境是否允许在主 FastAPI 进程内启用本地 vLLM 流式。"""
    return not is_windows_host()


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


def strip_retired_settings(payload: Optional[dict]) -> dict:
    """丢弃已下线功能的残留键：兼容含残留键的旧 settings.json，读到不报错也不会写回。"""
    source = payload or {}
    return {
        key: value
        for key, value in source.items()
        if key not in RETIRED_SETTINGS_KEYS
    }


class UserSettings:
    """
    User-configurable settings stored in data/settings.json
    """
    DEFAULT_SETTINGS = {
        "asr_provider": "whisper",
        "asr_language": "zh",
        "whisper_model": "large-v3",
        "whisper_device": "auto",
        "qwen_asr_model": "Qwen/Qwen3-ASR-1.7B",
        "qwen_asr_device": "auto",
        "qwen_asr_dtype": "auto",
        "qwen_asr_max_new_tokens": 256,
        "qwen_asr_return_timestamps": False,
        "qwen_asr_use_forced_aligner": False,
        "qwen_asr_forced_aligner_model": "Qwen/Qwen3-ForcedAligner-0.6B",
        "qwen_asr_max_audio_seconds": 30,
        # 火山引擎豆包双向流式 ASR（在线识别）
        # 注意：Access Token 明文不落盘，只以 doubao_asr_api_key_encrypted 形式写入，
        # 因此这里没有 doubao_asr_api_key 的默认值（它只作为 PUT 的写入字段存在）。
        "doubao_asr_resource_id": "volc.seedasr.sauc.duration",
        "doubao_asr_endpoint": "wss://openspeech.bytedance.com/api/v3/sauc/bigmodel",
        "doubao_asr_end_window_size": 0,
        "pause_audio_on_recording": True,
        "auto_optimize": False,
        "auto_copy_to_clipboard": True,  # 自动复制转码文本到剪贴板
        "hotkey": None,  # Will be set dynamically based on platform
        # 已下线功能（划词工具栏 / 剪贴板鼠标触发 / 统一浮层）不再提供默认值，
        # 见文件顶部 RETIRED_SETTINGS_KEYS
        # Recording mode settings
        "recording_mode": "single",     # single / multi / streaming
        "silence_threshold": 1.5,       # 静音阈值（秒）
        "silence_level": -40,           # 静音音量阈值（dB）
        # Prompt preset settings
        "default_preset_id": None,
        "last_used_preset_id": None
    }

    def __init__(self, settings_path: Path):
        self.settings_path = settings_path
        # 读-改-写串行化：settings.json 是唯一真相源，同一时刻只允许一个写序列
        self._lock = get_thread_lock(settings_path)
        self._settings = self._load()

    def _load(self) -> dict:
        """Load settings from file or create default.

        形状容错：文件内容为 `null` / `[]` / `"str"`（顶层非对象）时按空对象处理，
        绝不让 import 期抛 TypeError 把后端直接卡死在启动阶段。
        """
        loaded = read_json_lenient(self.settings_path, default=None)

        if loaded is None and self.settings_path.exists():
            logger.warning(
                "settings.json 无法解析或为空，已回落到默认设置: %s",
                self.settings_path,
            )

        payload = strip_retired_settings(ensure_dict(loaded, logger=logger, source=self.settings_path))

        # Merge with defaults to ensure all keys exist
        settings = {**self.DEFAULT_SETTINGS, **payload}

        # Normalize to a registerable hotkey on load.
        if not is_valid_hotkey(settings.get("hotkey")):
            settings["hotkey"] = get_default_hotkey()

        return settings

    def _save(self) -> None:
        """Save settings to file（原子写，保持 indent=2 / ensure_ascii=False）。"""
        with self._lock:
            write_json_atomic(self.settings_path, self._settings)

    def get(self, key: str, default=None):
        """Get a setting value."""
        return self._settings.get(key, default)

    def set(self, key: str, value) -> None:
        """Set a setting value and save."""
        with self._lock:
            self._settings[key] = value
            self._save()

    def update(self, updates: dict) -> None:
        """Update multiple settings and save."""
        with self._lock:
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

    # ASR Configuration
    asr_provider: str = "whisper"
    asr_language: str = "zh"
    asr_preload_on_startup: bool = False

    # Whisper Configuration
    whisper_model: str = "large-v3"
    whisper_device: str = "auto"  # auto, cuda, cpu, coreml
    whisper_compute_type: str = "int8"  # float16 (CUDA/CoreML), int8_float16, int8 (CPU)

    # Qwen3-ASR Configuration
    qwen_asr_model: str = "Qwen/Qwen3-ASR-1.7B"
    qwen_asr_device: str = "auto"
    qwen_asr_dtype: str = "auto"
    qwen_asr_max_new_tokens: int = 256
    qwen_asr_return_timestamps: bool = False
    qwen_asr_use_forced_aligner: bool = False
    qwen_asr_forced_aligner_model: str = "Qwen/Qwen3-ForcedAligner-0.6B"
    qwen_asr_max_audio_seconds: int = 30
    qwen_streaming_backend: str = "auto"
    qwen_streaming_base_url: str = ""
    qwen_streaming_timeout_seconds: float = 30.0

    # 火山引擎豆包双向流式 ASR（在线识别）
    # Access Token 的落盘形态是 data/settings.json 里的加密字段，这里只作为环境变量兜底
    doubao_asr_api_key: str = ""
    doubao_asr_resource_id: str = "volc.seedasr.sauc.duration"
    doubao_asr_endpoint: str = "wss://openspeech.bytedance.com/api/v3/sauc/bigmodel"
    doubao_asr_connect_timeout_seconds: float = 10.0
    doubao_asr_chunk_ms: int = 200
    # 0 = 不设置强制断句，跟随服务端语义分段；>0 时按该静音时长（毫秒）输出 definite
    doubao_asr_end_window_size: int = 0
    doubao_asr_force_to_speech_time: int = 0

    # LLM 接入已迁移到 LiteLLM + data/llm_v2.json（供应商档案 + 模型条目），
    # 旧的 DEEPSEEK_* 环境变量与「.env 内置 key 兜底」路径一并移除（2026-10）。
    # extra="ignore" 会让残留的 DEEPSEEK_* 键被安全忽略，不报错。

    # 本地文件转写接口（POST /api/transcribe，采集层脚本与 App「转文字」页共用）
    transcribe_file_max_duration_seconds: int = 7200      # 时长上限，超过返回 413（默认 2 小时）
    transcribe_file_async_threshold_seconds: int = 1800   # 超过该时长改走 202 异步任务（默认 30 分钟）
    transcribe_file_api_token: str = ""                   # 非空时要求请求头 X-ATT-Token 匹配
    transcribe_file_busy_retry_after_seconds: int = 30    # 引擎忙（503）时 Retry-After 的秒数
    transcribe_file_cache_size: int = 200                 # 内容哈希结果缓存的条数上限（LRU）

    # Processing
    max_upload_size: int = 500 * 1024 * 1024  # 500MB
    # 注：chunk_duration / supported_formats / target_sample_rate / target_channels
    # 曾是零读取的死字段（真实生效的是 AudioProcessor 的类常量），已在功能减法中删除。

    @property
    def data_path(self) -> Path:
        return Path(self.data_dir)

    @property
    def models_path(self) -> Path:
        return Path(self.models_dir)

    @property
    def faster_whisper_models_path(self) -> Path:
        """faster-whisper 模型缓存目录"""
        return self.models_path / "faster_whisper"

    @property
    def qwen_models_path(self) -> Path:
        """Qwen 模型缓存目录"""
        return self.models_path / "qwen"

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

    @property
    def transcribe_file_cache_path(self) -> Path:
        """文件转写结果缓存（key 为文件内容 sha256，value 为上次响应体）"""
        return self.data_path / "transcribe_file_cache.json"

    # Pydantic 2 风格配置：迁移自 v1 的 class Config，消除弃用告警
    # extra="ignore" 是必需的：`.env` 是**全项目共用**的配置文件，除本类字段外还包含
    # docker-compose（VLLM_IMAGE / ATT_BACKEND_PORT）、桥接脚本（QWEN_STREAMING_MODEL 等）
    # 读取的键。pydantic-settings 对 env 文件里的未知键默认 extra="forbid"，
    # 一旦照文档「复制 .env.example 为 .env」就会直接 ValidationError、后端起不来。
    model_config = SettingsConfigDict(
        env_file=".env",
        env_file_encoding="utf-8",
        extra="ignore",
    )


# Global settings instance
settings = Settings()

# User settings instance (for configurable options)
user_settings = UserSettings(settings.data_path / "settings.json")
