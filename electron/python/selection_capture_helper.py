"""Windows 选区复制回退 helper。"""
import ctypes
import json
import sys
import time


CF_UNICODETEXT = 13
GMEM_MOVEABLE = 0x0002
VK_CONTROL = 0x11
VK_C = 0x43
KEYEVENTF_KEYUP = 0x0002

user32 = ctypes.windll.user32
kernel32 = ctypes.windll.kernel32

user32.OpenClipboard.argtypes = [ctypes.c_void_p]
user32.OpenClipboard.restype = ctypes.c_bool
user32.CloseClipboard.argtypes = []
user32.CloseClipboard.restype = ctypes.c_bool
user32.IsClipboardFormatAvailable.argtypes = [ctypes.c_uint]
user32.IsClipboardFormatAvailable.restype = ctypes.c_bool
user32.GetClipboardData.argtypes = [ctypes.c_uint]
user32.GetClipboardData.restype = ctypes.c_void_p
user32.EmptyClipboard.argtypes = []
user32.EmptyClipboard.restype = ctypes.c_bool
user32.SetClipboardData.argtypes = [ctypes.c_uint, ctypes.c_void_p]
user32.SetClipboardData.restype = ctypes.c_void_p
user32.keybd_event.argtypes = [ctypes.c_ubyte, ctypes.c_ubyte, ctypes.c_uint, ctypes.c_void_p]
user32.keybd_event.restype = None
kernel32.GlobalLock.argtypes = [ctypes.c_void_p]
kernel32.GlobalLock.restype = ctypes.c_void_p
kernel32.GlobalUnlock.argtypes = [ctypes.c_void_p]
kernel32.GlobalUnlock.restype = ctypes.c_bool
kernel32.GlobalAlloc.argtypes = [ctypes.c_uint, ctypes.c_size_t]
kernel32.GlobalAlloc.restype = ctypes.c_void_p


def open_clipboard():
    for _ in range(20):
        if user32.OpenClipboard(None):
            return True
        time.sleep(0.01)
    return False


def close_clipboard():
    user32.CloseClipboard()


def read_clipboard_text() -> str:
    if not open_clipboard():
        return ""

    try:
        if not user32.IsClipboardFormatAvailable(CF_UNICODETEXT):
            return ""

        handle = user32.GetClipboardData(CF_UNICODETEXT)
        if not handle:
            return ""

        locked = kernel32.GlobalLock(handle)
        if not locked:
            return ""

        try:
            return ctypes.wstring_at(locked)
        finally:
            kernel32.GlobalUnlock(handle)
    finally:
        close_clipboard()


def write_clipboard_text(text: str) -> None:
    value = text or ""
    encoded = (value + "\0").encode("utf-16-le")
    global_handle = kernel32.GlobalAlloc(GMEM_MOVEABLE, len(encoded))
    if not global_handle:
        return

    locked = kernel32.GlobalLock(global_handle)
    if not locked:
        return

    try:
        ctypes.memmove(locked, encoded, len(encoded))
    finally:
        kernel32.GlobalUnlock(global_handle)

    if not open_clipboard():
        return

    try:
        user32.EmptyClipboard()
        user32.SetClipboardData(CF_UNICODETEXT, global_handle)
    finally:
        close_clipboard()


def send_key_event(virtual_key: int, key_up: bool = False) -> None:
    user32.keybd_event(virtual_key, 0, KEYEVENTF_KEYUP if key_up else 0, 0)


def send_ctrl_c() -> None:
    send_key_event(VK_CONTROL, key_up=False)
    send_key_event(VK_C, key_up=False)
    send_key_event(VK_C, key_up=True)
    send_key_event(VK_CONTROL, key_up=True)


def capture_selected_text() -> dict:
    previous_text = read_clipboard_text()
    send_ctrl_c()
    time.sleep(0.12)
    selected_text = read_clipboard_text()
    clipboard_changed = previous_text != selected_text

    if clipboard_changed:
        write_clipboard_text(previous_text)

    selected_text = (selected_text or "").strip()
    if not selected_text:
        return {
            "success": False,
            "error": "No selected text captured"
        }

    if not clipboard_changed:
        return {
            "success": False,
            "error": "Clipboard unchanged after Ctrl+C fallback"
        }

    return {
        "success": True,
        "text": selected_text
    }


def main() -> int:
    if sys.platform != "win32":
        print(json.dumps({
            "success": False,
            "error": "Selection capture helper only supports Windows"
        }, ensure_ascii=False))
        return 1

    try:
        if hasattr(sys.stdout, "reconfigure"):
            sys.stdout.reconfigure(encoding="utf-8")
        print(json.dumps(capture_selected_text(), ensure_ascii=False))
        return 0
    except Exception as error:
        print(json.dumps({
            "success": False,
            "error": str(error)
        }, ensure_ascii=False))
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
