"""
Paste helper for the Electron main process.

Simulates the system paste shortcut into the currently focused application.
"""
from __future__ import annotations

import argparse
import json
import platform
import sys
import time


IS_MACOS = platform.system() == "Darwin"


def print_result(payload: dict) -> int:
    print(json.dumps(payload, ensure_ascii=False))
    return 0 if payload.get("success", False) else 1


def paste(delay_ms: int = 120) -> None:
    from pynput.keyboard import Controller, Key

    keyboard = Controller()
    time.sleep(max(delay_ms, 0) / 1000)

    modifier = Key.cmd if IS_MACOS else Key.ctrl
    keyboard.press(modifier)
    keyboard.press("v")
    keyboard.release("v")
    keyboard.release(modifier)


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
