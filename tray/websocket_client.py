"""
WebSocket Client for Tray Tool
Connects to FastAPI backend to send recording commands
"""
import asyncio
import json
from typing import Optional, Callable
from datetime import datetime

try:
    import websockets
except ImportError:
    websockets = None

import logging

logger = logging.getLogger(__name__)


class TrayWebSocketClient:
    """
    WebSocket client for tray tool communication.

    Features:
    - Connect to FastAPI WebSocket endpoint
    - Send recording commands
    - Receive status updates
    - Auto-reconnect on disconnect
    """

    def __init__(
        self,
        url: str = "ws://localhost:8000/api/ws/tray",
        on_status_update: Optional[Callable] = None,
        on_connected: Optional[Callable] = None,
        on_disconnected: Optional[Callable] = None
    ):
        """
        Initialize WebSocket client.

        Args:
            url: WebSocket server URL
            on_status_update: Callback for status updates
            on_connected: Callback when connected
            on_disconnected: Callback when disconnected
        """
        if websockets is None:
            raise ImportError("websockets package required: pip install websockets")

        self.url = url
        self.websocket: Optional[websockets.WebSocketClientProtocol] = None
        self.client_id: Optional[str] = None
        self.is_connected = False

        # Callbacks
        self.on_status_update = on_status_update
        self.on_connected = on_connected
        self.on_disconnected = on_disconnected

        # Reconnection settings
        self.reconnect_interval = 3.0
        self.max_reconnect_attempts = 10
        self.reconnect_attempts = 0

        # Control
        self._running = False
        self._receive_task: Optional[asyncio.Task] = None

    async def connect(self) -> bool:
        """
        Connect to WebSocket server.

        Returns:
            True if connected successfully
        """
        try:
            logger.info(f"Connecting to {self.url}...")
            self.websocket = await websockets.connect(self.url)
            self.is_connected = True
            self.reconnect_attempts = 0
            self._running = True

            logger.info("WebSocket connected, starting receive loop...")
            if self.on_connected:
                self.on_connected()

            # Start receiving messages
            self._receive_task = asyncio.create_task(self._receive_loop())
            logger.info(f"Receive task created: {self._receive_task}")

            return True

        except Exception as e:
            logger.error(f"Failed to connect: {type(e).__name__}: {e}")
            self.is_connected = False
            return False

    async def disconnect(self) -> None:
        """Disconnect from WebSocket server."""
        self._running = False

        if self._receive_task:
            self._receive_task.cancel()
            self._receive_task = None

        if self.websocket:
            await self.websocket.close()
            self.websocket = None

        self.is_connected = False
        logger.info("WebSocket disconnected")

        if self.on_disconnected:
            self.on_disconnected()

    async def _receive_loop(self) -> None:
        """Receive messages from WebSocket."""
        logger.info("Receive loop started")
        try:
            async for message in self.websocket:
                logger.debug(f"Received message: {message[:100]}...")
                try:
                    data = json.loads(message)
                    await self._handle_message(data)
                except json.JSONDecodeError as e:
                    logger.error(f"Failed to parse message: {e}")
        except websockets.exceptions.ConnectionClosed as e:
            logger.info(f"Connection closed by server: code={e.code}, reason={e.reason}")
        except Exception as e:
            logger.error(f"Receive error: {type(e).__name__}: {e}")
        finally:
            logger.info("Receive loop ended")
            self.is_connected = False
            if self.on_disconnected:
                self.on_disconnected()

            # Attempt reconnect if still running
            if self._running:
                await self._attempt_reconnect()

    async def _handle_message(self, data: dict) -> None:
        """Handle incoming message."""
        msg_type = data.get("type")

        if msg_type == "connected":
            self.client_id = data.get("client_id")
            logger.info(f"Client connected with ID: {self.client_id}")

            # Send initial status update
            if self.on_status_update:
                status = data.get("status", {})
                self.on_status_update(status)

        elif msg_type == "status":
            if self.on_status_update:
                self.on_status_update(data)

        elif msg_type == "pong":
            # Heartbeat response
            pass

    async def _attempt_reconnect(self) -> None:
        """Attempt to reconnect after disconnect."""
        while self._running and self.reconnect_attempts < self.max_reconnect_attempts:
            self.reconnect_attempts += 1
            logger.info(f"Reconnecting... ({self.reconnect_attempts}/{self.max_reconnect_attempts})")

            await asyncio.sleep(self.reconnect_interval)

            if await self.connect():
                return

        logger.error("Max reconnect attempts reached")
        self._running = False

    async def send_recording_command(self, action: str) -> bool:
        """
        Send recording command to server.

        Args:
            action: "start" or "stop"

        Returns:
            True if sent successfully
        """
        if not self.is_connected or not self.websocket:
            logger.warning("Not connected, cannot send command")
            return False

        try:
            message = {
                "type": "recording",
                "action": action,
                "timestamp": int(datetime.now().timestamp() * 1000)
            }
            await self.websocket.send(json.dumps(message))
            logger.info(f"Sent recording command: {action}")
            return True

        except Exception as e:
            logger.error(f"Failed to send command: {e}")
            return False

    async def send_ping(self) -> bool:
        """Send ping for heartbeat."""
        if not self.is_connected or not self.websocket:
            return False

        try:
            await self.websocket.send(json.dumps({"type": "ping"}))
            return True
        except Exception as e:
            logger.error(f"Failed to send ping: {e}")
            return False
