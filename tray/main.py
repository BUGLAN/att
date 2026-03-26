"""
System Tray Application for Audio-to-Text
Main entry point for the tray tool - PySide6 implementation
"""
import asyncio
import platform
import sys
import logging
import signal
from pathlib import Path
from typing import Optional, List
from datetime import datetime

# Setup path for imports
sys.path.insert(0, str(Path(__file__).parent.parent))

from PySide6.QtWidgets import QApplication, QSystemTrayIcon, QMenu
from PySide6.QtGui import QIcon, QAction
from PySide6.QtCore import QTimer, QThread, Signal, Slot, QCoreApplication

from tray.websocket_client import TrayWebSocketClient
from tray.hotkey import create_hotkey_listener
from tray.audio_controller import get_audio_controller, MuteContext
from app.config import get_default_hotkey

# Configure logging
logging.basicConfig(
    level=logging.INFO,
    format='%(asctime)s - %(name)s - %(levelname)s - %(message)s'
)
logger = logging.getLogger(__name__)

# Platform detection
IS_MACOS = platform.system() == "Darwin"


class AsyncWorker(QThread):
    """
    Background thread for asyncio operations (WebSocket, hotkey callbacks).
    """
    # Signals for thread-safe communication
    status_updated = Signal(bool, bool, bool)  # recording, transcribing, connected
    connected = Signal()
    disconnected = Signal()
    hotkey_changed = Signal(dict)
    web_recording = Signal(str)  # action: "start" or "stop"
    settings_changed = Signal(dict)
    error_occurred = Signal(str)
    transcription_completed = Signal(dict)  # record data
    optimization_completed = Signal(dict)  # record data

    def __init__(self, tray_app: 'TrayApp'):
        super().__init__()
        self.tray_app = tray_app
        self.loop: Optional[asyncio.AbstractEventLoop] = None
        self._running = False
        self._mute_context: Optional[MuteContext] = None  # Track active mute context

    def run(self):
        """Run asyncio event loop in background thread."""
        self.loop = asyncio.new_event_loop()
        asyncio.set_event_loop(self.loop)
        self._running = True

        try:
            self.loop.run_until_complete(self._run_async())
        except Exception as e:
            logger.error(f"AsyncWorker error: {type(e).__name__}: {e}")
            self.error_occurred.emit(str(e))
        finally:
            self.loop.close()

    async def _run_async(self):
        """Run async components."""
        logger.info("AsyncWorker started")

        # Load settings from server
        await self._load_settings()

        # Initialize WebSocket client
        self.tray_app.ws_client = TrayWebSocketClient(
            url=self.tray_app.server_url,
            on_status_update=self._on_status_update,
            on_connected=self._on_connected,
            on_disconnected=self._on_disconnected,
            on_hotkey_change=self._on_hotkey_change,
            on_web_recording=self._on_web_recording,
            on_settings_change=self._on_settings_change,
            on_transcription_completed=self._on_transcription_completed,
            on_optimization_completed=self._on_optimization_completed
        )

        # Connect to server
        connected = await self.tray_app.ws_client.connect()
        logger.info(f"WebSocket connected: {connected}")

        # Keep running
        try:
            while self._running:
                await asyncio.sleep(0.1)
        except asyncio.CancelledError:
            logger.info("AsyncWorker cancelled")
        finally:
            if self.tray_app.ws_client:
                await self.tray_app.ws_client.disconnect()

    async def _load_settings(self):
        """Load settings from server."""
        try:
            import aiohttp
            async with aiohttp.ClientSession() as session:
                async with session.get(
                    self.tray_app.server_url.replace('/ws/tray', '/settings').replace('ws://', 'http://')
                ) as resp:
                    if resp.status == 200:
                        settings = await resp.json()

                        # Load hotkey config
                        hotkey_config = settings.get('hotkey')
                        if hotkey_config and self.tray_app.hotkey_listener:
                            if hasattr(self.tray_app.hotkey_listener, 'set_hotkey'):
                                self.tray_app.hotkey_listener.set_hotkey(hotkey_config)
                                logger.info(f"Loaded hotkey config: {hotkey_config}")
                                self.hotkey_changed.emit(hotkey_config)

                        # Load pause_audio_on_recording setting
                        pause_audio = settings.get('pause_audio_on_recording', True)
                        self.tray_app.pause_audio_on_recording = pause_audio
                        logger.info(f"Loaded pause_audio_on_recording: {pause_audio}")

                        # Load auto_copy_to_clipboard setting
                        auto_copy = settings.get('auto_copy_to_clipboard', True)
                        self.tray_app.auto_copy_to_clipboard = auto_copy
                        logger.info(f"Loaded auto_copy_to_clipboard: {auto_copy}")

                        # Load auto_optimize setting
                        auto_optimize = settings.get('auto_optimize', False)
                        self.tray_app.auto_optimize = auto_optimize
                        logger.info(f"Loaded auto_optimize: {auto_optimize}")
        except Exception as e:
            logger.warning(f"Failed to load settings from server: {e}")

    def _on_status_update(self, status: dict):
        """Handle status update from server."""
        recording = status.get("recording", False)
        transcribing = status.get("transcribing", False)
        logger.info(f"Status update: recording={recording}, transcribing={transcribing}")
        self.status_updated.emit(recording, transcribing, self.tray_app.is_connected)

    def _on_connected(self):
        """Handle WebSocket connected."""
        self.tray_app.is_connected = True
        logger.info("WebSocket connected")
        self.connected.emit()

    def _on_disconnected(self):
        """Handle WebSocket disconnected."""
        self.tray_app.is_connected = False
        logger.info("WebSocket disconnected")
        self.disconnected.emit()

    def _on_hotkey_change(self, hotkey_config: dict):
        """Handle hotkey configuration change."""
        logger.info(f"Received hotkey update: {hotkey_config}")
        self.tray_app.hotkey_config = hotkey_config
        if self.tray_app.hotkey_listener and hasattr(self.tray_app.hotkey_listener, 'set_hotkey'):
            self.tray_app.hotkey_listener.set_hotkey(hotkey_config)
        self.hotkey_changed.emit(hotkey_config)

    def _on_web_recording(self, action: str):
        """Handle web recording notification."""
        logger.info(f"Web recording notification: {action}")
        self.web_recording.emit(action)

    def _on_settings_change(self, settings: dict):
        """Handle settings change notification."""
        self.settings_changed.emit(settings)

    def _on_transcription_completed(self, data: dict):
        """Handle transcription completed notification."""
        logger.info(f"Transcription completed: {data.get('record_id')}")
        self.transcription_completed.emit(data)

    def _on_optimization_completed(self, data: dict):
        """Handle optimization completed notification."""
        logger.info(f"Optimization completed: {data.get('record_id')}")
        self.optimization_completed.emit(data)

    async def on_hotkey_press(self):
        """Handle hotkey press - start recording."""
        if self.tray_app.is_recording:
            logger.info("Already recording, ignoring hotkey")
            return

        logger.info("Hotkey pressed - starting recording")

        # Mark that this recording was initiated by tray (for mute control)
        self.tray_app._recording_initiated_by_tray = True

        # Enter mute context first (will restore on exit regardless of what happens)
        if self.tray_app.pause_audio_on_recording:
            self._mute_context = self.tray_app.audio_controller.mute_context()
            await self._mute_context.__aenter__()

        if self.tray_app.ws_client and self.tray_app.ws_client.is_connected:
            await self.tray_app.ws_client.send_recording_command("start")
            self.tray_app.is_recording = True
            self.status_updated.emit(True, False, self.tray_app.is_connected)
        # Note: Even if WebSocket fails, mute will be restored on release

    async def on_hotkey_release(self):
        """Handle hotkey release - stop recording."""
        logger.info("Hotkey released - stopping recording")

        # ALWAYS exit mute context first (guaranteed restore, regardless of is_recording)
        if self._mute_context:
            await self._mute_context.__aexit__(None, None, None)
            self._mute_context = None

        # Reset the flag after mute context is handled
        self.tray_app._recording_initiated_by_tray = False

        if self.tray_app.is_recording:
            if self.tray_app.ws_client and self.tray_app.ws_client.is_connected:
                await self.tray_app.ws_client.send_recording_command("stop")
                self.tray_app.is_recording = False
                self.status_updated.emit(False, self.tray_app.is_transcribing, self.tray_app.is_connected)

    def stop(self):
        """Stop the worker."""
        self._running = False


class HistoryFetcher(QThread):
    """Background thread for fetching history records."""
    history_fetched = Signal(list)  # List of records
    error_occurred = Signal(str)

    def __init__(self, server_url: str):
        super().__init__()
        self.server_url = server_url

    def run(self):
        """Fetch history records."""
        try:
            import aiohttp
            import asyncio

            # Create a new event loop for this thread
            loop = asyncio.new_event_loop()
            asyncio.set_event_loop(loop)

            try:
                records = loop.run_until_complete(self._fetch_history())
                self.history_fetched.emit(records)
            finally:
                loop.close()
        except Exception as e:
            logger.error(f"Failed to fetch history: {e}")
            self.error_occurred.emit(str(e))

    async def _fetch_history(self) -> List[dict]:
        """Fetch history records from server."""
        import aiohttp
        async with aiohttp.ClientSession() as session:
            async with session.get(
                self.server_url.replace('/ws/tray', '/records').replace('ws://', 'http://'),
                params={'limit': 10}
            ) as resp:
                if resp.status == 200:
                    return await resp.json()
                return []


class TrayApp(QSystemTrayIcon):
    """
    PySide6-based system tray application for Audio-to-Text.

    Features:
    - System tray icon with status indicator
    - Right-click menu with status and history
    - Global hotkey for recording
    - WebSocket connection to backend
    """

    def __init__(self, server_url: str = "ws://localhost:8000/api/ws/tray"):
        super().__init__()

        self.server_url = server_url

        # State
        self.is_recording = False
        self.is_transcribing = False
        self.is_connected = False
        self.hotkey_config: Optional[dict] = None
        self.pause_audio_on_recording = True
        self.auto_copy_to_clipboard = True
        self.auto_optimize = False
        self.records: List[dict] = []
        self.history_actions: List[QAction] = []  # Dynamic history menu actions
        self._recording_initiated_by_tray = False  # Track recording source for mute control

        # Components
        self.ws_client: Optional[TrayWebSocketClient] = None
        self.hotkey_listener = None
        self.audio_controller = get_audio_controller(enabled=True)
        self.async_worker: Optional[AsyncWorker] = None
        self.history_fetcher: Optional[HistoryFetcher] = None
        self.history_timer: Optional[QTimer] = None

        # Load icons
        self._load_icons()

        # Set initial icon
        self.setIcon(self.icons["disconnected"])
        self.setToolTip("Audio-to-Text")

        # Build menu
        self._build_menu()

        # Connect signals
        self._connect_signals()

    def _load_icons(self):
        """Load icons for different states."""
        icons_dir = Path(__file__).parent / "icons"
        logger.info(f"Loading icons from: {icons_dir}")

        # Check if all required icon files exist
        required_icons = ["idle.png", "recording.png", "transcribing.png", "disconnected.png"]
        all_icons_exist = all((icons_dir / name).exists() for name in required_icons)

        # Check if we need to regenerate icons (old sizes: 22x22 or 44x44, new is 88x88)
        need_regenerate = False
        if all_icons_exist:
            # Check if existing icons are the old size
            for name in ["idle", "recording", "transcribing", "disconnected"]:
                icon_path = icons_dir / f"{name}.png"
                if icon_path.exists():
                    from PIL import Image
                    img = Image.open(icon_path)
                    if img.size[0] < 88:  # Old size, need to regenerate
                        need_regenerate = True
                        break

        # Generate icons if needed
        if not all_icons_exist or need_regenerate:
            if need_regenerate:
                logger.info("Regenerating icons for high-resolution display (88x88)")
            elif not all_icons_exist:
                logger.info("Generating missing icon files")
            icons_dir.mkdir(parents=True, exist_ok=True)
            self._generate_icons(icons_dir)

        # Load icons
        self.icons = {}
        for name in ["idle", "recording", "transcribing", "disconnected"]:
            icon_path = icons_dir / f"{name}.png"
            if icon_path.exists():
                icon = QIcon(str(icon_path))
                logger.info(f"Loaded icon {name}: path={icon_path}, null={icon.isNull()}, sizes={icon.availableSizes()}")
                self.icons[name] = icon
            else:
                logger.warning(f"Icon file not found: {icon_path}")
                self.icons[name] = QIcon()

    def _generate_icons(self, icons_dir: Path):
        """Generate icons for system tray.

        - idle/transcribing: ATT accent yellow
        - disconnected: muted gray
        - recording: warm orange-red to indicate active recording
        """
        from PIL import Image, ImageDraw

        # Use 88x88 for high-resolution display support (4x the typical 22x22)
        size = 88
        accent_yellow = (201, 162, 39, 255)
        disconnected_gray = (113, 113, 122, 200)
        recording_color = (230, 126, 34, 255)

        for name in ["idle", "transcribing", "disconnected"]:
            image = Image.new('RGBA', (size, size), (0, 0, 0, 0))
            draw = ImageDraw.Draw(image)
            center = size // 2
            radius = size // 2 - 2  # Minimal margin for complete circle

            draw.ellipse(
                [center - radius, center - radius, center + radius, center + radius],
                fill=accent_yellow if name != "disconnected" else disconnected_gray,
            )
            image.save(icons_dir / f"{name}.png")
            logger.info(f"Generated tray icon: {name} ({size}x{size})")

        image = Image.new('RGBA', (size, size), (0, 0, 0, 0))
        draw = ImageDraw.Draw(image)
        center = size // 2
        radius = size // 2 - 2
        draw.ellipse(
            [center - radius, center - radius, center + radius, center + radius],
            fill=recording_color,
        )
        image.save(icons_dir / "recording.png")
        logger.info(f"Generated recording icon: warm orange-red ({size}x{size})")

    def _build_menu(self):
        """Build the context menu with history at top level."""
        self.menu = QMenu()

        # History records will be inserted at top level dynamically
        # Placeholder separator (history items will be inserted above this)
        self._history_separator = self.menu.addSeparator()

        # Settings submenu
        self.settings_menu = self.menu.addMenu("设置")

        self.status_action = self.settings_menu.addAction("状态: 空闲")
        self.status_action.setEnabled(False)

        self.ws_action = self.settings_menu.addAction("WebSocket: 未连接")
        self.ws_action.setEnabled(False)

        self.hotkey_action = self.settings_menu.addAction("快捷键: ⌘⌥")
        self.hotkey_action.setEnabled(False)

        self.menu.addSeparator()

        # Actions
        refresh_action = self.menu.addAction("刷新历史")
        refresh_action.triggered.connect(self._refresh_history)

        reconnect_action = self.menu.addAction("重连")
        reconnect_action.triggered.connect(self._reconnect)

        quit_action = self.menu.addAction("退出")
        quit_action.triggered.connect(self._quit)

        self.setContextMenu(self.menu)

    def _build_history_items(self, records: List[dict]):
        """Build history items at menu top level."""
        self.records = records

        # Remove old history actions
        for action in self.history_actions:
            self.menu.removeAction(action)
        self.history_actions.clear()

        if not records:
            no_data = QAction("暂无录音", self.menu)
            no_data.setEnabled(False)
            self.menu.insertAction(self._history_separator, no_data)
            self.history_actions.append(no_data)
        else:
            # Insert history items before the separator
            for record in records[:10]:
                created_at = record.get('created_at', '')
                text = record.get('optimized_text') or record.get('raw_text', '')

                # Truncate preview
                preview = text[:30] + '...' if len(text) > 30 else text
                if not preview:
                    preview = "(空)"

                # Format time
                try:
                    dt = datetime.fromisoformat(created_at)
                    time_str = dt.strftime('%m-%d %H:%M')
                except:
                    time_str = created_at[:10] if len(created_at) >= 10 else created_at

                action_text = f"{time_str} | {preview}"
                action = QAction(action_text, self.menu)
                action.triggered.connect(lambda checked, t=text: self._copy_text(t))

                # Insert before separator (at top of menu)
                self.menu.insertAction(self._history_separator, action)
                self.history_actions.append(action)

    def _connect_signals(self):
        """Connect signals to slots."""
        # Will be connected after async worker is created
        pass

    def _connect_worker_signals(self, worker: AsyncWorker):
        """Connect async worker signals."""
        worker.status_updated.connect(self._on_status_updated)
        worker.connected.connect(self._on_connected)
        worker.disconnected.connect(self._on_disconnected)
        worker.hotkey_changed.connect(self._on_hotkey_changed)
        worker.web_recording.connect(self._on_web_recording)
        worker.settings_changed.connect(self._on_settings_changed)
        worker.error_occurred.connect(self._on_error)
        worker.transcription_completed.connect(self._on_transcription_completed)
        worker.optimization_completed.connect(self._on_optimization_completed)

    @Slot(bool, bool, bool)
    def _on_status_updated(self, recording: bool, transcribing: bool, connected: bool):
        """Handle status update."""
        self.is_recording = recording
        self.is_transcribing = transcribing
        self._update_icon()
        self._update_menu_text()

    @Slot()
    def _on_connected(self):
        """Handle WebSocket connected."""
        self.is_connected = True
        self._update_icon()
        self._update_menu_text()

    @Slot()
    def _on_disconnected(self):
        """Handle WebSocket disconnected."""
        self.is_connected = False
        self._update_icon()
        self._update_menu_text()

    @Slot(dict)
    def _on_hotkey_changed(self, hotkey_config: dict):
        """Handle hotkey configuration change."""
        self.hotkey_config = hotkey_config
        display = hotkey_config.get('display', '未设置')
        self.hotkey_action.setText(f"快捷键: {display}")

    @Slot(str)
    def _on_web_recording(self, action: str):
        """Handle web recording notification."""
        # Skip if recording was initiated by tray (mute is managed by MuteContext)
        if self._recording_initiated_by_tray:
            logger.debug(f"Skipping web recording handler for tray-initiated recording: {action}")
            return

        if not self.pause_audio_on_recording:
            return

        if action == "start":
            self.audio_controller.pause_all()
        elif action == "stop":
            self.audio_controller.resume_all()

    @Slot(dict)
    def _on_settings_changed(self, settings: dict):
        """Handle settings change."""
        pause_audio = settings.get("pause_audio_on_recording")
        if pause_audio is not None:
            self.pause_audio_on_recording = pause_audio
        auto_copy = settings.get("auto_copy_to_clipboard")
        if auto_copy is not None:
            self.auto_copy_to_clipboard = auto_copy
        auto_optimize = settings.get("auto_optimize")
        if auto_optimize is not None:
            self.auto_optimize = auto_optimize

    @Slot(str)
    def _on_error(self, error: str):
        """Handle error."""
        logger.error(f"Error: {error}")

    @Slot(dict)
    def _on_transcription_completed(self, data: dict):
        """Handle transcription completed - refresh history and optionally copy to clipboard."""
        logger.info(f"Transcription completed, refreshing history: {data.get('record_id')}")
        self._refresh_history()
        if data.get('electron_active', False):
            logger.info("Electron active, skipping tray copy/paste for transcription result")
            return
        # Only copy if auto_optimize is disabled (otherwise wait for optimization)
        if self.auto_copy_to_clipboard and not self.auto_optimize:
            text = data.get('text', '')
            if text:
                self._copy_and_paste(text)

    @Slot(dict)
    def _on_optimization_completed(self, data: dict):
        """Handle optimization completed - refresh history and copy optimized text."""
        logger.info(f"Optimization completed, refreshing history: {data.get('record_id')}")
        self._refresh_history()
        if data.get('electron_active', False):
            logger.info("Electron active, skipping tray copy/paste for optimized result")
            return
        # Copy optimized text
        if self.auto_copy_to_clipboard:
            text = data.get('optimized_text', '')
            if text:
                self._copy_and_paste(text)

    def _update_icon(self):
        """Update tray icon based on current state."""
        if not self.is_connected:
            icon_key = "disconnected"
        elif self.is_recording:
            icon_key = "recording"
        elif self.is_transcribing:
            icon_key = "transcribing"
        else:
            icon_key = "idle"

        self.setIcon(self.icons[icon_key])

    def _update_menu_text(self):
        """Update menu text."""
        status_text = "录音中" if self.is_recording else "空闲"
        self.status_action.setText(f"状态: {status_text}")

        ws_text = "已连接" if self.is_connected else "未连接"
        self.ws_action.setText(f"WebSocket: {ws_text}")

    def _copy_text(self, text: str, show_notification: bool = True):
        """Copy text to clipboard."""
        if not text:
            text = "(空)"
        clipboard = QApplication.clipboard()
        clipboard.setText(text)
        if show_notification:
            self.showMessage(
                "已复制",
                f"已复制到剪贴板: {text[:50]}{'...' if len(text) > 50 else ''}",
                QSystemTrayIcon.Information,
                2000
            )

    def _copy_and_paste(self, text: str):
        """Copy text to clipboard and simulate paste."""
        if not text:
            text = "(空)"

        # Copy to clipboard
        clipboard = QApplication.clipboard()
        clipboard.setText(text)

        # Simulate Ctrl+V / Cmd+V paste
        from pynput.keyboard import Controller, Key
        keyboard = Controller()
        if IS_MACOS:
            keyboard.press(Key.cmd)
            keyboard.press('v')
            keyboard.release('v')
            keyboard.release(Key.cmd)
        else:
            keyboard.press(Key.ctrl)
            keyboard.press('v')
            keyboard.release('v')
            keyboard.release(Key.ctrl)

        self.showMessage(
            "已复制并粘贴",
            f"已粘贴: {text[:30]}{'...' if len(text) > 30 else ''}",
            QSystemTrayIcon.Information,
            2000
        )

    def _refresh_history(self):
        """Refresh history records."""
        if self.history_fetcher and self.history_fetcher.isRunning():
            return

        self.history_fetcher = HistoryFetcher(self.server_url)
        self.history_fetcher.history_fetched.connect(self._build_history_items)
        self.history_fetcher.start()

    def _reconnect(self):
        """Manually reconnect WebSocket."""
        if self.async_worker and self.async_worker.loop and self.ws_client:
            asyncio.run_coroutine_threadsafe(
                self.ws_client.connect(),
                self.async_worker.loop
            )

    def _quit(self):
        """Quit the application."""
        logger.info("Quitting tray application")

        # Stop history timer
        if self.history_timer:
            self.history_timer.stop()

        # Stop hotkey listener
        if self.hotkey_listener:
            self.hotkey_listener.stop()

        # Stop async worker
        if self.async_worker:
            self.async_worker.stop()
            self.async_worker.wait(2000)

        # Quit application
        QCoreApplication.quit()

    def start(self):
        """Start the tray application."""
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

        # Create hotkey listener
        default_hotkey = get_default_hotkey()
        self.hotkey_listener = create_hotkey_listener(
            on_press=self._on_hotkey_press,
            on_release=self._on_hotkey_release,
            hotkey_config=default_hotkey
        )

        # Start async worker
        self.async_worker = AsyncWorker(self)
        self._connect_worker_signals(self.async_worker)

        # Set event loop for hotkey listener
        if hasattr(self.hotkey_listener, 'set_loop') and self.async_worker.loop:
            # Wait for loop to be created
            QTimer.singleShot(500, lambda: self._set_hotkey_loop())

        self.async_worker.start()

        # Start hotkey listener
        if not self.hotkey_listener.start():
            logger.error("Failed to start hotkey listener")

        # Start history refresh timer
        self.history_timer = QTimer()
        self.history_timer.timeout.connect(self._refresh_history)
        self.history_timer.start(30000)  # 30 seconds

        # Initial history refresh
        QTimer.singleShot(1000, self._refresh_history)

        # Show tray icon and verify
        current_icon = self.icon()
        logger.info(f"Current icon is null: {current_icon.isNull()}")
        logger.info(f"Current icon sizes: {current_icon.availableSizes()}")

        self.show()
        logger.info(f"Tray icon visible: {self.isVisible()}")
        logger.info(f"Tray icon geometry: {self.geometry()}")

        # Show startup message to verify tray functionality
        QTimer.singleShot(2000, lambda: self.showMessage(
            "ATT 托盘已启动",
            "点击图标查看菜单，使用快捷键开始录音",
            QSystemTrayIcon.Information,
            3000
        ))

    def _set_hotkey_loop(self):
        """Set event loop for hotkey listener after it's created."""
        if self.async_worker and self.async_worker.loop:
            if hasattr(self.hotkey_listener, 'set_loop'):
                self.hotkey_listener.set_loop(self.async_worker.loop)

    def _on_hotkey_press(self):
        """Handle hotkey press - called from pynput callback."""
        if self.async_worker and self.async_worker.loop:
            asyncio.run_coroutine_threadsafe(
                self.async_worker.on_hotkey_press(),
                self.async_worker.loop
            )

    def _on_hotkey_release(self):
        """Handle hotkey release - called from pynput callback."""
        if self.async_worker and self.async_worker.loop:
            asyncio.run_coroutine_threadsafe(
                self.async_worker.on_hotkey_release(),
                self.async_worker.loop
            )


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

    # Create Qt application
    app = QApplication(sys.argv)
    app.setQuitOnLastWindowClosed(False)  # Keep running when no windows

    # Create and start tray app
    tray_app = TrayApp(server_url=args.server)
    tray_app.start()

    # Setup signal handler for Ctrl+C
    def signal_handler(sig, frame):
        logger.info("Received interrupt signal (Ctrl+C)")
        tray_app._quit()
        app.quit()

    signal.signal(signal.SIGINT, signal_handler)

    # Run application
    sys.exit(app.exec())


if __name__ == "__main__":
    main()
