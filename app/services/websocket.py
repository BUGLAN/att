"""
WebSocket Connection Manager
Manages connections from tray tools and web frontend
"""
from fastapi import WebSocket
from typing import Dict, Set, Optional, Any
from datetime import datetime
import json
import asyncio

from app.utils.logging import get_logger

logger = get_logger(__name__)


class ConnectionManager:
    """
    Manages WebSocket connections and message broadcasting.

    Supports multiple client types:
    - "web": Browser frontend
    - "tray": System tray tool

    Features:
    - Connection management with unique client IDs
    - Broadcast messages to all or specific clients
    - Status tracking and synchronization
    """

    def __init__(self):
        # 客户端连接：{client_id: websocket}
        self.active_connections: Dict[str, WebSocket] = {}

        # 客户端类型：{client_id: "tray" | "web"}
        self.client_types: Dict[str, str] = {}

        # 当前状态
        self.current_status: Dict[str, Any] = {
            "recording": False,
            "transcribing": False,
            "record_id": None,
            "duration": 0.0,
            "connected_clients": 0
        }

        # 状态锁（防止并发更新）
        self._lock = asyncio.Lock()

    async def connect(
        self,
        websocket: WebSocket,
        client_id: str,
        client_type: str = "web"
    ) -> None:
        """
        Accept new WebSocket connection.

        Args:
            websocket: WebSocket connection
            client_id: Unique client identifier
            client_type: Client type ("web" or "tray")
        """
        await websocket.accept()

        async with self._lock:
            self.active_connections[client_id] = websocket
            self.client_types[client_id] = client_type
            self.current_status["connected_clients"] = len(self.active_connections)

        logger.info(
            "WebSocket client connected",
            client_id=client_id,
            client_type=client_type,
            total_clients=len(self.active_connections)
        )

        # 发送当前状态给新客户端
        await self.send_to_client(client_id, {
            "type": "connected",
            "client_id": client_id,
            "client_type": client_type,
            "status": self.current_status,
            "timestamp": int(datetime.now().timestamp() * 1000)
        })

        # 广播客户端数量更新
        await self.broadcast_status()

    def disconnect(self, client_id: str) -> None:
        """
        Remove disconnected client.

        Args:
            client_id: Client identifier to remove
        """
        self.active_connections.pop(client_id, None)
        self.client_types.pop(client_id, None)
        self.current_status["connected_clients"] = len(self.active_connections)

        logger.info(
            "WebSocket client disconnected",
            client_id=client_id,
            remaining_clients=len(self.active_connections)
        )

    async def send_to_client(self, client_id: str, message: dict) -> bool:
        """
        Send message to specific client.

        Args:
            client_id: Target client identifier
            message: Message dictionary to send

        Returns:
            True if sent successfully, False otherwise
        """
        if client_id in self.active_connections:
            try:
                await self.active_connections[client_id].send_json(message)
                return True
            except Exception as e:
                logger.error(
                    "Failed to send message to client",
                    client_id=client_id,
                    error=str(e)
                )
                return False
        return False

    async def broadcast(
        self,
        message: dict,
        exclude: Optional[str] = None
    ) -> None:
        """
        Broadcast message to all connected clients.

        Args:
            message: Message dictionary to broadcast
            exclude: Optional client_id to exclude from broadcast
        """
        disconnected = []

        for client_id, connection in self.active_connections.items():
            if client_id == exclude:
                continue

            try:
                await connection.send_json(message)
            except Exception as e:
                logger.warning(
                    "Failed to broadcast to client, marking for disconnect",
                    client_id=client_id,
                    error=str(e)
                )
                disconnected.append(client_id)

        # 清理断开的连接
        for client_id in disconnected:
            self.disconnect(client_id)

    async def broadcast_status(self) -> None:
        """Broadcast current status to all clients."""
        await self.broadcast({
            "type": "status",
            **self.current_status,
            "timestamp": int(datetime.now().timestamp() * 1000)
        })

    async def update_status(self, **kwargs) -> None:
        """
        Update status and broadcast to all clients.

        Args:
            **kwargs: Status fields to update
        """
        async with self._lock:
            self.current_status.update(kwargs)

        await self.broadcast_status()

    def get_tray_clients(self) -> Set[str]:
        """Get all tray tool client IDs."""
        return {
            cid for cid, ctype in self.client_types.items()
            if ctype == "tray"
        }

    def get_web_clients(self) -> Set[str]:
        """Get all web frontend client IDs."""
        return {
            cid for cid, ctype in self.client_types.items()
            if ctype == "web"
        }

    def get_client_type(self, client_id: str) -> Optional[str]:
        """Get client type by ID."""
        return self.client_types.get(client_id)

    @property
    def client_count(self) -> int:
        """Get total number of connected clients."""
        return len(self.active_connections)


# 全局连接管理器实例
manager = ConnectionManager()
