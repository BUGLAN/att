/**
 * WebSocket 客户端
 * 功能：连接管理、自动重连、消息处理、轮询降级
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

        // 新增：转录/优化状态回调
        this.onTranscriptionStarted = options.onTranscriptionStarted || (() => {});
        this.onTranscriptionCompleted = options.onTranscriptionCompleted || (() => {});
        this.onOptimizationStarted = options.onOptimizationStarted || (() => {});
        this.onOptimizationCompleted = options.onOptimizationCompleted || (() => {});
        this.onSegmentTranscribed = options.onSegmentTranscribed || (() => {});
        this.onErrorMessage = options.onErrorMessage || (() => {});

        // 心跳
        this.heartbeatInterval = null;
        this.heartbeatTimeout = 30000;  // 30秒

        // 重连定时器
        this.reconnectTimer = null;

        // 轮询降级
        this.fallbackToPolling = false;
        this.pollingInterval = null;
        this.currentRecordId = null;

        // 事件处理器映射
        this._handlers = {};
    }

    /**
     * 注册事件处理器
     * @param {string} event - 事件类型
     * @param {Function} handler - 处理函数
     */
    on(event, handler) {
        this._handlers[event] = handler;
    }

    /**
     * 触发事件
     * @param {string} event - 事件类型
     * @param {any} data - 事件数据
     */
    _emit(event, data) {
        const handler = this._handlers[event];
        if (handler) {
            handler(data);
        }
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

                // 启动轮询降级
                if (!this.fallbackToPolling) {
                    this.fallbackToPolling = true;
                    this._startPolling();
                }

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
        this._emit(data.type, data.data || data);

        switch (data.type) {
            case 'connected':
                console.log('Client connected:', data.client_id);
                this.clientId = data.client_id;
                // 停止轮询降级
                this._stopPolling();
                this.fallbackToPolling = false;
                break;

            case 'recording_command':
                console.log('Received recording command:', data.action);
                this.onRecordingCommand(data);
                break;

            case 'status':
                this.onStatusUpdate(data);
                break;

            // 新增：转录状态通知
            case 'transcription_started':
                console.log('Transcription started:', data.data?.record_id);
                this.onTranscriptionStarted(data.data);
                break;

            case 'transcription_completed':
                console.log('Transcription completed:', data.data?.record_id);
                this.onTranscriptionCompleted(data.data);
                break;

            case 'optimization_started':
                console.log('Optimization started:', data.data?.record_id);
                this.onOptimizationStarted(data.data);
                break;

            case 'optimization_completed':
                console.log('Optimization completed:', data.data?.record_id);
                this.onOptimizationCompleted(data.data);
                break;

            case 'segment_transcribed':
                console.log('Segment transcribed:', data.data?.segment_id);
                this.onSegmentTranscribed(data.data);
                break;

            case 'error':
                console.error('Server error:', data.data);
                this.onErrorMessage(data.data);
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

    /**
     * 设置当前监听的记录ID
     * @param {string} recordId - 记录ID
     */
    setCurrentRecord(recordId) {
        this.currentRecordId = recordId;
    }

    /**
     * 启动轮询降级
     * @private
     */
    _startPolling() {
        if (this.pollingInterval) return;

        console.log('Starting polling fallback...');
        this.pollingInterval = setInterval(async () => {
            if (!this.currentRecordId) return;

            try {
                const response = await fetch(`/api/records/${this.currentRecordId}/status`);
                const status = await response.json();

                // 模拟 WebSocket 消息
                if (status.status === 'transcribing') {
                    this.onTranscriptionStarted({ record_id: this.currentRecordId });
                } else if (status.status === 'transcribed') {
                    this.onTranscriptionCompleted({
                        record_id: this.currentRecordId,
                        text: status.raw_text,
                        processing_time: status.processing_time
                    });
                } else if (status.status === 'optimizing') {
                    this.onOptimizationStarted({ record_id: this.currentRecordId });
                } else if (status.status === 'optimized') {
                    // 获取完整记录
                    const fullResponse = await fetch(`/api/records/${this.currentRecordId}`);
                    const record = await fullResponse.json();
                    this.onOptimizationCompleted({
                        record_id: this.currentRecordId,
                        optimized_text: record.optimized_text
                    });
                } else if (status.status === 'failed') {
                    this.onErrorMessage({
                        record_id: this.currentRecordId,
                        error: 'Transcription failed'
                    });
                }
            } catch (error) {
                console.error('Polling error:', error);
            }
        }, 2000);
    }

    /**
     * 停止轮询
     * @private
     */
    _stopPolling() {
        if (this.pollingInterval) {
            clearInterval(this.pollingInterval);
            this.pollingInterval = null;
            console.log('Polling stopped');
        }
    }
}

window.WebSocketClient = WebSocketClient;
