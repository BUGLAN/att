"""
macOS native status bar app using rumps.
More reliable than pystray on macOS.
"""
import asyncio
import logging
import threading
from typing import Optional

logger = logging.getLogger(__name__)

# Check if rumps is available
try:
    import rumps
    RUMPS_AVAILABLE = True
except ImportError:
    RUMPS_AVAILABLE = False
    logger.warning("rumps not available, install with: pip install rumps")


class RumpsTrayApp(rumps.App):
    """
    macOS native status bar app using rumps.
    """
    
    def __init__(
        self,
        on_quit: Optional[callable] = None,
        on_reconnect: Optional[callable] = None,
        on_status_update: Optional[callable] = None,
        title: str = "ATT"
    ):
        """
        Initialize rumps tray app.
        
        Args:
            on_quit: Callback when quit is clicked
            on_reconnect: Callback when reconnect is clicked
            on_status_update: Callback to get current status
            title: App title
        """
        if not RUMPS_AVAILABLE:
            raise ImportError("rumps is not available")
        
        super().__init__(title, quit_button=None)
        
        self.on_quit = on_quit
        self.on_reconnect = on_reconnect
        self.on_status_update = on_status_update
        
        # State
        self.is_recording = False
        self.is_transcribing = False
        self.is_connected = False
        self.hotkey_display = "⌘⌥"
        
        # Build menu
        self._build_menu()
        
        # Start with disconnected icon
        self._update_icon("disconnected")
    
    def _build_menu(self):
        """Build the menu."""
        self.menu = [
            "状态: 空闲",
            None,  # Separator
            "WebSocket: 未连接",
            f"快捷键: {self.hotkey_display}",
            None,  # Separator
            "重连",
            "退出"
        ]
    
    def _update_icon(self, status: str):
        """Update the status bar icon based on status."""
        # Use emoji/text as icon (more reliable than image icons)
        icons = {
            "idle": "🎙️",           # Microphone emoji
            "recording": "🔴",       # Red circle
            "transcribing": "🟡",    # Yellow circle
            "disconnected": "⚪"     # White circle
        }
        self.title = icons.get(status, icons["idle"])
    
    def update_status(self, recording: bool = False, transcribing: bool = False, connected: bool = False):
        """Update the status display."""
        self.is_recording = recording
        self.is_transcribing = transcribing
        self.is_connected = connected
        
        # Update icon
        if not connected:
            self._update_icon("disconnected")
        elif recording:
            self._update_icon("recording")
        elif transcribing:
            self._update_icon("transcribing")
        else:
            self._update_icon("idle")
        
        # Update menu items
        status_text = "录音中" if recording else "空闲"
        ws_text = "已连接" if connected else "未连接"
        
        self.menu["状态: 空闲"].title = f"状态: {status_text}"
        self.menu["WebSocket: 未连接"].title = f"WebSocket: {ws_text}"
    
    def update_hotkey_display(self, display: str):
        """Update the hotkey display in menu."""
        self.hotkey_display = display
        self.menu[f"快捷键: {self.hotkey_display}"].title = f"快捷键: {display}"
    
    @rumps.clicked("退出")
    def quit_callback(self, _):
        """Handle quit click."""
        logger.info("Quit clicked")
        if self.on_quit:
            self.on_quit()
        rumps.quit_application()
    
    @rumps.clicked("重连")
    def reconnect_callback(self, _):
        """Handle reconnect click."""
        logger.info("Reconnect clicked")
        if self.on_reconnect:
            self.on_reconnect()


def create_rumps_app(
    on_quit: Optional[callable] = None,
    on_reconnect: Optional[callable] = None,
    on_status_update: Optional[callable] = None
) -> Optional[RumpsTrayApp]:
    """
    Create a rumps tray app if available.
    
    Returns:
        RumpsTrayApp instance or None if rumps not available
    """
    if not RUMPS_AVAILABLE:
        return None
    
    return RumpsTrayApp(
        on_quit=on_quit,
        on_reconnect=on_reconnect,
        on_status_update=on_status_update
    )
