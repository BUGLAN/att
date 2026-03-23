/**
 * WebSocket 客户端
 * 功能：连接管理、自动重连、消息处理
 */
class WebSocketClient {
    constructor(options = {}) {
        this.url = options.url || this._buildUrl();
        this.reconnectInterval = options.reconnectInterval || 3000;
        this.maxReconnectAttempts = options.maxReconnectAttempts || 5;
        this.reconnectAttempts = 0;
        this.isConnected = false;
        this.socket = null;
        this.clientId = null;

        // 回调函数
        this.onConnect = options.onConnect || (() => {});
        this.onDisconnect = options.onDisconnect || (() => {});
        this.onMessage = options.onMessage || (() => {});
        this.onError = options.onError || (() => {});
        this.onRecordingCommand = options.onRecordingCommand || (() => {});
        this.onStatusUpdate = options.onStatusUpdate || (() => {});

        // 心跳
        this.heartbeatInterval = null;
        this.heartbeatTimeout = 30000;  // 30秒

        // 重连定时器
        this.reconnectTimer = null;
    }

    /**
     * 构建 WebSocket URL
     */
    _buildUrl() {
        const protocol = window.location.protocol === 'https:' ? 'wss:' : 'ws:';
        return `${protocol}//${window.location.host}/api/ws/web`;
    }

    /**
     * 建立连接
     */
    connect() {
        if (this.socket && this.socket.readyState === WebSocket.OPEN) {
            console.log('WebSocket already connected');
            return;
        }

        // 清理旧连接
        this._cleanup();

        console.log('Connecting to WebSocket:', this.url);

        try {
            this.socket = new WebSocket(this.url);

            this.socket.onopen = () => {
                console.log('WebSocket connected');
                this.isConnected = true;
                this.reconnectAttempts = 0;
                this._startHeartbeat();
                this.onConnect();
            };

            this.socket.onclose = (event) => {
                console.log('WebSocket closed:', event.code, event.reason);
                this.isConnected = false;
                this.clientId = null;
                this._stopHeartbeat();
                this.onDisconnect();
                this._attemptReconnect();
            };

            this.socket.onerror = (error) => {
                console.error('WebSocket error:', error);
                this.onError(error);
            };

            this.socket.onmessage = (event) => {
                try {
                    const data = JSON.parse(event.data);
                    this._handleMessage(data);
                } catch (e) {
                    console.error('Failed to parse WebSocket message:', e);
                }
            };

        } catch (error) {
            console.error('Failed to create WebSocket:', error);
            this._attemptReconnect();
        }
    }

    /**
     * 处理接收到的消息
     */
    _handleMessage(data) {
        this.onMessage(data);

        switch (data.type) {
            case 'connected':
                console.log('Client connected:', data.client_id);
                this.clientId = data.client_id;
                break;

            case 'recording_command':
                console.log('Received recording command:', data.action);
                this.onRecordingCommand(data);
                break;

            case 'status':
                this.onStatusUpdate(data);
                break;

            case 'pong':
                // 心跳响应，无需处理
                break;

            default:
                console.log('Unknown message type:', data.type);
        }
    }

    /**
     * 发送消息
     */
    send(data) {
        if (this.socket && this.socket.readyState === WebSocket.OPEN) {
            this.socket.send(JSON.stringify(data));
            return true;
        }
        console.warn('WebSocket not connected, cannot send message');
        return false;
    }

    /**
     * 发送录音控制命令
     * @param {string} action - 'start' 或 'stop'
     */
    sendRecordingCommand(action) {
        return this.send({
            type: 'recording',
            action: action,
            timestamp: Date.now()
        });
    }

    /**
     * 发送状态更新
     * @param {Object} status - 状态对象
     */
    sendStatusUpdate(status) {
        return this.send({
            type: 'status_update',
            ...status,
            timestamp: Date.now()
        });
    }

    /**
     * 启动心跳
     */
    _startHeartbeat() {
        this._stopHeartbeat();

        this.heartbeatInterval = setInterval(() => {
            if (this.isConnected) {
                this.send({ type: 'ping' });
            }
        }, this.heartbeatTimeout);
    }

    /**
     * 停止心跳
     */
    _stopHeartbeat() {
        if (this.heartbeatInterval) {
            clearInterval(this.heartbeatInterval);
            this.heartbeatInterval = null;
        }
    }

    /**
     * 尝试重连
     */
    _attemptReconnect() {
        if (this.reconnectAttempts >= this.maxReconnectAttempts) {
            console.log('Max reconnect attempts reached');
            return;
        }

        // 清理旧的重连定时器
        if (this.reconnectTimer) {
            clearTimeout(this.reconnectTimer);
        }

        this.reconnectAttempts++;
        const delay = this.reconnectInterval * Math.pow(1.5, this.reconnectAttempts - 1);

        console.log(`Reconnecting in ${delay}ms... (${this.reconnectAttempts}/${this.maxReconnectAttempts})`);

        this.reconnectTimer = setTimeout(() => {
            this.connect();
        }, delay);
    }

    /**
     * 清理资源
     */
    _cleanup() {
        this._stopHeartbeat();

        if (this.reconnectTimer) {
            clearTimeout(this.reconnectTimer);
            this.reconnectTimer = null;
        }

        if (this.socket) {
            // 移除事件监听器，避免重复触发
            this.socket.onopen = null;
            this.socket.onclose = null;
            this.socket.onerror = null;
            this.socket.onmessage = null;

            if (this.socket.readyState === WebSocket.OPEN) {
                this.socket.close();
            }
            this.socket = null;
        }
    }

    /**
     * 主动断开连接
     */
    disconnect() {
        console.log('Disconnecting WebSocket...');
        this._cleanup();
        this.isConnected = false;
        this.clientId = null;
    }

    /**
     * 重置重连计数（连接成功后调用）
     */
    resetReconnectAttempts() {
        this.reconnectAttempts = 0;
    }
}

window.WebSocketClient = WebSocketClient;
