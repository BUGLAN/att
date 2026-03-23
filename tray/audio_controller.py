"""
Audio Controller for macOS
Mutes and unmutes system output during recording
"""
import subprocess
import platform
import logging
from typing import Optional

logger = logging.getLogger(__name__)


class AudioController:
    """
    Controls system output mute on macOS.

    Uses AppleScript to mute/unmute system output volume.
    This ensures no other audio interferes with recording.

    Note: This only works on macOS. On other platforms, the controller
    will be disabled and methods will return False.
    """

    def __init__(self, enabled: bool = True):
        """
        Initialize audio controller.

        Args:
            enabled: Whether audio control is enabled
        """
        self.enabled = enabled
        self.is_macos = platform.system() == "Darwin"
        self._original_muted = False  # Track original mute state

        if not self.is_macos:
            logger.info("Audio control only supported on macOS")

    def is_muted(self) -> bool:
        """
        Check if system output is currently muted.

        Returns:
            True if muted, False otherwise
        """
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

    def set_mute(self, mute: bool) -> bool:
        """
        Set system output mute state.

        Args:
            mute: True to mute, False to unmute

        Returns:
            True if successful, False otherwise
        """
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

    def pause_all(self) -> bool:
        """
        Mute system output (called before recording).

        Saves the current mute state and mutes the system.

        Returns:
            True if successful, False otherwise
        """
        if not self.enabled or not self.is_macos:
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
        if not self.enabled or not self.is_macos:
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
