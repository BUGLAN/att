"""
Audio Controller for macOS
Mutes and unmutes system output during recording
"""
import subprocess
import platform
import logging
from typing import Optional, TYPE_CHECKING

if TYPE_CHECKING:
    from audio_controller import AudioController

logger = logging.getLogger(__name__)


class MuteContext:
    """
    Context manager for system mute state.

    Ensures mute state is always restored when exiting the context,
    even if exceptions occur. Designed for cross-method usage where
    __aenter__ and __aexit__ are called separately.

    Usage:
        # Normal usage with 'async with'
        async with audio_controller.mute_context():
            # System is muted here
            await do_recording()
        # System is restored here

        # Cross-method usage (for hotkey press/release)
        self._mute_context = audio_controller.mute_context()
        await self._mute_context.__aenter__()  # On press
        # ... later ...
        await self._mute_context.__aexit__(None, None, None)  # On release
    """

    def __init__(self, controller: 'AudioController'):
        self.controller = controller
        self._original_muted: Optional[bool] = None
        self._entered: bool = False

    async def __aenter__(self) -> 'MuteContext':
        """Enter mute context - save state and mute."""
        # Skip if already inside this context (re-entrant)
        if self._entered:
            logger.debug("MuteContext: Already entered, skipping nested entry")
            return self

        if not self.controller.enabled:
            logger.debug("MuteContext: Audio control disabled")
            self._entered = True
            return self

        if not (self.controller.is_macos or self.controller.is_windows):
            logger.debug("MuteContext: Audio control not supported on this platform")
            self._entered = True
            return self

        logger.info("MuteContext: Entering - saving and muting system")

        # Save current mute state
        try:
            self._original_muted = self.controller.is_muted()
        except Exception:
            self._original_muted = False

        # Mute if not already muted
        if not self._original_muted:
            if not self.controller.set_mute(True):
                logger.warning("MuteContext: Failed to mute system")

        self._entered = True
        return self

    async def __aexit__(self, exc_type, exc_val, exc_tb) -> bool:
        """Exit mute context - restore state (always)."""
        if not self._entered:
            logger.debug("MuteContext: Not entered, skipping exit")
            return False

        # Always attempt restore, even on exception
        try:
            if self.controller.enabled and (self.controller.is_macos or self.controller.is_windows):
                logger.info("MuteContext: Exiting - restoring mute state")
                if self._original_muted is not None:
                    self.controller.set_mute(self._original_muted)
        except Exception as e:
            logger.error(f"MuteContext: Failed to restore mute state: {e}")
        finally:
            self._entered = False

        # Don't suppress exceptions
        return False


class AudioController:
    """
    Controls system output mute on macOS and Windows.

    - macOS: Uses AppleScript to mute/unmute system output volume.
    - Windows: Uses pycaw to access Core Audio API.

    This ensures no other audio interferes with recording.
    """

    def __init__(self, enabled: bool = True):
        """
        Initialize audio controller.

        Args:
            enabled: Whether audio control is enabled
        """
        self.enabled = enabled
        self.is_macos = platform.system() == "Darwin"
        self.is_windows = platform.system() == "Windows"
        self._original_muted = False  # Track original mute state
        self._windows_audio = None  # Windows: pycaw interface

        if self.is_macos:
            logger.info("Audio control using AppleScript (macOS)")
        elif self.is_windows:
            self._init_windows_audio()
        else:
            logger.info("Audio control only supported on macOS and Windows")

    def _init_windows_audio(self):
        """Initialize Windows audio interface using pycaw."""
        try:
            from pycaw.pycaw import AudioUtilities
            devices = AudioUtilities.GetSpeakers()
            self._windows_audio = devices.EndpointVolume
            if self._windows_audio:
                logger.info("Windows audio interface initialized (pycaw)")
            else:
                logger.warning("Windows EndpointVolume not available")
        except ImportError:
            logger.warning("pycaw not installed, Windows audio control disabled")
        except Exception as e:
            logger.warning(f"Failed to init Windows audio: {e}")

    def is_muted(self) -> bool:
        """
        Check if system output is currently muted.

        Returns:
            True if muted, False otherwise
        """
        if self.is_macos:
            cmd = 'osascript -e "output muted of (get volume settings)"'
            try:
                result = subprocess.run(
                    cmd,
                    shell=True,
                    capture_output=True,
                    text=True,
                    timeout=5
                )
                return result.stdout.strip().lower() == "true"
            except Exception as e:
                logger.error(f"Failed to check mute status: {e}")
                return False
        elif self.is_windows and self._windows_audio:
            try:
                return self._windows_audio.GetMute() == 1
            except Exception as e:
                logger.error(f"Failed to check Windows mute status: {e}")
                return False
        return False

    def set_mute(self, mute: bool) -> bool:
        """
        Set system output mute state.

        Args:
            mute: True to mute, False to unmute

        Returns:
            True if successful, False otherwise
        """
        if self.is_macos:
            state = "true" if mute else "false"
            cmd = f'osascript -e "set volume output muted {state}"'
            try:
                subprocess.run(
                    cmd,
                    shell=True,
                    check=True,
                    capture_output=True,
                    text=True,
                    timeout=5
                )
                logger.debug(f"System {'muted' if mute else 'unmuted'}")
                return True
            except subprocess.TimeoutExpired:
                logger.error("Timeout setting mute state")
                return False
            except subprocess.CalledProcessError as e:
                logger.error(f"Failed to set mute state: {e.stderr}")
                return False
            except Exception as e:
                logger.error(f"Unexpected error setting mute state: {e}")
                return False
        elif self.is_windows and self._windows_audio:
            try:
                self._windows_audio.SetMute(1 if mute else 0, None)
                logger.debug(f"Windows system {'muted' if mute else 'unmuted'}")
                return True
            except Exception as e:
                logger.error(f"Failed to set Windows mute state: {e}")
                return False
        return False

    def pause_all(self) -> bool:
        """
        Mute system output (called before recording).

        Saves the current mute state and mutes the system.

        Returns:
            True if successful, False otherwise
        """
        if not self.enabled:
            return False

        if not (self.is_macos or self.is_windows):
            return False

        logger.info("Muting system output...")

        # Save current mute state
        try:
            self._original_muted = self.is_muted()
        except Exception:
            self._original_muted = False

        # Mute system (unless already muted)
        if self._original_muted:
            logger.debug("System already muted, skipping")
            return True

        return self.set_mute(True)

    def resume_all(self) -> bool:
        """
        Restore system mute state (called after recording).

        Restores the original mute state before recording started.

        Returns:
            True if successful, False otherwise
        """
        if not self.enabled:
            return False

        if not (self.is_macos or self.is_windows):
            return False

        logger.info("Restoring system mute state...")

        # Restore original mute state
        return self.set_mute(self._original_muted)

    def set_enabled(self, enabled: bool) -> None:
        """
        Enable or disable audio control.

        Args:
            enabled: Whether to enable audio control
        """
        self.enabled = enabled
        logger.info(f"Audio control {'enabled' if enabled else 'disabled'}")

    def mute_context(self) -> MuteContext:
        """
        Create a context manager for safe mute control.

        The context manager ensures mute state is always restored when
        exiting, even if exceptions occur or the calling code doesn't
        complete normally.

        Returns:
            MuteContext instance for use with 'async with'

        Example:
            async with audio_controller.mute_context():
                # System is muted
                await record_audio()
            # System restored automatically

            # Cross-method usage (for hotkey press/release):
            ctx = audio_controller.mute_context()
            await ctx.__aenter__()  # On press - mutes system
            # ... recording happens ...
            await ctx.__aexit__(None, None, None)  # On release - restores
        """
        return MuteContext(self)


# Singleton instance for convenience
_controller: Optional[AudioController] = None


def get_audio_controller(enabled: bool = True) -> AudioController:
    """
    Get the singleton AudioController instance.

    Args:
        enabled: Whether to enable audio control

    Returns:
        AudioController instance
    """
    global _controller
    if _controller is None:
        _controller = AudioController(enabled=enabled)
    return _controller
