"""
WebSocket Connection Manager
Manages connections from the current frontend
"""
from fastapi import WebSocket
from typing import Dict, Optional, Any
from datetime import datetime
import asyncio

from app.utils.logging import get_logger

logger = get_logger(__name__)


class ConnectionManager:
    """Manage WebSocket connections and synchronized status state."""

    def __init__(self):
        self.active_connections: Dict[str, WebSocket] = {}
        self.current_status: Dict[str, Any] = {
            "recording": False,
            "transcribing": False,
            "record_id": None,
            "duration": 0.0,
            "connected_clients": 0
        }
        self._lock = asyncio.Lock()

    async def connect(self, websocket: WebSocket, client_id: str) -> None:
        await websocket.accept()

        async with self._lock:
            self.active_connections[client_id] = websocket
            self.current_status["connected_clients"] = len(self.active_connections)

        logger.info(
            "WebSocket client connected",
            client_id=client_id,
            total_clients=len(self.active_connections)
        )

        await self.send_to_client(client_id, {
            "type": "connected",
            "client_id": client_id,
            "status": self.current_status,
            "timestamp": int(datetime.now().timestamp() * 1000)
        })

        await self.broadcast_status()

    def disconnect(self, client_id: str) -> None:
        self.active_connections.pop(client_id, None)
        self.current_status["connected_clients"] = len(self.active_connections)

        logger.info(
            "WebSocket client disconnected",
            client_id=client_id,
            remaining_clients=len(self.active_connections)
        )

    async def send_to_client(self, client_id: str, message: dict) -> bool:
        if client_id in self.active_connections:
            try:
                await self.active_connections[client_id].send_json(message)
                return True
            except Exception as error:
                logger.error(
                    "Failed to send message to client",
                    client_id=client_id,
                    error=str(error)
                )
                return False
        return False

    async def broadcast(self, message: dict, exclude: Optional[str] = None) -> None:
        disconnected = []

        for client_id, connection in self.active_connections.items():
            if client_id == exclude:
                continue

            try:
                await connection.send_json(message)
            except Exception as error:
                logger.warning(
                    "Failed to broadcast to client, marking for disconnect",
                    client_id=client_id,
                    error=str(error)
                )
                disconnected.append(client_id)

        for client_id in disconnected:
            self.disconnect(client_id)

    async def broadcast_status(self) -> None:
        await self.broadcast({
            "type": "status",
            **self.current_status,
            "timestamp": int(datetime.now().timestamp() * 1000)
        })

    async def update_status(self, **kwargs) -> None:
        async with self._lock:
            self.current_status.update(kwargs)

        await self.broadcast_status()

    def get_active_web_client(self) -> Optional[str]:
        return next(iter(self.active_connections), None)

    @property
    def client_count(self) -> int:
        return len(self.active_connections)


manager = ConnectionManager()
