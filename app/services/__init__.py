"""
Services module for Audio-to-Text application
"""
from app.services.hotkey_listener import get_hotkey_service, init_hotkey_service
from app.services.websocket import manager as ws_manager

__all__ = ["ws_manager", "get_hotkey_service", "init_hotkey_service"]
