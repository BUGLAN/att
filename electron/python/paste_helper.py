"""
Paste helper for the Electron main process.

Simulates the system paste shortcut into the currently focused application.
"""
from __future__ import annotations

import argparse
import ctypes
import json
import platform
import sys
import time


IS_MACOS = platform.system() == "Darwin"
IS_WINDOWS = platform.system() == "Windows"


def print_result(payload: dict) -> int:
    print(json.dumps(payload, ensure_ascii=False))
    return 0 if payload.get("success", False) else 1


def _paste_with_pynput(delay_ms: int = 120) -> None:
    from pynput.keyboard import Controller, Key

    keyboard = Controller()
    time.sleep(max(delay_ms, 0) / 1000)

    modifier = Key.cmd if IS_MACOS else Key.ctrl
    keyboard.press(modifier)
    time.sleep(0.04)
    keyboard.press("v")
    time.sleep(0.02)
    keyboard.release("v")
    time.sleep(0.04)
    keyboard.release(modifier)


def _paste_with_sendinput(delay_ms: int = 120) -> None:
    user32 = ctypes.windll.user32
    time.sleep(max(delay_ms, 0) / 1000)

    VK_CONTROL = 0x11
    VK_V = 0x56
    KEYEVENTF_KEYUP = 0x0002

    user32.keybd_event(VK_CONTROL, 0, 0, 0)
    time.sleep(0.04)
    user32.keybd_event(VK_V, 0, 0, 0)
    time.sleep(0.02)
    user32.keybd_event(VK_V, 0, KEYEVENTF_KEYUP, 0)
    time.sleep(0.04)
    user32.keybd_event(VK_CONTROL, 0, KEYEVENTF_KEYUP, 0)


def paste(delay_ms: int = 120) -> None:
    if IS_WINDOWS:
        _paste_with_sendinput(delay_ms)
        return

    _paste_with_pynput(delay_ms)


def main() -> int:
    parser = argparse.ArgumentParser(description="Electron paste helper")
    parser.add_argument("--delay-ms", type=int, default=120, help="Delay before simulating paste")
    args = parser.parse_args()

    try:
        paste(args.delay_ms)
        return print_result({
            "success": True,
            "platform": platform.system(),
            "delay_ms": args.delay_ms
        })
    except Exception as error:
        return print_result({
            "success": False,
            "platform": platform.system(),
            "error": str(error)
        })


if __name__ == "__main__":
    sys.exit(main())
