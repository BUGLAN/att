"""
System Tray Application for Audio-to-Text
Main entry point for the tray tool
"""
import asyncio
import platform
import sys
import logging
from pathlib import Path
from typing import Optional

# Setup path for imports
sys.path.insert(0, str(Path(__file__).parent.parent))

try:
    import pystray
    from PIL import Image, ImageDraw
except ImportError:
    print("Required packages: pip install pystray Pillow")
    sys.exit(1)

from tray.websocket_client import TrayWebSocketClient
from tray.hotkey import create_hotkey_listener
from tray.audio_controller import get_audio_controller

# Import get_default_hotkey from app config
import sys
from pathlib import Path
sys.path.insert(0, str(Path(__file__).parent.parent))
from app.config import get_default_hotkey

# Configure logging
logging.basicConfig(
    level=logging.INFO,
    format='%(asctime)s - %(name)s - %(levelname)s - %(message)s'
)
logger = logging.getLogger(__name__)

# Platform detection
IS_MACOS = platform.system() == "Darwin"


class TrayApp:
    """
    System tray application for Audio-to-Text.

    Features:
    - Global hotkey for recording (Cmd+Shift+R on macOS, Ctrl+Shift+R on Windows)
    - WebSocket connection to backend
    - Status indicator in tray icon
    """

    def __init__(self, server_url: str = "ws://localhost:8000/api/ws/tray"):
        """
        Initialize tray application.

        Args:
            server_url: WebSocket server URL
        """
        self.server_url = server_url

        # Components
        self.ws_client: Optional[TrayWebSocketClient] = None
        self.hotkey_listener = None
        self.tray_icon: Optional[pystray.Icon] = None

        # State
        self.is_recording = False
        self.is_transcribing = False
        self.is_connected = False
        self.connected_clients = 0

        # Audio controller
        self.audio_controller = get_audio_controller(enabled=True)

        # Hotkey configuration
        self.hotkey_config: Optional[dict] = None

        # Settings
        self.pause_audio_on_recording: bool = True  # Default value

        # Event loop
        self.loop: Optional[asyncio.AbstractEventLoop] = None
        self._ws_task: Optional[asyncio.Task] = None
        self._running = False

    def _create_icon_image(self, status: str = "idle") -> Image.Image:
        """
        Create tray icon image.

        Args:
            status: Icon status (idle, recording, transcribing, disconnected)

        Returns:
            PIL Image for tray icon
        """
        # Icon size (32x32 for most platforms)
        size = 32

        # Colors for different states
        colors = {
            "idle": "#52525b",      # Gray
            "recording": "#f87171",  # Red
            "transcribing": "#c9a227",  # Gold
            "disconnected": "#71717a"  # Lighter gray
        }

        color = colors.get(status, colors["idle"])

        # Create image
        image = Image.new('RGBA', (size, size), (0, 0, 0, 0))
        draw = ImageDraw.Draw(image)

        # Draw microphone icon
        center = size // 2
        radius = size // 3

        # Outer circle
        draw.ellipse(
            [center - radius, center - radius, center + radius, center + radius],
            fill=color
        )

        # Inner circle (microphone hole)
        inner_radius = radius // 2
        draw.ellipse(
            [center - inner_radius, center - inner_radius, center + inner_radius, center + inner_radius],
            fill="#000000"
        )

        # Base line
        draw.rectangle(
            [center - radius + 2, center + radius - 4, center + radius - 2, center + radius + 2],
            fill=color
        )

        return image

    def _create_menu(self) -> pystray.Menu:
        """Create tray menu."""
        # Get current hotkey display
        hotkey_display = self._get_hotkey_display()

        return pystray.Menu(
            pystray.MenuItem(
                lambda item: f"状态: {'录音中' if self.is_recording else '空闲'}",
                None,
                enabled=False
            ),
            pystray.Menu.SEPARATOR,
            pystray.MenuItem(
                lambda item: f"WebSocket: {'已连接' if self.is_connected else '未连接'}",
                None,
                enabled=False
            ),
            pystray.MenuItem(
                lambda item: f"快捷键: {hotkey_display}",
                None,
                enabled=False
            ),
            pystray.Menu.SEPARATOR,
            pystray.MenuItem("重连", self._reconnect),
            pystray.MenuItem("退出", self._quit)
        )

    def _get_hotkey_display(self) -> str:
        """Get current hotkey display string."""
        # First try hotkey listener's config
        if self.hotkey_listener and hasattr(self.hotkey_listener, '_hotkey_config'):
            config = self.hotkey_listener._hotkey_config
            if config:
                return config.get('display', '未设置')

        # Fallback to stored config
        if self.hotkey_config:
            return self.hotkey_config.get('display', '未设置')

        # Final fallback to default
        default = get_default_hotkey()
        return default.get('display', '⌥⌘' if IS_MACOS else 'Alt+Ctrl')

    def _update_icon(self) -> None:
        """Update tray icon based on current state."""
        if not self.tray_icon:
            return

        if not self.is_connected:
            status = "disconnected"
        elif self.is_recording:
            status = "recording"
        elif self.is_transcribing:
            status = "transcribing"
        else:
            status = "idle"

        try:
            self.tray_icon.icon = self._create_icon_image(status)
            self.tray_icon.menu = self._create_menu()
        except Exception as e:
            logger.error(f"Failed to update icon: {e}")

    def _on_status_update(self, status: dict) -> None:
        """Handle status update from server."""
        self.is_recording = status.get("recording", False)
        self.is_transcribing = status.get("transcribing", False)
        self.connected_clients = status.get("connected_clients", 0)

        logger.info(f"Status update: recording={self.is_recording}, transcribing={self.is_transcribing}")

        # Update icon
        if self.tray_icon:
            self._update_icon()

    def _on_connected(self) -> None:
        """Handle WebSocket connected."""
        self.is_connected = True
        logger.info("WebSocket connected")
        self._update_icon()

    def _on_disconnected(self) -> None:
        """Handle WebSocket disconnected."""
        self.is_connected = False
        logger.info("WebSocket disconnected")
        self._update_icon()

    def _on_hotkey_change(self, hotkey_config: dict) -> None:
        """Handle hotkey configuration change from server."""
        logger.info(f"Received hotkey update: {hotkey_config}")

        # Store the config
        self.hotkey_config = hotkey_config

        # Update hotkey listener
        if self.hotkey_listener and hasattr(self.hotkey_listener, 'set_hotkey'):
            self.hotkey_listener.set_hotkey(hotkey_config)

        # Update tray menu
        self._update_icon()

    def _on_web_recording(self, action: str) -> None:
        """
        Handle web recording notification.

        Args:
            action: "start" or "stop"
        """
        logger.info(f"Web recording notification: {action}, pause_audio_on_recording={self.pause_audio_on_recording}")

        if not self.pause_audio_on_recording:
            logger.info("Pause audio on recording is disabled, skipping")
            return

        if action == "start":
            # Pause other audio playback
            if not self.audio_controller.pause_all():
                logger.warning("Failed to pause media playback")
        elif action == "stop":
            # Resume audio playback
            if not self.audio_controller.resume_all():
                logger.warning("Failed to resume media playback")

    def _on_settings_change(self, settings: dict) -> None:
        """
        Handle settings change notification.

        Args:
            settings: Settings dict with updated values
        """
        pause_audio = settings.get("pause_audio_on_recording")
        if pause_audio is not None:
            self.pause_audio_on_recording = pause_audio
            logger.info(f"Updated pause_audio_on_recording to: {pause_audio}")

    async def _on_hotkey_press(self) -> None:
        """Handle hotkey press - start recording."""
        if self.is_recording:
            logger.info("Already recording, ignoring hotkey")
            return

        logger.info("Hotkey pressed - starting recording")

        # Pause other audio playback (if setting is enabled)
        if self.pause_audio_on_recording:
            if not self.audio_controller.pause_all():
                logger.warning("Failed to pause media playback")

        if self.ws_client and self.ws_client.is_connected:
            await self.ws_client.send_recording_command("start")
            self.is_recording = True
            self._update_icon()

    async def _on_hotkey_release(self) -> None:
        """Handle hotkey release - stop recording."""
        if not self.is_recording:
            return

        logger.info("Hotkey released - stopping recording")

        if self.ws_client and self.ws_client.is_connected:
            await self.ws_client.send_recording_command("stop")
            self.is_recording = False
            self._update_icon()

        # Resume audio playback (if setting is enabled)
        if self.pause_audio_on_recording:
            if not self.audio_controller.resume_all():
                logger.warning("Failed to resume media playback")

    def _reconnect(self) -> None:
        """Manually reconnect WebSocket."""
        if self.loop and self.ws_client:
            asyncio.run_coroutine_threadsafe(
                self.ws_client.connect(),
                self.loop
            )

    def _quit(self) -> None:
        """Quit the application."""
        logger.info("Quitting tray application")
        self._running = False  # Signal async loop to stop

        # Stop hotkey listener
        if self.hotkey_listener:
            self.hotkey_listener.stop()

        # Stop tray icon
        if self.tray_icon:
            self.tray_icon.stop()

    async def _run_async(self) -> None:
        """Run async components."""
        logger.info("_run_async started")

        # Load settings from server
        try:
            import aiohttp
            async with aiohttp.ClientSession() as session:
                async with session.get(self.server_url.replace('/ws/tray', '/settings').replace('ws://', 'http://')) as resp:
                    if resp.status == 200:
                        settings = await resp.json()

                        # Load hotkey config
                        self.hotkey_config = settings.get('hotkey')
                        if self.hotkey_config and self.hotkey_listener:
                            if hasattr(self.hotkey_listener, 'set_hotkey'):
                                self.hotkey_listener.set_hotkey(self.hotkey_config)
                                logger.info(f"Loaded hotkey config: {self.hotkey_config}")

                        # Load pause_audio_on_recording setting
                        self.pause_audio_on_recording = settings.get('pause_audio_on_recording', True)
                        logger.info(f"Loaded pause_audio_on_recording: {self.pause_audio_on_recording}")
        except Exception as e:
            logger.warning(f"Failed to load settings from server: {e}")

        # Initialize WebSocket client
        self.ws_client = TrayWebSocketClient(
            url=self.server_url,
            on_status_update=self._on_status_update,
            on_connected=self._on_connected,
            on_disconnected=self._on_disconnected,
            on_hotkey_change=self._on_hotkey_change,
            on_web_recording=self._on_web_recording,
            on_settings_change=self._on_settings_change
        )

        # Connect to server
        connected = await self.ws_client.connect()
        logger.info(f"connect() returned: {connected}")

        # Keep running
        try:
            while self._running:
                await asyncio.sleep(0.1)
        except asyncio.CancelledError:
            logger.info("_run_async cancelled")
        finally:
            # Cleanup
            if self.ws_client:
                await self.ws_client.disconnect()

    def run(self) -> None:
        """Run the tray application."""
        logger.info("Starting Audio-to-Text tray application")
        logger.info(f"Platform: {platform.system()}")
        logger.info(f"Server URL: {self.server_url}")

        # macOS accessibility permission reminder
        if IS_MACOS:
            print("\n" + "=" * 60)
            print("⚠️  macOS 权限提示")
            print("=" * 60)
            print("如果快捷键不工作，请授予辅助功能权限：")
            print("系统偏好设置 → 安全性与隐私 → 隐私 → 辅助功能")
            print("添加 Terminal 或 Python 到允许列表")
            print("=" * 60 + "\n")

        # Create tray icon
        self.tray_icon = pystray.Icon(
            "att_tray",
            icon=self._create_icon_image("disconnected"),
            menu=self._create_menu()
        )

        # Start async loop in separate thread (creates event loop)
        import threading

        def run_async_loop():
            logger.info("Async thread started")
            self.loop = asyncio.new_event_loop()
            asyncio.set_event_loop(self.loop)

            # Get default hotkey config for initial setup
            default_hotkey = get_default_hotkey()

            # Now create and set the hotkey listener with the loop and initial config
            self.hotkey_listener = create_hotkey_listener(
                on_press=self._on_hotkey_press,
                on_release=self._on_hotkey_release,
                hotkey_config=default_hotkey
            )
            # Set the event loop for the hotkey listener
            if hasattr(self.hotkey_listener, 'set_loop'):
                self.hotkey_listener.set_loop(self.loop)

            # Start hotkey listener
            if not self.hotkey_listener.start():
                logger.error("Failed to start hotkey listener")

            logger.info("Running async loop...")
            try:
                self.loop.run_until_complete(self._run_async())
            except Exception as e:
                logger.error(f"Async loop error: {type(e).__name__}: {e}")
            logger.info("Async thread ended")

        # Set running flag before starting async thread
        self._running = True
        async_thread = threading.Thread(target=run_async_loop, daemon=True)
        async_thread.start()

        # Wait a bit for the event loop to be created
        import time
        time.sleep(0.1)

        # Run tray icon (blocking)
        self.tray_icon.run()

        logger.info("Tray application stopped")


def main():
    """Main entry point."""
    import argparse

    parser = argparse.ArgumentParser(description="Audio-to-Text Tray Tool")
    parser.add_argument(
        "--server",
        default="ws://localhost:8000/api/ws/tray",
        help="WebSocket server URL (default: ws://localhost:8000/api/ws/tray)"
    )
    parser.add_argument(
        "--debug",
        action="store_true",
        help="Enable debug logging"
    )

    args = parser.parse_args()

    if args.debug:
        logging.getLogger().setLevel(logging.DEBUG)

    app = TrayApp(server_url=args.server)
    app.run()


if __name__ == "__main__":
    main()
