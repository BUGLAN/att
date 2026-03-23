"""
Audio Controller for macOS
Pauses and resumes media playback during recording
"""
import subprocess
import platform
import logging
from typing import Optional

logger = logging.getLogger(__name__)


class AudioController:
    """
    Controls media playback on macOS.

    Uses AppleScript to send media key events (Play/Pause).
    This provides a simple and reliable way to pause/resume
    any media playing on the system (Spotify, Music, Chrome, etc.)

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
        self._was_paused = False  # Track if we paused something

        if not self.is_macos:
            logger.info("Audio control only supported on macOS")

    def pause_all(self) -> bool:
        """
        Pause all media playback.

        Sends a Play/Pause media key event, which will pause
        any currently playing media.

        Returns:
            True if successful, False otherwise
        """
        if not self.enabled or not self.is_macos:
            return False

        logger.info("Pausing media playback...")

        # Send Play/Pause key to pause current playback
        success = self._send_media_key("playpause")

        if success:
            self._was_paused = True

        return success

    def resume_all(self) -> bool:
        """
        Resume media playback.

        Sends another Play/Pause media key event to resume.
        Only resumes if we previously paused something.

        Returns:
            True if successful, False otherwise
        """
        if not self.enabled or not self.is_macos:
            return False

        # Only resume if we paused something
        if not self._was_paused:
            logger.info("No media was paused, skipping resume")
            return True

        logger.info("Resuming media playback...")

        # Send Play/Pause key to resume
        success = self._send_media_key("playpause")

        if success:
            self._was_paused = False

        return success

    def _send_media_key(self, key: str) -> bool:
        """
        Send media key event via AppleScript.

        Args:
            key: Media key type ('playpause', 'next', 'previous')

        Returns:
            True if successful, False otherwise
        """
        # Media key codes for macOS
        # Reference: https://eastmanreference.com/complete-list-of-applescript-key-codes
        key_codes = {
            "playpause": 16,    # Play/Pause
            "next": 17,         # Next track
            "previous": 18      # Previous track
        }

        code = key_codes.get(key)
        if code is None:
            logger.error(f"Unknown media key: {key}")
            return False

        script = f'''
        tell application "System Events"
            key code {code}
        end tell
        '''

        try:
            result = subprocess.run(
                ["osascript", "-e", script],
                check=True,
                capture_output=True,
                text=True,
                timeout=5
            )
            logger.debug(f"Media key sent: {key} (code {code})")
            return True

        except subprocess.TimeoutExpired:
            logger.error("Timeout sending media key")
            return False

        except subprocess.CalledProcessError as e:
            logger.error(f"Failed to send media key: {e.stderr}")
            return False

        except Exception as e:
            logger.error(f"Unexpected error sending media key: {e}")
            return False

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
