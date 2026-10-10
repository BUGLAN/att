"""
Application configuration management using Pydantic Settings
"""
from pydantic_settings import BaseSettings, SettingsConfigDict
from pydantic import Field
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
VALID_CLIPBOARD_RETRIGGER_ACTIONS = {"toggle", "show_only"}
VALID_CLIPBOARD_ENTER_BEHAVIORS = {"copy_and_paste", "copy_only"}

# 已下线功能的僵尸配置键：第 1 轮已删除「划词工具栏」与「桌面鼠标触发」两条链路，
# 这里集中登记，确保它们不再出现在默认值、归一化结果与 PUT 白名单中。
#
# 注意：selection_toolbar_show_examples / selection_toolbar_auto_add_to_wordbook
# **不在**此列 —— 旧「划词工具栏」虽然删了，但统一悬浮层里的「划词面板」
# （src/components/overlay/SelectionPanel.vue）仍在读取这两个键来控制
# 「显示示例」与「自动加入生词本」。一旦把它们当作僵尸键剥离，
# 面板会因 `undefined !== false` 而回落到 true，静默改写用户已设为 false 的偏好。
RETIRED_SETTINGS_KEYS = frozenset({
    "selection_toolbar_enabled",
    "selection_toolbar_trigger_delay_ms",
    "selection_toolbar_fallback_to_clipboard",
    "clipboard_overlay_trigger",
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


def get_default_clipboard_overlay_settings() -> dict:
    """Return default clipboard overlay settings."""
    return {
        "clipboard_overlay_retrigger_action": "toggle",
        "clipboard_overlay_enter_behavior": "copy_and_paste",
        "clipboard_overlay_auto_focus_search": True,
        "clipboard_overlay_close_on_click_copy": True,
    }


def normalize_clipboard_overlay_settings(payload: Optional[dict]) -> dict:
    """Normalize clipboard overlay settings against supported values."""
    defaults = get_default_clipboard_overlay_settings()
    source = payload or {}
    # 丢弃已下线功能的残留键：兼容含残留键的旧 settings.json，读到不报错也不会写回
    normalized = {
        key: value
        for key, value in source.items()
        if key not in RETIRED_SETTINGS_KEYS
    }

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
        "clipboard_overlay_retrigger_action": "toggle",
        "clipboard_overlay_enter_behavior": "copy_and_paste",
        "clipboard_overlay_auto_focus_search": True,
        "clipboard_overlay_close_on_click_copy": True,
        # 划词面板（统一悬浮层的 selection 模式）仍在使用的两个键：
        # 旧「划词工具栏」已下线，但面板本身是活的，不能当作僵尸键删除。
        # 若将来要下线，必须先改 SelectionPanel.vue 的读取逻辑，否则行为会静默变成 true。
        "selection_toolbar_show_examples": True,
        "selection_toolbar_auto_add_to_wordbook": True,
        # 已下线功能（划词工具栏开关 / 触发延迟 / 剪贴板鼠标触发）不再提供默认值，
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

        payload = ensure_dict(loaded, logger=logger, source=self.settings_path)

        # Merge with defaults to ensure all keys exist
        settings = {**self.DEFAULT_SETTINGS, **payload}

        # Normalize to a registerable hotkey on load.
        if not is_valid_hotkey(settings.get("hotkey")):
            settings["hotkey"] = get_default_hotkey()

        return normalize_clipboard_overlay_settings(settings)

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
    qwen_streaming_gpu_memory_utilization: float = 0.85
    qwen_streaming_max_model_len: int = 16384

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
