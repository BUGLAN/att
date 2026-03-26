"""
System audio helper for the Electron main process.

This script is intentionally stateless. The Electron main process tracks
the original mute state and invokes this helper for point-in-time actions.
"""
from __future__ import annotations

import argparse
import json
import platform
import subprocess
import sys


IS_MACOS = platform.system() == "Darwin"
IS_WINDOWS = platform.system() == "Windows"


class AudioController:
    """Minimal cross-platform mute controller."""

    def __init__(self) -> None:
        self.is_macos = IS_MACOS
        self.is_windows = IS_WINDOWS
        self._windows_audio = None

        if self.is_windows:
            self._init_windows_audio()

    def _init_windows_audio(self) -> None:
        try:
            from pycaw.pycaw import AudioUtilities
            devices = AudioUtilities.GetSpeakers()
            self._windows_audio = devices.EndpointVolume
        except Exception:
            self._windows_audio = None

    @property
    def supported(self) -> bool:
        return self.is_macos or (self.is_windows and self._windows_audio is not None)

    def is_muted(self) -> bool:
        if self.is_macos:
            result = subprocess.run(
                ['osascript', '-e', 'output muted of (get volume settings)'],
                capture_output=True,
                text=True,
                timeout=5,
                check=False
            )
            return result.stdout.strip().lower() == "true"

        if self.is_windows and self._windows_audio is not None:
            return bool(self._windows_audio.GetMute())

        raise RuntimeError("Audio mute control is not available on this platform")

    def set_mute(self, mute: bool) -> bool:
        if self.is_macos:
            state = "true" if mute else "false"
            subprocess.run(
                ['osascript', '-e', f'set volume output muted {state}'],
                capture_output=True,
                text=True,
                timeout=5,
                check=True
            )
            return True

        if self.is_windows and self._windows_audio is not None:
            self._windows_audio.SetMute(1 if mute else 0, None)
            return True

        raise RuntimeError("Audio mute control is not available on this platform")


def print_result(payload: dict) -> int:
    print(json.dumps(payload, ensure_ascii=False))
    return 0 if payload.get("success", False) else 1


def main() -> int:
    parser = argparse.ArgumentParser(description="Electron audio helper")
    subparsers = parser.add_subparsers(dest="command", required=True)

    subparsers.add_parser("status", help="Get current mute status")

    set_mute_parser = subparsers.add_parser("set-mute", help="Set mute state")
    set_mute_parser.add_argument("value", choices=["true", "false"])

    args = parser.parse_args()
    controller = AudioController()

    if not controller.supported:
        return print_result({
            "success": False,
            "supported": False,
            "platform": platform.system(),
            "error": "Audio mute control is unavailable. Install pycaw on Windows or use macOS."
        })

    try:
        if args.command == "status":
            return print_result({
                "success": True,
                "supported": True,
                "platform": platform.system(),
                "muted": controller.is_muted()
            })

        if args.command == "set-mute":
            muted = args.value == "true"
            controller.set_mute(muted)
            return print_result({
                "success": True,
                "supported": True,
                "platform": platform.system(),
                "muted": muted
            })
    except subprocess.CalledProcessError as error:
        stderr = (error.stderr or "").strip()
        return print_result({
            "success": False,
            "supported": True,
            "platform": platform.system(),
            "error": stderr or str(error)
        })
    except Exception as error:
        return print_result({
            "success": False,
            "supported": True,
            "platform": platform.system(),
            "error": str(error)
        })

    return print_result({
        "success": False,
        "supported": True,
        "platform": platform.system(),
        "error": "Unknown command"
    })


if __name__ == "__main__":
    sys.exit(main())
