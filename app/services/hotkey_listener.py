"""
Backend input listener service.

Provides:
- keyboard press/hold hotkey for recording start/stop
- global mouse trigger for the clipboard overlay
"""
import asyncio
import platform
import time
from typing import Dict, List, Optional, Set

from app.config import (
    VALID_CLIPBOARD_TRIGGER_MODES,
    get_default_clipboard_overlay_settings,
    get_default_hotkey,
    is_valid_hotkey,
    user_settings,
)
from app.services.websocket import manager as ws_manager
from app.utils.logging import get_logger

logger = get_logger(__name__)

IS_MACOS = platform.system() == "Darwin"
MODIFIER_KEY_MAP: Dict[str, List] = {}
LEFT_RIGHT_CHORD_WINDOW_SECONDS = 0.15


def _init_modifier_key_map() -> None:
    """Initialize modifier aliases with pynput key variants."""
    try:
        from pynput import keyboard
    except ImportError:
        logger.warning("pynput is unavailable; backend hotkey listener disabled")
        return

    global MODIFIER_KEY_MAP
    MODIFIER_KEY_MAP = {
        "cmd": [keyboard.Key.cmd, keyboard.Key.cmd_r],
        "ctrl": [keyboard.Key.ctrl_l, keyboard.Key.ctrl_r, keyboard.Key.ctrl],
        "alt": [keyboard.Key.alt_l, keyboard.Key.alt_r, keyboard.Key.alt],
        "shift": [keyboard.Key.shift_l, keyboard.Key.shift_r, keyboard.Key.shift],
        "win": [keyboard.Key.cmd, keyboard.Key.cmd_r],
    }


_init_modifier_key_map()


def _build_hotkey_display(modifiers: List[str], key: Optional[str]) -> str:
    """Build a readable display string for persisted settings."""
    if IS_MACOS:
        token_map = {
            "cmd": "Cmd",
            "ctrl": "Ctrl",
            "alt": "Alt",
            "shift": "Shift",
            "win": "Cmd",
        }
        return "".join([token_map.get(modifier, modifier) for modifier in modifiers]) + (key.upper() if key else "")

    token_map = {
        "cmd": "Cmd",
        "ctrl": "Ctrl",
        "alt": "Alt",
        "shift": "Shift",
        "win": "Win",
    }
    parts = [token_map.get(modifier, modifier) for modifier in modifiers]
    if key:
        parts.append(key.upper())
    return "+".join(parts)


def _schedule_callback(loop: Optional[asyncio.AbstractEventLoop], callback, event_name: str) -> None:
    if not callback:
        return

    try:
        result = callback()
        if asyncio.iscoroutine(result):
            if loop:
                future = asyncio.run_coroutine_threadsafe(result, loop)

                def _raise_callback_error(done_future):
                    try:
                        done_future.result()
                    except Exception as error:
                        logger.error(
                            f"Error in {event_name} callback coroutine",
                            error=str(error),
                        )

                future.add_done_callback(_raise_callback_error)
            else:
                asyncio.create_task(result)
    except Exception as error:
        logger.error(f"Error in {event_name} callback", error=str(error))


def normalize_hotkey(hotkey: Optional[dict]) -> dict:
    """Normalize hotkey payload for runtime usage."""
    if not is_valid_hotkey(hotkey):
        hotkey = get_default_hotkey()

    modifiers = list(hotkey.get("modifiers", []))
    key = hotkey.get("key")
    normalized_key = key.lower() if isinstance(key, str) and key else None

    return {
        "modifiers": modifiers,
        "key": normalized_key,
        "display": hotkey.get("display") or _build_hotkey_display(modifiers, normalized_key),
    }


def normalize_clipboard_trigger_mode(trigger_mode: Optional[str]) -> str:
    """Normalize clipboard overlay trigger mode for runtime usage."""
    default_mode = get_default_clipboard_overlay_settings()["clipboard_overlay_trigger"]
    return trigger_mode if trigger_mode in VALID_CLIPBOARD_TRIGGER_MODES else default_mode


class PynputHotkeyListener:
    """Global hotkey listener with press-and-hold behavior."""

    def __init__(
        self,
        on_press=None,
        on_release=None,
        loop: Optional[asyncio.AbstractEventLoop] = None,
        hotkey_config: Optional[Dict] = None,
    ):
        self.on_press = on_press
        self.on_release = on_release
        self.loop = loop
        self._hotkey_config = normalize_hotkey(hotkey_config)

        self.is_pressed = False
        self.is_running = False
        self._listener = None
        self._key_states = {}

    def set_hotkey(self, hotkey_config: Optional[Dict]) -> None:
        self._hotkey_config = normalize_hotkey(hotkey_config)
        logger.info("Hotkey updated", hotkey=self._hotkey_config)

    def start(self) -> bool:
        if self.is_running:
            return True

        try:
            from pynput import keyboard
        except ImportError:
            logger.error("pynput package required: pip install pynput")
            return False

        try:
            self._listener = keyboard.Listener(
                on_press=self._on_key_press,
                on_release=self._on_key_release,
            )
            self._listener.start()
            self.is_running = True
            logger.info("Backend hotkey listener started", hotkey=self._hotkey_config)
            return True
        except Exception as error:
            logger.error("Failed to start backend hotkey listener", error=str(error))
            return False

    def stop(self) -> None:
        if self._listener:
            self._listener.stop()
            self._listener = None

        self.is_running = False
        self.is_pressed = False
        self._key_states.clear()
        logger.info("Backend hotkey listener stopped")

    def _check_hotkey(self) -> bool:
        from pynput import keyboard

        modifiers = self._hotkey_config.get("modifiers", [])
        key = self._hotkey_config.get("key")

        for modifier in modifiers:
            mod_keys = MODIFIER_KEY_MAP.get(modifier, [])
            if not mod_keys or not any(self._key_states.get(mod_key, False) for mod_key in mod_keys):
                return False

        if key:
            try:
                key_code = keyboard.KeyCode.from_char(key)
            except Exception:
                return False
            if not self._key_states.get(key_code, False):
                return False

        return bool(modifiers)

    def _on_key_press(self, key) -> None:
        self._key_states[key] = True

        if self._check_hotkey() and not self.is_pressed:
            self.is_pressed = True
            logger.info("Hotkey pressed", hotkey=self._hotkey_config)
            _schedule_callback(self.loop, self.on_press, "hotkey press")

    def _on_key_release(self, key) -> None:
        self._key_states[key] = False

        if self.is_pressed and not self._check_hotkey():
            self.is_pressed = False
            logger.info("Hotkey released", hotkey=self._hotkey_config)
            _schedule_callback(self.loop, self.on_release, "hotkey release")


class ClipboardMouseTriggerListener:
    """Global mouse trigger listener for the clipboard overlay."""

    def __init__(self, on_trigger=None, loop: Optional[asyncio.AbstractEventLoop] = None, trigger_mode: Optional[str] = None):
        self.on_trigger = on_trigger
        self.loop = loop
        self.trigger_mode = normalize_clipboard_trigger_mode(trigger_mode)
        self.is_running = False
        self._listener = None
        self._pressed_buttons: Set[str] = set()
        self._press_times: Dict[str, float] = {}
        self._triggered = False

    def set_trigger_mode(self, trigger_mode: Optional[str]) -> None:
        self.trigger_mode = normalize_clipboard_trigger_mode(trigger_mode)
        self._pressed_buttons.clear()
        self._press_times.clear()
        self._triggered = False
        logger.info("Clipboard trigger mode updated", trigger_mode=self.trigger_mode)

    def start(self) -> bool:
        if self.is_running:
            return True

        try:
            from pynput import mouse
        except ImportError:
            logger.error("pynput package required: pip install pynput")
            return False

        try:
            self._listener = mouse.Listener(on_click=self._on_click)
            self._listener.start()
            self.is_running = True
            logger.info("Clipboard mouse trigger listener started", trigger_mode=self.trigger_mode)
            return True
        except Exception as error:
            logger.error("Failed to start clipboard mouse trigger listener", error=str(error))
            return False

    def stop(self) -> None:
        if self._listener:
            self._listener.stop()
            self._listener = None

        self.is_running = False
        self._pressed_buttons.clear()
        self._press_times.clear()
        self._triggered = False
        logger.info("Clipboard mouse trigger listener stopped")

    def _button_name(self, button) -> Optional[str]:
        try:
            button_name = button.name
        except Exception:
            button_name = str(button).split(".")[-1]

        if button_name in {"left", "right", "middle"}:
            return button_name

        return None

    def _should_trigger_left_right_chord(self) -> bool:
        if self.trigger_mode != "mouse_left_right_chord" or self._triggered:
            return False

        if not {"left", "right"}.issubset(self._pressed_buttons):
            return False

        left_time = self._press_times.get("left")
        right_time = self._press_times.get("right")
        if left_time is None or right_time is None:
            return False

        return abs(left_time - right_time) <= LEFT_RIGHT_CHORD_WINDOW_SECONDS

    def _on_click(self, x, y, button, pressed) -> None:
        button_name = self._button_name(button)
        if not button_name:
            return

        if pressed:
            self._pressed_buttons.add(button_name)
            self._press_times[button_name] = time.monotonic()

            if self.trigger_mode == "mouse_middle" and button_name == "middle" and not self._triggered:
                self._triggered = True
                logger.info("Clipboard overlay trigger fired", trigger_mode=self.trigger_mode)
                _schedule_callback(self.loop, self.on_trigger, "clipboard trigger")
                return

            if self._should_trigger_left_right_chord():
                self._triggered = True
                logger.info("Clipboard overlay trigger fired", trigger_mode=self.trigger_mode)
                _schedule_callback(self.loop, self.on_trigger, "clipboard trigger")
            return

        self._pressed_buttons.discard(button_name)

        if self.trigger_mode == "mouse_middle" and button_name == "middle":
            self._triggered = False
        elif self.trigger_mode == "mouse_left_right_chord" and button_name in {"left", "right"}:
            if not {"left", "right"}.issubset(self._pressed_buttons):
                self._triggered = False


class BackendHotkeyService:
    """Coordinates backend keyboard and mouse listeners with websocket broadcasts."""

    def __init__(self, loop: asyncio.AbstractEventLoop):
        self.loop = loop
        self.listener = PynputHotkeyListener(
            on_press=self._broadcast_start,
            on_release=self._broadcast_stop,
            loop=loop,
            hotkey_config=user_settings.get("hotkey"),
        )
        self.clipboard_trigger_listener = ClipboardMouseTriggerListener(
            on_trigger=self._broadcast_clipboard_overlay_trigger,
            loop=loop,
            trigger_mode=user_settings.get("clipboard_overlay_trigger"),
        )

    async def _broadcast_start(self) -> None:
        await ws_manager.broadcast({
            "type": "recording_command",
            "action": "start",
            "source": "backend_hotkey",
        }, client_type_filter="web")

    async def _broadcast_stop(self) -> None:
        await ws_manager.broadcast({
            "type": "recording_command",
            "action": "stop",
            "source": "backend_hotkey",
        }, client_type_filter="web")

    async def _broadcast_clipboard_overlay_trigger(self) -> None:
        await ws_manager.broadcast({
            "type": "clipboard_overlay_command",
            "action": "trigger",
            "source": "backend_mouse_trigger",
        }, client_type_filter="web")

    def start(self) -> bool:
        hotkey_started = self.listener.start()
        clipboard_trigger_started = self.clipboard_trigger_listener.start()
        return hotkey_started or clipboard_trigger_started

    def stop(self) -> None:
        self.listener.stop()
        self.clipboard_trigger_listener.stop()

    def update_hotkey(self, hotkey: Optional[dict]) -> dict:
        normalized = normalize_hotkey(hotkey)
        self.listener.set_hotkey(normalized)
        return {
            "success": True,
            "hotkey": normalized,
        }

    def update_clipboard_trigger_mode(self, trigger_mode: Optional[str]) -> dict:
        normalized_mode = normalize_clipboard_trigger_mode(trigger_mode)
        self.clipboard_trigger_listener.set_trigger_mode(normalized_mode)
        return {
            "success": True,
            "trigger_mode": normalized_mode,
        }


_hotkey_service: Optional[BackendHotkeyService] = None


def init_hotkey_service(loop: asyncio.AbstractEventLoop) -> BackendHotkeyService:
    """Create and cache the backend hotkey service."""
    global _hotkey_service
    if _hotkey_service is None:
        _hotkey_service = BackendHotkeyService(loop)
    return _hotkey_service


def get_hotkey_service() -> Optional[BackendHotkeyService]:
    """Return the active backend hotkey service."""
    return _hotkey_service
