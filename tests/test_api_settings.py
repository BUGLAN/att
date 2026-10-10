"""settings API 回归测试。

覆盖审计报告 §3.2 第 4/5 条：
- PUT /api/settings 必须做类型/枚举校验，非法取值返回 400 且给出中文说明；
- 未知键继续静默丢弃（Electron 保存设置时会连带提交整份 settings 快照）；
- GET /api/settings 不再是无条件写接口：只有确实需要初始化/修复时才落盘一次。
测试使用隔离的临时 settings.json，不触碰真实 data/settings.json。
"""
import json

import pytest
from fastapi import FastAPI
from fastapi.testclient import TestClient

from app.api import routes
from app.api.schemas import UserSettingsUpdate
from app.config import RETIRED_SETTINGS_KEYS, UserSettings


@pytest.fixture
def settings_env(monkeypatch, temp_dir):
    """把路由层依赖的 user_settings 换成隔离目录下的真实实例。"""
    isolated_settings = UserSettings(temp_dir / "settings.json")
    monkeypatch.setattr(routes, "user_settings", isolated_settings)
    # 避免跨测试复用流式会话管理器
    monkeypatch.setattr(routes, "streaming_session_manager", None)

    app = FastAPI()
    app.include_router(routes.router)
    return TestClient(app), isolated_settings


def test_update_model_covers_all_persisted_setting_keys():
    """契约护栏：UserSettings 的全部持久化键都必须被 UserSettingsUpdate 声明。

    否则 PUT /api/settings 会把它们当作未知键静默丢弃，Electron 同步的设置会失效。
    """
    missing_keys = set(UserSettings.DEFAULT_SETTINGS) - set(UserSettingsUpdate.model_fields)

    assert missing_keys == set()
    # 该键不在 DEFAULT_SETTINGS 中，但前端「麦克风设备」下拉依赖它
    assert "audio_input_device" in UserSettingsUpdate.model_fields
    # 已下线功能的僵尸键必须同时从默认值与 PUT 白名单里消失（否则旧渲染进程整份提交会把它写回）
    assert RETIRED_SETTINGS_KEYS.isdisjoint(UserSettings.DEFAULT_SETTINGS)
    assert RETIRED_SETTINGS_KEYS.isdisjoint(UserSettingsUpdate.model_fields)


def test_put_valid_key_applied_and_persisted(settings_env):
    """合法键仍能写入并持久化（Electron 的 pause_audio_on_recording 同步依赖此路径）。"""
    client, user_settings = settings_env

    response = client.put(
        "/api/settings",
        json={"pause_audio_on_recording": False, "auto_optimize": True},
    )

    assert response.status_code == 200
    assert response.json()["pause_audio_on_recording"] is False
    assert response.json()["auto_optimize"] is True
    assert user_settings.get("pause_audio_on_recording") is False
    assert user_settings.get("auto_optimize") is True

    persisted = json.loads(user_settings.settings_path.read_text(encoding="utf-8"))
    assert persisted["pause_audio_on_recording"] is False
    assert persisted["auto_optimize"] is True


def test_put_valid_hotkey_still_accepted(settings_env):
    """合法热键配置必须继续被接受（桌面端同步契约）。"""
    client, user_settings = settings_env
    hotkey = {"modifiers": ["win", "alt"], "key": None, "display": "Win+Alt"}

    response = client.put("/api/settings", json={"hotkey": hotkey})

    assert response.status_code == 200
    assert user_settings.get("hotkey") == hotkey


def test_put_unknown_keys_silently_dropped(settings_env):
    """未知键静默丢弃，合法键照常生效（保留既有宽松策略）。"""
    client, user_settings = settings_env

    response = client.put(
        "/api/settings",
        json={"pause_audio_on_recording": False, "totally_unknown_key": "x", "another": 1},
    )

    assert response.status_code == 200
    assert response.json()["pause_audio_on_recording"] is False
    assert "totally_unknown_key" not in response.json()
    assert user_settings.get("totally_unknown_key") is None


def test_put_only_unknown_keys_returns_400(settings_env):
    """全部是未知键时维持原行为：400 No valid settings provided。"""
    client, _ = settings_env

    response = client.put("/api/settings", json={"totally_unknown_key": "x"})

    assert response.status_code == 400
    assert response.json()["detail"] == "No valid settings provided"


def test_put_invalid_int_type_rejected(settings_env):
    """qwen_asr_max_new_tokens 传字符串必须先被拒，而不是落盘后在重建 provider 时 int() 抛错。"""
    client, user_settings = settings_env

    response = client.put("/api/settings", json={"qwen_asr_max_new_tokens": "abc"})

    assert response.status_code == 400
    detail = response.json()["detail"]
    assert "设置项校验失败" in detail
    assert "qwen_asr_max_new_tokens" in detail
    assert "整数" in detail
    # 非法值绝不能落盘
    assert user_settings.get("qwen_asr_max_new_tokens") == 256


def test_put_invalid_bool_type_rejected(settings_env):
    """布尔开关传非布尔值被拒。"""
    client, _ = settings_env

    response = client.put("/api/settings", json={"auto_optimize": "maybe"})

    assert response.status_code == 400
    assert "auto_optimize" in response.json()["detail"]


def test_put_invalid_asr_provider_enum_rejected(settings_env):
    """asr_provider 走枚举校验，非法值返回 400 与中文说明。"""
    client, user_settings = settings_env

    response = client.put("/api/settings", json={"asr_provider": "not_a_provider"})

    assert response.status_code == 400
    detail = response.json()["detail"]
    assert "asr_provider" in detail
    assert "枚举" in detail
    assert user_settings.get("asr_provider") == "whisper"


def test_put_invalid_number_type_rejected(settings_env):
    """数值型设置传非数值被拒。"""
    client, _ = settings_env

    response = client.put("/api/settings", json={"silence_threshold": "很久"})

    assert response.status_code == 400
    assert "silence_threshold" in response.json()["detail"]


def test_put_invalid_hotkey_shape_rejected(settings_env):
    """hotkey 传非对象被拒（原实现只校验 hotkey 的形状，这里同时覆盖类型层）。"""
    client, _ = settings_env

    response = client.put("/api/settings", json={"hotkey": 12345})

    assert response.status_code == 400
    assert "hotkey" in response.json()["detail"]


def test_put_null_value_not_written(settings_env):
    """显式传 null 视为「未提供」，不会被当成一次有效更新。"""
    client, user_settings = settings_env

    response = client.put("/api/settings", json={"audio_input_device": None})

    assert response.status_code == 400
    assert response.json()["detail"] == "No valid settings provided"
    assert user_settings.get("audio_input_device") is None


def test_get_settings_has_no_write_side_effect_when_clean(settings_env):
    """GET /api/settings 在数据已归一化后不得再写盘。"""
    client, user_settings = settings_env

    # 预热：允许第一次 GET 做一次必要的初始化落盘
    client.get("/api/settings")
    before_bytes = user_settings.settings_path.read_bytes()

    writes = []
    original_update = user_settings.update

    def counting_update(updates):
        writes.append(dict(updates))
        return original_update(updates)

    user_settings.update = counting_update
    try:
        response = client.get("/api/settings")
    finally:
        user_settings.update = original_update

    assert response.status_code == 200
    assert writes == []
    assert user_settings.settings_path.read_bytes() == before_bytes


def test_get_settings_repairs_stale_preset_once(settings_env):
    """失效的预设 id 会被归一化修正，且只落盘一次，第二次 GET 不再写。"""
    client, user_settings = settings_env
    user_settings.set("default_preset_id", "preset-does-not-exist")
    user_settings.set("last_used_preset_id", "preset-does-not-exist")

    writes = []
    original_update = user_settings.update

    def counting_update(updates):
        writes.append(dict(updates))
        return original_update(updates)

    user_settings.update = counting_update
    try:
        first = client.get("/api/settings")
        writes_after_first = len(writes)
        second = client.get("/api/settings")
        writes_after_second = len(writes)
    finally:
        user_settings.update = original_update

    assert first.status_code == 200
    assert second.status_code == 200
    # 归一化结果不能把失效 id 直接透出
    assert first.json()["default_preset_id"] != "preset-does-not-exist"
    # 最多一次落盘（prompts 目录为空、没有可回落预设时可能无需修复）
    assert writes_after_first <= 1
    assert writes_after_second == writes_after_first


def test_get_settings_repairs_all_stale_values_in_single_write(settings_env):
    """失效的预设 id 与非法浮层枚举同时存在时，只允许一次落盘（旧实现会分两次写）。"""
    client, user_settings = settings_env
    user_settings.set("default_preset_id", "preset-does-not-exist")
    user_settings.set("last_used_preset_id", "preset-does-not-exist")
    # 用仍在用的浮层枚举键制造非法值：clipboard_overlay_trigger 已彻底下线，不再可写
    user_settings.set("clipboard_overlay_retrigger_action", "not_a_valid_action")

    writes = []
    original_update = user_settings.update

    def counting_update(updates):
        writes.append(dict(updates))
        return original_update(updates)

    user_settings.update = counting_update
    try:
        response = client.get("/api/settings")
        writes_first = len(writes)
        writes_payload = list(writes)
        client.get("/api/settings")
        writes_second = len(writes)
    finally:
        user_settings.update = original_update

    assert response.status_code == 200
    # 一次写盘修复全部失效值
    assert writes_first == 1
    assert {"default_preset_id", "last_used_preset_id", "clipboard_overlay_retrigger_action"} <= set(writes_payload[0])
    # 修复落盘绝不能把已下线的僵尸键带回去
    assert RETIRED_SETTINGS_KEYS.isdisjoint(writes_payload[0])
    # 归一化后再次 GET 必须是纯读
    assert writes_second == writes_first


def test_get_settings_keeps_response_shape(settings_env):
    """GET 返回的字段集合不能变化（前端契约）。"""
    client, _ = settings_env

    response = client.get("/api/settings")

    assert response.status_code == 200
    payload = response.json()
    for key in (
        "asr_provider",
        "whisper_model",
        "qwen_asr_max_new_tokens",
        "pause_audio_on_recording",
        "auto_optimize",
        "auto_copy_to_clipboard",
        "hotkey",
        "clipboard_overlay_retrigger_action",
        "clipboard_overlay_close_on_click_copy",
        "recording_mode",
        "silence_threshold",
        "silence_level",
        "default_preset_id",
        "last_used_preset_id",
    ):
        assert key in payload

    # 已下线功能的僵尸键不得再出现在响应里
    assert RETIRED_SETTINGS_KEYS.isdisjoint(payload)


def test_retired_keys_are_silently_dropped_by_update_model():
    """退役键必须走 extra="ignore" 静默丢弃：旧渲染进程整份提交也不会被写回。"""
    assert UserSettingsUpdate.model_config["extra"] == "ignore"

    parsed = UserSettingsUpdate.model_validate({
        "auto_optimize": True,
        "clipboard_overlay_trigger": "mouse_middle",
        "selection_toolbar_enabled": True,
        "selection_toolbar_trigger_delay_ms": 500,
    })
    dumped = parsed.model_dump(exclude_unset=True)

    assert dumped["auto_optimize"] is True
    assert RETIRED_SETTINGS_KEYS.isdisjoint(dumped)


def test_legacy_settings_file_with_retired_keys_is_tolerated(monkeypatch, temp_dir):
    """含已下线僵尸键的旧 settings.json：后端照常启动，键被静默丢弃且不写回。"""
    legacy_path = temp_dir / "settings.json"
    legacy_path.write_text(
        json.dumps(
            {
                "auto_optimize": True,
                # 仍在用的浮层键保留，并故意放一个非法枚举值验证归一化没被改坏
                "clipboard_overlay_retrigger_action": "not_a_valid_action",
                "clipboard_overlay_close_on_click_copy": False,
                # 6 个已下线的僵尸键
                "clipboard_overlay_trigger": "not_a_valid_trigger",
                "selection_toolbar_enabled": False,
                "selection_toolbar_auto_add_to_wordbook": False,
                "selection_toolbar_show_examples": False,
                "selection_toolbar_trigger_delay_ms": 300,
                "selection_toolbar_fallback_to_clipboard": False,
            },
            ensure_ascii=False,
        ),
        encoding="utf-8",
    )

    isolated_settings = UserSettings(legacy_path)
    monkeypatch.setattr(routes, "user_settings", isolated_settings)
    monkeypatch.setattr(routes, "streaming_session_manager", None)

    app = FastAPI()
    app.include_router(routes.router)
    client = TestClient(app)

    response = client.get("/api/settings")

    assert response.status_code == 200
    payload = response.json()
    for key in RETIRED_SETTINGS_KEYS:
        assert key not in payload
    # 仍在使用的键必须保持原有归一化行为
    assert payload["auto_optimize"] is True
    assert payload["clipboard_overlay_retrigger_action"] == "toggle"
    assert payload["clipboard_overlay_close_on_click_copy"] is False

    # 任何一次正常写入都不得把僵尸键带回磁盘
    put_response = client.put("/api/settings", json={"auto_optimize": False})
    assert put_response.status_code == 200
    assert RETIRED_SETTINGS_KEYS.isdisjoint(put_response.json())

    persisted = json.loads(legacy_path.read_text(encoding="utf-8"))
    for key in RETIRED_SETTINGS_KEYS:
        assert key not in persisted
