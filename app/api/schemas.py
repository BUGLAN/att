"""API 请求模型：把「裸 dict + 手写白名单」收敛为可校验的 Pydantic 模型。

设计约束（与前端契约保持一致）：
1. 只声明 PUT /api/settings 已知的设置键；未知键按 Pydantic 默认行为静默丢弃，
   保留既有的宽松策略（Electron 桌面端同步会连带提交整份 settings 快照）。
2. 所有字段可选；显式传入 ``null`` 视为「本次未提供」，避免把 None 写进 settings.json。
3. 非法取值不在这里抛异常，由路由层捕获 ValidationError 后转成带中文说明的 400。
"""
from enum import Enum
from typing import Optional

from pydantic import BaseModel, ConfigDict, Field, ValidationError


class ASRProviderName(str, Enum):
    """受支持的 ASR provider。"""

    WHISPER = "whisper"
    QWEN_ASR = "qwen_asr"
    DOUBAO_ASR = "doubao_asr"
    CHATGPT_ASR = "chatgpt_asr"


class RecordingModeName(str, Enum):
    """受支持的录音模式。"""

    SINGLE = "single"
    MULTI = "multi"
    STREAMING = "streaming"


class UserSettingsUpdate(BaseModel):
    """PUT /api/settings 的设置更新体。

    字段名即白名单：未声明的键会被静默丢弃，因此前端可以继续整份提交。
    """

    # 未知键静默丢弃（ConfigDict(extra="ignore") 是 Pydantic 的默认值，这里显式写出以表明契约）
    model_config = ConfigDict(extra="ignore")

    # ASR 基础配置
    asr_provider: Optional[ASRProviderName] = None
    asr_language: Optional[str] = None

    # Whisper 配置
    whisper_model: Optional[str] = None
    whisper_device: Optional[str] = None

    # Qwen3-ASR 配置
    qwen_asr_model: Optional[str] = None
    qwen_asr_device: Optional[str] = None
    qwen_asr_dtype: Optional[str] = None
    qwen_asr_max_new_tokens: Optional[int] = Field(default=None, ge=1, le=32768)
    qwen_asr_return_timestamps: Optional[bool] = None
    qwen_asr_use_forced_aligner: Optional[bool] = None
    qwen_asr_forced_aligner_model: Optional[str] = None
    qwen_asr_max_audio_seconds: Optional[int] = Field(default=None, ge=1, le=3600)

    # 火山引擎豆包双向流式 ASR（在线识别）
    # doubao_asr_api_key 是「只写字段」：写入后立即加密成
    # doubao_asr_api_key_encrypted 落盘，GET /api/settings 只回报 doubao_asr_api_key_set。
    doubao_asr_api_key: Optional[str] = None
    # 显式清除已保存的 Access Token（避免「空串 = 未提供」导致无法清空）
    doubao_asr_api_key_clear: Optional[bool] = None
    doubao_asr_resource_id: Optional[str] = None
    doubao_asr_endpoint: Optional[str] = None
    # 0 = 不设置强制断句；>0 时按该静音时长（毫秒）输出 definite
    doubao_asr_end_window_size: Optional[int] = Field(default=None, ge=0, le=10000)

    # ChatGPT 听写 ASR（在线批量识别，实现移植自 Wangnov/codex-asr）
    # chatgpt_asr_api_key 同样是「只写字段」：写入后立即加密成
    # chatgpt_asr_api_key_encrypted 落盘，GET /api/settings 只回报 chatgpt_asr_api_key_set。
    chatgpt_asr_api_key: Optional[str] = None
    # 显式清除已保存的访问令牌（避免「空串 = 未提供」导致无法清空）
    chatgpt_asr_api_key_clear: Optional[bool] = None
    chatgpt_asr_endpoint: Optional[str] = None
    chatgpt_asr_timeout_seconds: Optional[int] = Field(default=None, ge=10, le=600)

    # 录音与剪贴板行为
    pause_audio_on_recording: Optional[bool] = None
    auto_optimize: Optional[bool] = None
    auto_copy_to_clipboard: Optional[bool] = None
    hotkey: Optional[dict] = None
    # 已下线功能的键（划词工具栏 / 剪贴板鼠标触发 / 统一浮层全套 clipboard_overlay_*、
    # selection_toolbar_*）不再声明：未声明的键由 extra="ignore" 静默丢弃，
    # 旧渲染进程整份提交也不会写回；旧 settings.json 的残留值由
    # strip_retired_settings 统一剥离。

    # 录音模式与静音参数
    recording_mode: Optional[RecordingModeName] = None
    silence_threshold: Optional[float] = Field(default=None, ge=0, le=60)
    silence_level: Optional[float] = Field(default=None, ge=-100, le=0)

    # 提示词预设
    default_preset_id: Optional[str] = None
    last_used_preset_id: Optional[str] = None

    # 音频输入设备
    audio_input_device: Optional[str] = None


# pydantic 错误类型 → 中文说明；未覆盖的类型回落到 pydantic 原始描述
_VALIDATION_ERROR_MESSAGES = {
    "int_parsing": "必须是整数",
    "int_type": "必须是整数",
    "float_parsing": "必须是数字",
    "float_type": "必须是数字",
    "bool_parsing": "必须是布尔值（true/false）",
    "bool_type": "必须是布尔值（true/false）",
    "string_type": "必须是字符串",
    "dict_type": "必须是对象",
    "enum": "取值不在允许的枚举范围内",
    "greater_than_equal": "数值小于允许的最小值",
    "less_than_equal": "数值超过允许的最大值",
    "missing": "缺少必填项",
}


def format_validation_error(error: ValidationError) -> str:
    """把 Pydantic 校验错误转成可读的中文提示。"""
    messages = []
    for item in error.errors():
        location = ".".join(str(part) for part in item.get("loc", ())) or "body"
        error_type = item.get("type", "")
        reason = _VALIDATION_ERROR_MESSAGES.get(error_type)
        if reason is None:
            reason = str(item.get("msg", "取值非法"))
        messages.append(f"{location} {reason}")
    return "设置项校验失败：" + "；".join(messages)
