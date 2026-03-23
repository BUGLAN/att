"""
Global Hotkey Listener for Tray Tool
Supports macOS (Cmd+Shift+R) and Windows (Ctrl+Shift+R)
"""
import platform
import asyncio
from typing import Callable, Optional
import logging

logger = logging.getLogger(__name__)

# Platform detection
IS_MACOS = platform.system() == "Darwin"
IS_WINDOWS = platform.system() == "Windows"


class HotkeyListener:
    """
    Global hotkey listener for recording control.

    Default hotkeys:
    - macOS: Cmd + Shift + R (press and hold to record)
    - Windows: Ctrl + Shift + R

    Usage:
        listener = HotkeyListener(
            on_press=lambda: print("Recording started"),
            on_release=lambda: print("Recording stopped")
        )
        listener.start()
    """

    def __init__(
        self,
        on_press: Optional[Callable] = None,
        on_release: Optional[Callable] = None,
        hotkey: Optional[str] = None
    ):
        """
        Initialize hotkey listener.

        Args:
            on_press: Callback when hotkey is pressed
            on_release: Callback when hotkey is released
            hotkey: Custom hotkey (default: Cmd+Shift+R on macOS, Ctrl+Shift+R on Windows)
        """
        self.on_press = on_press
        self.on_release = on_release
        self.hotkey = hotkey or self._get_default_hotkey()

        # State
        self.is_pressed = False
        self.is_running = False
        self._listener = None

        logger.info(f"Hotkey configured: {self.hotkey}")

    def _get_default_hotkey(self) -> str:
        """Get default hotkey based on platform."""
        if IS_MACOS:
            return "<cmd>+<shift>+r"
        elif IS_WINDOWS:
            return "<ctrl>+<shift>+r"
        else:
            return "<ctrl>+<shift>+r"

    def start(self) -> bool:
        """
        Start listening for hotkey.

        Returns:
            True if started successfully
        """
        if self.is_running:
            logger.warning("Hotkey listener already running")
            return True

        try:
            import keyboard
        except ImportError:
            logger.error("keyboard package required: pip install keyboard")
            return False

        try:
            # Add hotkey handlers
            keyboard.add_hotkey(
                self.hotkey,
                self._on_hotkey_press,
                trigger_on_release=False
            )

            # For press-and-hold behavior, we need to track key state
            keyboard.add_hotkey(
                self.hotkey,
                self._on_hotkey_release,
                trigger_on_release=True
            )

            self.is_running = True
            logger.info("Hotkey listener started")
            return True

        except Exception as e:
            logger.error(f"Failed to start hotkey listener: {e}")
            return False

    def stop(self) -> None:
        """Stop listening for hotkey."""
        if not self.is_running:
            return

        try:
            import keyboard
            keyboard.remove_hotkey(self.hotkey)
        except Exception as e:
            logger.error(f"Failed to stop hotkey listener: {e}")

        self.is_running = False
        logger.info("Hotkey listener stopped")

    def _on_hotkey_press(self) -> None:
        """Handle hotkey press."""
        if self.is_pressed:
            return

        self.is_pressed = True
        logger.info(f"Hotkey pressed: {self.hotkey}")

        if self.on_press:
            try:
                result = self.on_press()
                # Handle async callbacks
                if asyncio.iscoroutine(result):
                    asyncio.create_task(result)
            except Exception as e:
                logger.error(f"Error in on_press callback: {e}")

    def _on_hotkey_release(self) -> None:
        """Handle hotkey release."""
        if not self.is_pressed:
            return

        self.is_pressed = False
        logger.info(f"Hotkey released: {self.hotkey}")

        if self.on_release:
            try:
                result = self.on_release()
                if asyncio.iscoroutine(result):
                    asyncio.create_task(result)
            except Exception as e:
                logger.error(f"Error in on_release callback: {e}")


class PynputHotkeyListener:
    """
    Alternative hotkey listener using pynput.
    More reliable for global hotkeys on macOS.

    This is recommended for macOS due to keyboard library limitations.
    """

    def __init__(
        self,
        on_press: Optional[Callable] = None,
        on_release: Optional[Callable] = None,
        loop: Optional[asyncio.AbstractEventLoop] = None
    ):
        """
        Initialize pynput hotkey listener.

        Args:
            on_press: Callback when hotkey is pressed
            on_release: Callback when hotkey is released
            loop: Event loop for async callbacks
        """
        self.on_press = on_press
        self.on_release = on_release
        self.loop = loop

        # State
        self.is_pressed = False
        self.is_running = False
        self._listener = None
        self._key_states = {}

    def set_loop(self, loop: asyncio.AbstractEventLoop) -> None:
        """Set the event loop for async callbacks."""
        self.loop = loop

    def start(self) -> bool:
        """Start listening for hotkey."""
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
                on_release=self._on_key_release
            )
            self._listener.start()
            self.is_running = True
            logger.info("Pynput hotkey listener started")
            return True

        except Exception as e:
            logger.error(f"Failed to start pynput listener: {e}")
            return False

    def stop(self) -> None:
        """Stop listening."""
        if self._listener:
            self._listener.stop()
            self._listener = None

        self.is_running = False
        self._key_states.clear()
        logger.info("Pynput hotkey listener stopped")

    def _check_hotkey(self) -> bool:
        """Check if the hotkey combination is pressed."""
        from pynput import keyboard

        if IS_MACOS:
            # Cmd + Shift + R
            return (
                self._key_states.get(keyboard.Key.cmd, False) or
                self._key_states.get(keyboard.Key.cmd_r, False)
            ) and (
                self._key_states.get(keyboard.Key.shift, False) or
                self._key_states.get(keyboard.Key.shift_r, False)
            ) and self._key_states.get(keyboard.KeyCode.from_char('r'), False)

        else:
            # Ctrl + Shift + R
            return (
                self._key_states.get(keyboard.Key.ctrl, False) or
                self._key_states.get(keyboard.Key.ctrl_r, False)
            ) and (
                self._key_states.get(keyboard.Key.shift, False) or
                self._key_states.get(keyboard.Key.shift_r, False)
            ) and self._key_states.get(keyboard.KeyCode.from_char('r'), False)

    def _on_key_press(self, key) -> None:
        """Handle key press."""
        self._key_states[key] = True

        if self._check_hotkey() and not self.is_pressed:
            self.is_pressed = True
            logger.info("Hotkey pressed")

            if self.on_press:
                try:
                    result = self.on_press()
                    if asyncio.iscoroutine(result) and self.loop:
                        # Schedule coroutine on main event loop
                        future = asyncio.run_coroutine_threadsafe(result, self.loop)
                        # Add callback to check for exceptions
                        def _check_exception(f):
                            try:
                                f.result()
                            except Exception as e:
                                logger.error(f"Error in on_press coroutine: {e}")
                        future.add_done_callback(_check_exception)
                except Exception as e:
                    logger.error(f"Error in on_press callback: {e}")

    def _on_key_release(self, key) -> None:
        """Handle key release."""
        self._key_states[key] = False

        if self.is_pressed and not self._check_hotkey():
            self.is_pressed = False
            logger.info("Hotkey released")

            if self.on_release:
                try:
                    result = self.on_release()
                    if asyncio.iscoroutine(result) and self.loop:
                        # Schedule coroutine on main event loop
                        future = asyncio.run_coroutine_threadsafe(result, self.loop)
                        # Add callback to check for exceptions
                        def _check_exception(f):
                            try:
                                f.result()
                            except Exception as e:
                                logger.error(f"Error in on_release coroutine: {e}")
                        future.add_done_callback(_check_exception)
                except Exception as e:
                    logger.error(f"Error in on_release callback: {e}")


def create_hotkey_listener(
    on_press: Optional[Callable] = None,
    on_release: Optional[Callable] = None,
    prefer_pynput: bool = True
) -> HotkeyListener | PynputHotkeyListener:
    """
    Create appropriate hotkey listener for the platform.

    Args:
        on_press: Callback when hotkey is pressed
        on_release: Callback when hotkey is released
        prefer_pynput: Prefer pynput for macOS (more reliable)

    Returns:
        HotkeyListener or PynputHotkeyListener instance
    """
    if IS_MACOS and prefer_pynput:
        logger.info("Using pynput for macOS hotkey listener")
        return PynputHotkeyListener(on_press=on_press, on_release=on_release)
    else:
        logger.info("Using keyboard library for hotkey listener")
        return HotkeyListener(on_press=on_press, on_release=on_release)
