"""设置编排服务：settings 归一化 + provider 重建 + 模型服务启停。

从 `app/api/routes.py` 的 `get_settings` / `update_settings`（原 102 行最长端点）
下沉而来，路由只负责「解析请求 → 调本服务 → 组装响应」。

保持不变的行为约束（都有回归测试覆盖）：

1. 白名单与类型校验统一交给 ``UserSettingsUpdate``：未知键静默丢弃，
   类型/枚举/范围非法 → 400 + 中文说明；
2. 显式传入 ``null`` 视为「本次未提供」，不写进 settings.json；
3. 归一化（提示词预设 id 回落 + 浮层枚举修复）在写入前后各执行一次；
4. 模型运行时配置变更 → 先 stop 模型服务 + 关闭流式会话管理器；
   provider 相关键变更 → 关闭旧 provider 并重建（失败返回 500，但设置已落盘）；
5. ``GET /api/settings`` 默认纯读：只有确实缺失/失效的键才会合并成一次写盘。
"""

from __future__ import annotations

from dataclasses import dataclass, field
from typing import Optional

from pydantic import ValidationError

from app.api.schemas import UserSettingsUpdate, format_validation_error
from app.config import is_valid_hotkey, normalize_clipboard_overlay_settings
from app.prompts import PromptManager, prompt_manager
from app.stt.base import ASRProvider
from app.stt.factory import create_asr_provider, get_asr_provider_config
from app.utils.logging import get_logger

logger = get_logger(__name__)

# 会改变模型运行时（需要卸载模型服务）的键
MODEL_RUNTIME_KEYS = {
    "asr_provider",
    "whisper_model",
    "whisper_device",
    "qwen_asr_model",
    "qwen_asr_device",
    "qwen_asr_dtype",
    "qwen_asr_return_timestamps",
    "qwen_asr_use_forced_aligner",
    "qwen_asr_forced_aligner_model",
    "recording_mode",
    # 豆包在线 ASR：凭据/接入点变化必须重建 provider，否则运行时会一直用旧 token
    "doubao_asr_api_key_encrypted",
    "doubao_asr_resource_id",
    "doubao_asr_endpoint",
    "doubao_asr_end_window_size",
}
# 需要重建 provider 的键（recording_mode 只影响运行时行为，不需要重建）
PROVIDER_REBUILD_KEYS = MODEL_RUNTIME_KEYS - {"recording_mode"}

# 豆包 Access Token 的落盘字段：GET /api/settings 只回报是否已配置，绝不回传密文
DOUBAO_SECRET_STORED_KEY = "doubao_asr_api_key_encrypted"
DOUBAO_SECRET_WRITE_KEY = "doubao_asr_api_key"
DOUBAO_SECRET_CLEAR_KEY = "doubao_asr_api_key_clear"
DOUBAO_SECRET_FLAG_KEY = "doubao_asr_api_key_set"

# 浮层相关的可修复键（枚举值会随版本变化而失效）
CLIPBOARD_OVERLAY_KEYS = {
    # 已下线的 clipboard_overlay_trigger 不再参与归一化，故不在此登记
    "clipboard_overlay_retrigger_action",
    "clipboard_overlay_enter_behavior",
    "clipboard_overlay_auto_focus_search",
    "clipboard_overlay_close_on_click_copy",
}


class SettingsValidationError(Exception):
    """设置校验/重建失败：由路由层翻译成对应 HTTP 错误。"""

    def __init__(self, message: str, status_code: int = 400) -> None:
        super().__init__(message)
        self.message = message
        self.status_code = status_code


@dataclass
class SettingsUpdateResult:
    """PUT /api/settings 的编排结果。"""

    payload: dict
    # 非 None 表示 provider 已重建，调用方需要把它写回运行时状态
    asr_provider: Optional[ASRProvider] = None
    provider_rebuilt: bool = False
    changed_keys: set = field(default_factory=set)


def get_prompt_manager() -> PromptManager:
    """返回共享的提示词管理器（每次调用重载磁盘上的 prompts/*.txt）。"""
    prompt_manager.reload_from_files()
    return prompt_manager


def normalize_preset_settings(payload: dict, *, user_settings, persist: bool = False) -> dict:
    """把提示词预设相关设置按 prompts/*.txt 归一化（可选的失效值修复落盘）。"""
    pm = get_prompt_manager()
    preset_ids = {preset.id for preset in pm.get_all()}
    fallback_default_id = pm.get_default_id()

    resolved_default_id = payload.get("default_preset_id")
    if resolved_default_id not in preset_ids:
        resolved_default_id = fallback_default_id

    resolved_last_used_id = payload.get("last_used_preset_id")
    if resolved_last_used_id not in preset_ids:
        resolved_last_used_id = resolved_default_id

    normalized = {
        **payload,
        "default_preset_id": resolved_default_id,
        "last_used_preset_id": resolved_last_used_id
    }

    if persist:
        updates = {}
        if payload.get("default_preset_id") != resolved_default_id:
            updates["default_preset_id"] = resolved_default_id
        if payload.get("last_used_preset_id") != resolved_last_used_id:
            updates["last_used_preset_id"] = resolved_last_used_id
        if updates:
            user_settings.update(updates)

    return normalized


def validate_hotkey(hotkey: dict) -> bool:
    """校验热键配置（转发给 app.config 的共享实现）。"""
    return is_valid_hotkey(hotkey)


def mask_doubao_secret(payload: dict) -> dict:
    """把豆包 Access Token 的密文换成布尔标记。

    GET /api/settings 会被渲染进程整份读取并回传，密文既无用处也不该外泄，
    因此响应里只保留「是否已配置」，明文与密文都不返回。
    """
    masked = {
        key: value
        for key, value in payload.items()
        if key != DOUBAO_SECRET_STORED_KEY
    }
    masked[DOUBAO_SECRET_FLAG_KEY] = bool(payload.get(DOUBAO_SECRET_STORED_KEY))
    return masked


def _apply_doubao_secret_update(filtered: dict) -> None:
    """把明文 Access Token 换成密文（就地修改 filtered）。

    - 非空明文 → 加密后写入 ``doubao_asr_api_key_encrypted``；
    - 空串/未传 → 视为「本次未提供」，保留已存密钥；
    - ``doubao_asr_api_key_clear=true`` → 显式清除。

    明文键本身绝不写进 settings.json，也不进日志。
    """
    clear_requested = bool(filtered.pop(DOUBAO_SECRET_CLEAR_KEY, False))
    raw_key = filtered.pop(DOUBAO_SECRET_WRITE_KEY, None)
    raw_key = str(raw_key).strip() if raw_key else ""

    if raw_key:
        from app.utils.encryption import get_encryption

        filtered[DOUBAO_SECRET_STORED_KEY] = get_encryption().encrypt(raw_key)
        return

    if clear_requested:
        filtered[DOUBAO_SECRET_STORED_KEY] = ""


def build_settings_response(*, user_settings) -> dict:
    """组装 GET /api/settings 的响应体（纯读，必要时单次修复落盘）。"""
    stored_settings = user_settings.get_all()
    # 纯归一化（persist=False）：不再由归一化函数内部隐式落盘
    payload = normalize_preset_settings(stored_settings, user_settings=user_settings)
    normalized_payload = normalize_clipboard_overlay_settings(payload)

    # 判断依据：
    # 1) 磁盘中不存在该键（首次运行 / 新增设置项）→ 需要初始化；
    # 2) 磁盘值与归一化结果不同，且该键属于「可能失效」的键
    #    （提示词预设 id 会因 prompts/*.txt 变化而失效，浮层枚举值同理）→ 需要修复。
    # 其它键（模型、设备、开关等）只按原值返回，写入一律留给 PUT /api/settings。
    repairable_keys = CLIPBOARD_OVERLAY_KEYS | {"default_preset_id", "last_used_preset_id"}
    initialization_updates = {
        key: value
        for key, value in normalized_payload.items()
        if key in repairable_keys and stored_settings.get(key) != value
    }
    if initialization_updates:
        logger.info(
            "初始化失效或缺失的用户设置项（GET /api/settings 单次落盘）",
            keys=sorted(initialization_updates),
        )
        user_settings.update(initialization_updates)

    return mask_doubao_secret(normalized_payload)


async def apply_settings_update(
    *,
    updates: dict,
    user_settings,
    asr_provider: Optional[ASRProvider],
    model_service_manager,
    close_streaming_manager,
) -> SettingsUpdateResult:
    """执行 PUT /api/settings 的全部编排（校验 → 落盘 → 模型服务/provider 重建）。

    Raises:
        SettingsValidationError: 校验失败（400）或 provider 重建失败（500）。
    """
    # 白名单与类型校验统一交给 UserSettingsUpdate：
    # - 未声明的键由 Pydantic 静默丢弃，保留既有宽松策略
    #   （Electron 桌面端保存设置时会连带提交整份 settings 快照）
    # - 类型 / 枚举 / 范围非法 → 400，并返回可读中文说明
    try:
        parsed_updates = UserSettingsUpdate.model_validate(updates)
    except ValidationError as error:
        validation_message = format_validation_error(error)
        logger.warning("设置项校验失败", error=validation_message)
        raise SettingsValidationError(validation_message) from error

    # 显式传入 null 视为「本次未提供」，避免把 None 写进 settings.json
    filtered = {
        key: value
        for key, value in parsed_updates.model_dump(mode="json", exclude_unset=True).items()
        if value is not None
    }

    if "hotkey" in filtered and not validate_hotkey(filtered["hotkey"]):
        raise SettingsValidationError("Invalid hotkey configuration")

    if "asr_provider" in filtered and filtered["asr_provider"] not in {
        "whisper",
        "qwen_asr",
        "doubao_asr",
    }:
        raise SettingsValidationError("Invalid ASR provider")

    # 豆包 Access Token：明文只在这里出现一次，落盘前换成密文
    _apply_doubao_secret_update(filtered)

    if not filtered:
        raise SettingsValidationError("No valid settings provided")

    merged_settings = {**user_settings.get_all(), **filtered}
    normalized_settings = normalize_clipboard_overlay_settings(
        normalize_preset_settings(merged_settings, user_settings=user_settings)
    )
    filtered = {key: normalized_settings[key] for key in filtered.keys()}
    previous_settings = user_settings.get_all()
    user_settings.update(filtered)

    changed_keys = {
        key for key, value in filtered.items()
        if previous_settings.get(key) != value
    }
    should_reset_model_runtime = bool(changed_keys & MODEL_RUNTIME_KEYS)
    should_rebuild_provider = bool(changed_keys & PROVIDER_REBUILD_KEYS)

    if should_reset_model_runtime:
        logger.info(
            "检测到模型运行时配置变更，准备卸载当前模型服务",
            changed_keys=sorted(changed_keys & MODEL_RUNTIME_KEYS),
        )
        await model_service_manager.stop()
        await close_streaming_manager()

    result = SettingsUpdateResult(payload={}, changed_keys=changed_keys)

    if should_rebuild_provider:
        if asr_provider is None:
            raise SettingsValidationError("ASR provider is not initialized", status_code=500)

        try:
            await asr_provider.close()
        except Exception as error:
            logger.warning("关闭旧 ASR provider 失败", error=str(error), exc_info=True)

        try:
            rebuilt_provider = create_asr_provider(get_asr_provider_config())
            model_service_manager.configure(rebuilt_provider)
        except Exception as error:
            logger.error("重建 ASR provider 失败", error=str(error), exc_info=True)
            raise SettingsValidationError(
                f"重新加载 ASR provider 失败: {error}",
                status_code=500,
            ) from error

        result.asr_provider = rebuilt_provider
        result.provider_rebuilt = True

    payload = normalize_preset_settings(user_settings.get_all(), user_settings=user_settings)
    result.payload = mask_doubao_secret(normalize_clipboard_overlay_settings(payload))
    return result
