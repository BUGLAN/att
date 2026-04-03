"""
Electron desktop agent.

负责在用户本机监听全局热键，并根据录音状态执行系统静音恢复。
该进程通过 stdio JSON-RPC 与 Electron 主进程通信，不依赖业务后端。
"""
from __future__ import annotations

import json
import platform
import sys
import threading
import time
from typing import Any

from pynput import keyboard

from audio_helper import AudioController


IS_MACOS = platform.system() == "Darwin"
IS_WINDOWS = platform.system() == "Windows"

MODIFIER_PRIORITY = {
    "cmd": 0,
    "win": 0,
    "ctrl": 1,
    "alt": 2,
    "shift": 3,
}

MODIFIER_LABELS = {
    "alt": "Alt",
    "ctrl": "Ctrl",
    "shift": "Shift",
    "win": "Win",
    "cmd": "Cmd",
}

MODIFIER_KEYS = {
    keyboard.Key.alt: "alt",
    keyboard.Key.alt_l: "alt",
    keyboard.Key.alt_r: "alt",
    keyboard.Key.ctrl: "ctrl",
    keyboard.Key.ctrl_l: "ctrl",
    keyboard.Key.ctrl_r: "ctrl",
    keyboard.Key.shift: "shift",
    keyboard.Key.shift_l: "shift",
    keyboard.Key.shift_r: "shift",
    keyboard.Key.cmd: "cmd" if IS_MACOS else "win",
    keyboard.Key.cmd_l: "cmd" if IS_MACOS else "win",
    keyboard.Key.cmd_r: "cmd" if IS_MACOS else "win",
}

PRESS_GUARD_MS = 250


def sort_modifiers(modifiers: list[str]) -> list[str]:
    return sorted(
        modifiers,
        key=lambda modifier: (MODIFIER_PRIORITY.get(modifier, 999), modifier),
    )


def build_hotkey_display(modifiers: list[str], key: str | None) -> str:
    parts = [MODIFIER_LABELS.get(modifier, modifier) for modifier in sort_modifiers(modifiers)]
    if key:
        parts.append(key.upper())
    return "+".join(parts)


def default_hotkey() -> dict[str, Any]:
    if IS_MACOS:
        return {
            "modifiers": ["cmd", "shift"],
            "key": "r",
            "display": "Cmd+Shift+R",
        }
    if IS_WINDOWS:
        return {
            "modifiers": ["win", "alt"],
            "key": None,
            "display": "Win+Alt",
        }
    return {
        "modifiers": ["ctrl", "shift"],
        "key": "r",
        "display": "Ctrl+Shift+R",
    }


def normalize_hotkey(hotkey: dict[str, Any] | None) -> dict[str, Any]:
    fallback = default_hotkey()
    if not isinstance(hotkey, dict):
        return fallback

    raw_modifiers = hotkey.get("modifiers", [])
    if not isinstance(raw_modifiers, list):
        return fallback

    modifiers = sort_modifiers(
        list(
            dict.fromkeys(
                str(modifier or "").strip().lower()
                for modifier in raw_modifiers
                if str(modifier or "").strip()
            )
        )
    )
    key = hotkey.get("key")
    if isinstance(key, str) and len(key) == 1 and key.isalnum():
        key = key.lower()
    else:
        key = None

    if not modifiers or (key is None and len(modifiers) < 2):
        return fallback

    return {
        "modifiers": modifiers,
        "key": key,
        "display": build_hotkey_display(modifiers, key),
    }


class DesktopMuteController:
    """录音期间系统输出静音控制器。"""

    def __init__(self) -> None:
        self.enabled = True
        self.recording_active = False
        self.original_muted: bool | None = None
        self.muted_by_controller = False
        self.audio_controller = AudioController()

    def update_policy(self, enabled: bool) -> dict[str, Any]:
        self.enabled = bool(enabled)
        if not self.enabled and self.recording_active:
            return self.set_recording_active(False)

        return {"success": True, "enabled": self.enabled}

    def set_recording_active(self, recording: bool) -> dict[str, Any]:
        if recording:
            return self._pause_for_recording()
        return self._resume_after_recording()

    def _pause_for_recording(self) -> dict[str, Any]:
        if not self.enabled:
            return {"success": True, "skipped": True, "reason": "disabled"}

        if self.recording_active:
            return {"success": True, "skipped": True, "reason": "already-recording"}

        if not self.audio_controller.supported:
            return {
                "success": False,
                "error": "Audio mute control is unavailable on this platform",
            }

        self.recording_active = True
        self.original_muted = bool(self.audio_controller.is_muted())

        if self.original_muted:
            self.muted_by_controller = False
            return {"success": True, "skipped": True, "muted": True}

        self.audio_controller.set_mute(True)
        self.muted_by_controller = True
        return {"success": True, "muted": True}

    def _resume_after_recording(self) -> dict[str, Any]:
        if not self.recording_active:
            return {"success": True, "skipped": True, "reason": "not-recording"}

        restore_muted = bool(self.original_muted)
        self.recording_active = False

        try:
          if not self.enabled and not self.muted_by_controller and not restore_muted:
              return {"success": True, "skipped": True, "reason": "disabled"}

          if self.muted_by_controller or restore_muted:
              self.audio_controller.set_mute(restore_muted)
              return {"success": True, "muted": restore_muted}

          return {"success": True, "skipped": True, "reason": "unchanged"}
        finally:
          self.original_muted = None
          self.muted_by_controller = False


class HotkeyMonitor:
    """基于 pynput 的全局热键监听器。"""

    def __init__(self, notifier) -> None:
        self.notifier = notifier
        self.hotkey = default_hotkey()
        self.listener: keyboard.Listener | None = None
        self.active_modifiers: set[str] = set()
        self.active_keys: set[str] = set()
        self.hotkey_latched = False
        self.last_press_timestamp = 0.0

    def start(self) -> None:
        if self.listener is not None:
            return

        try:
            self.listener = keyboard.Listener(
                on_press=self._on_press,
                on_release=self._on_release,
            )
            self.listener.start()
        except Exception as error:
            if IS_MACOS:
                self.notifier(
                    "permission.required",
                    {
                        "capability": "keyboard",
                        "message": "桌面热键监听需要系统辅助功能权限",
                        "error": str(error),
                    },
                )
                return
            raise

    def stop(self) -> None:
        if self.listener is None:
            return

        self.listener.stop()
        self.listener = None
        self.active_modifiers.clear()
        self.active_keys.clear()
        self.hotkey_latched = False

    def update_hotkey(self, hotkey: dict[str, Any]) -> dict[str, Any]:
        self.hotkey = normalize_hotkey(hotkey)
        self.active_modifiers.clear()
        self.active_keys.clear()
        self.hotkey_latched = False
        return self.hotkey

    def _on_press(self, key: keyboard.Key | keyboard.KeyCode) -> None:
        modifier = MODIFIER_KEYS.get(key)
        if modifier:
            self.active_modifiers.add(modifier)
        else:
            normalized_key = self._resolve_key_char(key)
            if normalized_key:
                self.active_keys.add(normalized_key)

        self._refresh_hotkey_state()

    def _on_release(self, key: keyboard.Key | keyboard.KeyCode) -> None:
        modifier = MODIFIER_KEYS.get(key)
        if modifier:
            self.active_modifiers.discard(modifier)
        else:
            normalized_key = self._resolve_key_char(key)
            if normalized_key:
                self.active_keys.discard(normalized_key)

        self._refresh_hotkey_state()

    def _resolve_key_char(self, key: keyboard.Key | keyboard.KeyCode) -> str | None:
        if isinstance(key, keyboard.KeyCode) and isinstance(key.char, str):
            normalized = key.char.lower()
            if len(normalized) == 1 and normalized.isalnum():
                return normalized
        return None

    def _matches_hotkey(self) -> bool:
        hotkey_modifiers = set(self.hotkey["modifiers"])
        if self.active_modifiers != hotkey_modifiers:
            return False

        if self.hotkey["key"] is None:
            return len(self.active_keys) == 0

        return self.active_keys == {self.hotkey["key"]}

    def _has_required_modifiers_only(self) -> bool:
        return self.active_modifiers == set(self.hotkey["modifiers"])

    def _refresh_hotkey_state(self) -> None:
        if self._matches_hotkey():
            now = time.monotonic() * 1000
            if not self.hotkey_latched and now - self.last_press_timestamp >= PRESS_GUARD_MS:
                self.hotkey_latched = True
                self.last_press_timestamp = now
                self.notifier("hotkey.press", {"hotkey": self.hotkey})
            return

        if self.hotkey["key"] is None and self.hotkey_latched and self._has_required_modifiers_only():
            return

        if self.hotkey_latched:
            self.hotkey_latched = False
            self.notifier("hotkey.release", {"hotkey": self.hotkey})


class DesktopAgent:
    """Desktop agent JSON-RPC 协议处理器。"""

    def __init__(self) -> None:
        self.shutdown_event = threading.Event()
        self.hotkey_monitor = HotkeyMonitor(self.notify)
        self.mute_controller = DesktopMuteController()

    def run(self) -> int:
        for raw_line in sys.stdin:
            line = raw_line.strip()
            if not line:
                continue

            try:
                payload = json.loads(line)
                self.handle_request(payload)
            except Exception as error:  # noqa: BLE001
                self.notify("agent.error", {"error": str(error)})

            if self.shutdown_event.is_set():
                break

        self.hotkey_monitor.stop()
        return 0

    def handle_request(self, payload: dict[str, Any]) -> None:
        method = payload.get("method")
        params = payload.get("params") or {}
        request_id = payload.get("id")

        try:
            if method == "agent.init":
                settings = params.get("settings") or {}
                hotkey = self.hotkey_monitor.update_hotkey(settings.get("hotkey"))
                mute_result = self.mute_controller.update_policy(
                    settings.get("pause_audio_on_recording", True)
                )
                self.hotkey_monitor.start()
                self.respond(
                    request_id,
                    {
                        "ready": True,
                        "hotkey": hotkey,
                        "mute": mute_result,
                    },
                )
                self.notify("agent.ready", {"hotkey": hotkey})
                return

            if method == "hotkey.update":
                hotkey = self.hotkey_monitor.update_hotkey(params.get("hotkey"))
                self.respond(request_id, {"success": True, "hotkey": hotkey})
                return

            if method == "mute.set_policy":
                result = self.mute_controller.update_policy(
                    params.get("pause_audio_on_recording", True)
                )
                self.respond(request_id, result)
                return

            if method == "recording.set_active":
                result = self.mute_controller.set_recording_active(
                    params.get("recording") is True
                )
                self.respond(request_id, result)
                return

            if method == "agent.shutdown":
                self.respond(request_id, {"success": True})
                self.shutdown_event.set()
                return

            self.respond_error(request_id, f"Unknown method: {method}")
        except Exception as error:  # noqa: BLE001
            self.respond_error(request_id, str(error))

    def respond(self, request_id: Any, result: dict[str, Any]) -> None:
        self.write_message(
            {
                "jsonrpc": "2.0",
                "id": request_id,
                "result": result,
            }
        )

    def respond_error(self, request_id: Any, message: str) -> None:
        self.write_message(
            {
                "jsonrpc": "2.0",
                "id": request_id,
                "error": {
                    "message": message,
                },
            }
        )

    def notify(self, method: str, params: dict[str, Any]) -> None:
        self.write_message(
            {
                "jsonrpc": "2.0",
                "method": method,
                "params": params,
            }
        )

    def write_message(self, payload: dict[str, Any]) -> None:
        sys.stdout.write(json.dumps(payload, ensure_ascii=False) + "\n")
        sys.stdout.flush()


def main() -> int:
    agent = DesktopAgent()
    return agent.run()


if __name__ == "__main__":
    raise SystemExit(main())
