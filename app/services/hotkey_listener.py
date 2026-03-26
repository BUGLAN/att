"""
Backend global hotkey listener service.

Reuses the proven pynput-based press/hold semantics from the main branch:
- hotkey press -> broadcast recording start
- hotkey release -> broadcast recording stop
"""
import asyncio
import platform
from typing import Dict, List, Optional

from app.config import get_default_hotkey, is_valid_hotkey, user_settings
from app.services.websocket import manager as ws_manager
from app.utils.logging import get_logger

logger = get_logger(__name__)

IS_MACOS = platform.system() == "Darwin"
IS_WINDOWS = platform.system() == "Windows"
MODIFIER_KEY_MAP: Dict[str, List] = {}


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

    def _schedule_callback(self, callback, event_name: str) -> None:
        if not callback:
            return

        try:
            result = callback()
            if asyncio.iscoroutine(result):
                if self.loop:
                    future = asyncio.run_coroutine_threadsafe(result, self.loop)

                    def _raise_callback_error(done_future):
                        try:
                            done_future.result()
                        except Exception as error:
                            logger.error(
                                f"Error in hotkey {event_name} callback coroutine",
                                error=str(error),
                            )

                    future.add_done_callback(_raise_callback_error)
                else:
                    asyncio.create_task(result)
        except Exception as error:
            logger.error(f"Error in hotkey {event_name} callback", error=str(error))

    def _on_key_press(self, key) -> None:
        self._key_states[key] = True

        if self._check_hotkey() and not self.is_pressed:
            self.is_pressed = True
            logger.info("Hotkey pressed", hotkey=self._hotkey_config)
            self._schedule_callback(self.on_press, "press")

    def _on_key_release(self, key) -> None:
        self._key_states[key] = False

        if self.is_pressed and not self._check_hotkey():
            self.is_pressed = False
            logger.info("Hotkey released", hotkey=self._hotkey_config)
            self._schedule_callback(self.on_release, "release")


class BackendHotkeyService:
    """Coordinates backend hotkey listening and websocket broadcasts."""

    def __init__(self, loop: asyncio.AbstractEventLoop):
        self.loop = loop
        self.listener = PynputHotkeyListener(
            on_press=self._broadcast_start,
            on_release=self._broadcast_stop,
            loop=loop,
            hotkey_config=user_settings.get("hotkey"),
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

    def start(self) -> bool:
        return self.listener.start()

    def stop(self) -> None:
        self.listener.stop()

    def update_hotkey(self, hotkey: Optional[dict]) -> dict:
        normalized = normalize_hotkey(hotkey)
        self.listener.set_hotkey(normalized)
        return {
            "success": True,
            "hotkey": normalized,
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
