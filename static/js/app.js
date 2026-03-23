/**
 * 主应用逻辑
 */
class App {
    constructor() {
        // 状态
        this.currentRecordId = null;  // 当前录音的 record ID
        this.records = [];             // 所有记录列表
        this.pendingRecords = [];      // 临时记录列表（乐观更新）
        this.isRecording = false;
        this.isTranscribing = false;   // 转写中状态
        this.selectMode = false;       // 选择模式
        this.selectedRecords = new Set(); // 选中的记录 ID
        this.recordingStartTime = null; // 录音开始时间
        this.pollingIntervals = new Map(); // 轮询管理
        this.processingTimes = new Map(); // 记录转录耗时

        // WebSocket 客户端
        this.wsClient = null;

        // 录音器
        this.recorder = new AudioRecorder({
            onRecordingStart: () => this.handleRecordingStart(),
            onRecordingStop: () => this.handleRecordingStop(),
            onUploadStarted: () => this.handleUploadStarted(),
            onUploadComplete: (result) => this.handleUploadComplete(result),
            onError: (error) => this.handleError(error)
        });

        // DOM 元素
        this.elements = {
            recordBtn: document.getElementById('recordBtn'),
            recordingStatus: document.getElementById('recordingStatus'),
            textDisplay: document.getElementById('textDisplay'),
            recordsList: document.getElementById('recordsList'),
            recordCount: document.getElementById('recordCount'),
            optimizeBtn: document.getElementById('optimizeBtn'),
            copyBtn: document.getElementById('copyBtn'),
            duration: document.getElementById('duration'),
            toast: document.getElementById('toast'),
            transcriptionTime: document.getElementById('transcriptionTime'),
            settingsPopup: document.getElementById('settingsPopup'),
            headerSettingsBtn: document.getElementById('headerSettingsBtn'),
            selectModeBtn: document.getElementById('selectModeBtn'),
            selectToolbar: document.getElementById('selectToolbar'),
            selectAllCheckbox: document.getElementById('selectAllCheckbox'),
            deleteSelectedBtn: document.getElementById('deleteSelectedBtn'),
            cancelSelectBtn: document.getElementById('cancelSelectBtn'),
            // 侧边栏元素
            sidebar: document.getElementById('sidebar'),
            sidebarToggle: document.getElementById('sidebarToggle'),
            connectionStatus: document.getElementById('connectionStatus'),
            clientCount: document.getElementById('clientCount'),
            clientCountNum: document.getElementById('clientCountNum'),
            // 页面元素
            mainContent: document.getElementById('mainContent'),
            settingsPage: document.getElementById('settingsPage')
        };

        // 当前播放的音频
        this.currentAudio = null;

        // 绑定事件
        this.bindEvents();

        // 初始化 WebSocket
        this.initWebSocket();

        // 初始化侧边栏
        this.initSidebar();

        // 加载历史记录
        this.fetchRecords();

        // 页面卸载时清理
        window.addEventListener('beforeunload', () => {
            this.stopAllPolling();
            if (this.wsClient) {
                this.wsClient.disconnect();
            }
        });
    }

    /**
     * 初始化 WebSocket 客户端
     */
    initWebSocket() {
        this.wsClient = new WebSocketClient({
            onConnect: () => this.handleWSConnect(),
            onDisconnect: () => this.handleWSDisconnect(),
            onRecordingCommand: (data) => this.handleRecordingCommand(data),
            onStatusUpdate: (data) => this.handleStatusUpdate(data),
            onError: (error) => console.error('WebSocket error:', error)
        });

        this.wsClient.connect();
    }

    /**
     * 初始化侧边栏
     */
    initSidebar() {
        // 侧边栏折叠
        this.elements.sidebarToggle.addEventListener('click', () => {
            this.elements.sidebar.classList.toggle('collapsed');
        });

        // 导航项点击
        document.querySelectorAll('.nav-item').forEach(item => {
            item.addEventListener('click', (e) => {
                e.preventDefault();
                const page = item.dataset.page;
                this.switchPage(page);

                // 更新活动状态
                document.querySelectorAll('.nav-item').forEach(i => i.classList.remove('active'));
                item.classList.add('active');
            });
        });
    }

    /**
     * 切换页面
     */
    switchPage(page) {
        console.log('Switch to page:', page);

        if (page === 'settings') {
            // 显示设置页面
            this.elements.mainContent.style.display = 'none';
            this.elements.settingsPage.style.display = 'block';

            // 加载设置
            if (window.settingsManager) {
                window.settingsManager.load();
            }
        } else {
            // 显示首页
            this.elements.mainContent.style.display = 'block';
            this.elements.settingsPage.style.display = 'none';
        }
    }

    /**
     * WebSocket 连接成功
     */
    handleWSConnect() {
        const indicator = this.elements.connectionStatus.querySelector('.status-indicator');
        const text = this.elements.connectionStatus.querySelector('.status-text');
        indicator.classList.remove('disconnected');
        indicator.classList.add('connected');
        text.textContent = '已连接';
    }

    /**
     * WebSocket 断开连接
     */
    handleWSDisconnect() {
        const indicator = this.elements.connectionStatus.querySelector('.status-indicator');
        const text = this.elements.connectionStatus.querySelector('.status-text');
        indicator.classList.remove('connected');
        indicator.classList.add('disconnected');
        text.textContent = '未连接';

        // 隐藏客户端数量
        this.elements.clientCount.style.display = 'none';
    }

    /**
     * 收到录音命令
     */
    async handleRecordingCommand(data) {
        console.log('Received recording command:', data.action);

        if (data.action === 'start' && !this.isRecording) {
            await this.startRecording();
        } else if (data.action === 'stop' && this.isRecording) {
            await this.stopRecording();
        }
    }

    /**
     * 状态更新
     */
    handleStatusUpdate(data) {
        // 更新客户端数量显示
        if (data.connected_clients !== undefined) {
            if (data.connected_clients > 1) {
                this.elements.clientCount.style.display = 'flex';
                this.elements.clientCountNum.textContent = data.connected_clients;
            } else {
                this.elements.clientCount.style.display = 'none';
            }
        }
    }

    /**
     * 广播状态到 WebSocket
     */
    broadcastStatus(status = {}) {
        if (this.wsClient && this.wsClient.isConnected) {
            this.wsClient.sendStatusUpdate({
                recording: this.isRecording,
                transcribing: this.isTranscribing,
                record_id: this.currentRecordId,
                duration: this.recorder ? this.recorder.getDuration() : 0,
                ...status
            });
        }
    }

    /**
     * 绑定事件监听
     */
    bindEvents() {
        // 录音按钮
        this.elements.recordBtn.addEventListener('click', () => this.toggleRecording());

        // 优化按钮
        this.elements.optimizeBtn.addEventListener('click', () => this.optimizeCurrentRecord());

        // 复制按钮
        this.elements.copyBtn.addEventListener('click', () => this.copyText());

        // 双击复制
        this.elements.textDisplay.addEventListener('dblclick', () => this.copyText());

        // 文本编辑保存
        this.elements.textDisplay.addEventListener('blur', () => this.saveText());

        // 设置弹窗事件
        this.elements.headerSettingsBtn.addEventListener('click', (e) => {
            e.stopPropagation();
            this.showSettingsPopup(this.elements.headerSettingsBtn);
        });
        this.elements.selectModeBtn.addEventListener('click', () => this.enterSelectMode());

        // 选择模式工具栏事件
        this.elements.selectAllCheckbox.addEventListener('change', (e) => this.toggleSelectAll(e.target.checked));
        this.elements.deleteSelectedBtn.addEventListener('click', () => this.deleteSelectedRecords());
        this.elements.cancelSelectBtn.addEventListener('click', () => this.exitSelectMode());

        // 点击外部关闭设置弹窗
        document.addEventListener('click', (e) => {
            if (!e.target.closest('.settings-popup') && !e.target.closest('.settings-btn')) {
                this.hideSettingsPopup();
            }
        });

        // ESC 关闭弹窗
        document.addEventListener('keydown', (e) => {
            if (e.key === 'Escape') {
                this.hideSettingsPopup();
                this.exitSelectMode();
            }
        });
    }

    /**
     * 切换录音状态
     */
    async toggleRecording() {
        if (this.isRecording) {
            await this.stopRecording();
        } else {
            await this.startRecording();
        }
    }

    /**
     * 开始录音
     */
    async startRecording() {
        // 清空当前记录
        this.currentRecordId = null;
        this.elements.textDisplay.value = '';
        this.recordingStartTime = Date.now();
        this.elements.transcriptionTime.textContent = '耗时: 0.00s';

        // 开始录音
        await this.recorder.start();
    }

    /**
     * 停止录音
     */
    async stopRecording() {
        // 更新 UI 状态
        this.elements.recordBtn.classList.remove('recording');
        this.elements.recordBtn.querySelector('.btn-text').textContent = '开始录音';
        this.elements.recordingStatus.classList.remove('recording');
        this.elements.recordingStatus.querySelector('.status-text').textContent = '上传中...';
        // 不禁用按钮，允许立即开始新录音

        await this.recorder.stop();

        // 立即恢复状态，允许新录音
        this.elements.recordingStatus.querySelector('.status-text').textContent = '准备就绪';
        this.updateButtonStates();
    }

    /**
     * 录音开始回调
     */
    handleRecordingStart() {
        this.isRecording = true;

        // 更新 UI
        this.elements.recordBtn.classList.add('recording');
        this.elements.recordBtn.querySelector('.btn-text').textContent = '停止录音';
        this.elements.recordingStatus.classList.add('recording');
        this.elements.recordingStatus.querySelector('.status-text').textContent = '正在录音...';

        // 广播状态
        this.broadcastStatus({ recording: true });
    }

    /**
     * 录音停止回调
     */
    handleRecordingStop() {
        this.isRecording = false;

        // 乐观更新：立即添加临时记录到列表
        const tempRecord = {
            id: 'temp_' + Date.now(),
            isTemp: true,  // 标记为临时记录
            status: 'pending',
            duration: this.recorder.getDuration(),
            created_at: new Date().toISOString(),
            raw_text: '',
            optimized_text: ''
        };
        this.pendingRecords.unshift(tempRecord);
        this.renderRecords();  // 立即渲染

        // 广播状态
        this.broadcastStatus({ recording: false });
    }

    /**
     * 上传开始回调（上传发起时触发，不等待转录）
     */
    handleUploadStarted() {
        // 清空文本显示，准备接收新录音
        this.elements.textDisplay.value = '';
        this.recordingStartTime = Date.now();
        this.elements.transcriptionTime.textContent = '耗时: 0.00s';
        // 注意：不在这里 fetchRecords()，因为记录还没创建
        // 临时记录已在 handleRecordingStop() 中添加
    }

    /**
     * 上传完成回调（上传完成，但转录可能还在进行）
     */
    async handleUploadComplete(result) {
        console.log('Upload complete:', result);
        this.currentRecordId = result.record_id;

        // 移除最早的临时记录（乐观更新完成）
        const tempIndex = this.pendingRecords.findIndex(r => r.isTemp);
        if (tempIndex !== -1) {
            this.pendingRecords.splice(tempIndex, 1);
        }

        // 刷新记录列表，显示"转写中"状态
        await this.fetchRecords();

        // 如果转录还在进行中，开始轮询
        if (result.transcription_status === 'transcribing' ||
            result.transcription_status === 'pending') {
            this.startPolling(result.record_id);
        } else if (result.transcription_status === 'transcribed' && result.raw_text) {
            // 如果已经转录完成（快速返回），直接显示
            this.elements.textDisplay.value = result.raw_text || '';
            this.updateButtonStates();
        }

        this.showToast('上传完成，正在转写...');
    }

    /**
     * 获取记录列表
     */
    async fetchRecords() {
        try {
            const response = await fetch('/api/records?limit=100');
            this.records = await response.json();
            this.renderRecords();
        } catch (error) {
            console.error('Failed to fetch records:', error);
        }
    }

    /**
     * 渲染记录列表
     */
    renderRecords() {
        // 合并临时记录和真实记录（临时记录在前）
        const allRecords = [...this.pendingRecords, ...this.records];

        // 更新记录数量
        this.elements.recordCount.textContent = `${allRecords.length} 条记录`;

        if (allRecords.length === 0) {
            this.elements.recordsList.innerHTML = '<p class="placeholder">暂无录音记录</p>';
            return;
        }

        // 渲染记录卡片
        this.elements.recordsList.innerHTML = allRecords.map(r => {
            const isTranscribing = r.status === 'pending' || r.status === 'transcribing';
            const isCompleted = r.status === 'transcribed' || r.status === 'optimized';
            const isFailed = r.status === 'failed';

            // 状态显示
            let statusBadge = '';
            if (isTranscribing) {
                statusBadge = '<span class="record-status-badge transcribing"><span class="spinner-small"></span>转写中</span>';
            } else if (isFailed) {
                statusBadge = '<span class="record-status-badge failed">转写失败</span>';
            }

            // 文本内容
            let textContent = '正在转写...';
            if (isCompleted) {
                textContent = this.escapeHtml(r.optimized_text || r.raw_text || '');
            } else if (isFailed) {
                textContent = '转写失败，请重试';
            }

            return `
            <div class="record-item ${this.selectMode ? 'select-mode' : ''}" data-id="${r.id}">
                <input type="checkbox"
                       class="record-checkbox"
                       data-id="${r.id}"
                       ${this.selectedRecords.has(r.id) ? 'checked' : ''}
                       style="display: ${this.selectMode ? 'block' : 'none'};">
                <div class="record-audio">
                    ${isTranscribing ? `
                        <span class="transcribing-spinner" title="转写中"></span>
                    ` : `
                        <button class="audio-play-btn" data-id="${r.id}" title="试听" ${isFailed ? 'disabled' : ''}>
                            <span class="play-icon">▶</span>
                        </button>
                    `}
                </div>
                <div class="record-content">
                    <div class="record-text">${textContent}</div>
                    ${statusBadge}
                </div>
                <div class="record-meta">
                    <div class="record-times">
                        <span class="record-duration" title="音频时长">${this.formatDuration(r.duration)}</span>
                        ${this.processingTimes.has(r.id) ? `
                            <span class="record-processing" title="转录耗时">⏱${this.processingTimes.get(r.id).toFixed(1)}s</span>
                        ` : ''}
                        <span class="record-time" title="生成时间">${this.formatDateTime(r.created_at)}</span>
                    </div>
                    ${isCompleted ? `
                        <div class="record-actions">
                            <button class="btn-record-action btn-optimize" data-id="${r.id}" title="AI优化文本">✨优化</button>
                            <button class="btn-record-action btn-copy" data-id="${r.id}" title="复制文本">复制</button>
                        </div>
                    ` : ''}
                </div>
            </div>
        `}).join('');

        // 绑定事件
        this.bindRecordEvents();
    }

    /**
     * 绑定记录项事件
     */
    bindRecordEvents() {
        // 点击记录项复制文本（非选择模式）
        this.elements.recordsList.querySelectorAll('.record-item').forEach(item => {
            item.addEventListener('click', (e) => {
                // 忽略播放按钮和设置按钮点击
                if (e.target.closest('.audio-play-btn') ||
                    e.target.closest('.settings-btn') ||
                    e.target.closest('.record-checkbox') ||
                    e.target.closest('.btn-record-action')) return;

                const recordId = item.dataset.id;

                if (this.selectMode) {
                    // 选择模式下切换选中状态
                    this.toggleRecordSelection(recordId);
                } else {
                    // 复制文本
                    this.copyRecordText(recordId);
                }
            });
        });

        // 复选框事件
        this.elements.recordsList.querySelectorAll('.record-checkbox').forEach(checkbox => {
            checkbox.addEventListener('change', (e) => {
                e.stopPropagation();
                const recordId = checkbox.dataset.id;
                this.toggleRecordSelection(recordId);
            });
        });

        // 播放按钮
        this.elements.recordsList.querySelectorAll('.audio-play-btn').forEach(btn => {
            btn.addEventListener('click', (e) => {
                e.stopPropagation();
                const recordId = btn.dataset.id;
                this.toggleAudioPlayback(recordId, btn);
            });
        });

        // 优化按钮
        this.elements.recordsList.querySelectorAll('.btn-optimize').forEach(btn => {
            btn.addEventListener('click', async (e) => {
                e.stopPropagation();
                const recordId = btn.dataset.id;
                await this.optimizeRecord(recordId);
            });
        });

        // 复制按钮
        this.elements.recordsList.querySelectorAll('.btn-copy').forEach(btn => {
            btn.addEventListener('click', async (e) => {
                e.stopPropagation();
                const recordId = btn.dataset.id;
                await this.copyRecordText(recordId);
            });
        });
    }

    /**
     * 切换音频播放
     */
    toggleAudioPlayback(recordId, btn) {
        // 检查点击的是否是当前正在播放的
        const isCurrentPlaying = this.currentAudio && btn.querySelector('.play-icon').textContent === '⏸';

        // 如果正在播放同一个，则停止
        if (isCurrentPlaying) {
            this.currentAudio.pause();
            this.currentAudio = null;
            btn.querySelector('.play-icon').textContent = '▶';
            return;
        }

        // 停止当前播放的其他音频
        if (this.currentAudio) {
            this.currentAudio.pause();
            this.currentAudio = null;
            document.querySelectorAll('.audio-play-btn .play-icon').forEach(icon => {
                icon.textContent = '▶';
            });
        }

        // 开始新的播放
        const audio = new Audio(`/api/records/${recordId}/audio`);
        this.currentAudio = audio;

        btn.querySelector('.play-icon').textContent = '⏸';

        audio.onended = () => {
            btn.querySelector('.play-icon').textContent = '▶';
            this.currentAudio = null;
        };

        audio.onerror = () => {
            btn.querySelector('.play-icon').textContent = '▶';
            this.showToast('音频加载失败');
            this.currentAudio = null;
        };

        audio.play();
    }

    /**
     * 显示设置弹窗
     */
    showSettingsPopup(btn) {
        const popup = this.elements.settingsPopup;
        const rect = btn.getBoundingClientRect();

        // 定位弹窗
        popup.style.top = `${rect.bottom + 4}px`;
        popup.style.left = `${rect.right - 140}px`;
        popup.style.display = 'block';
    }

    /**
     * 隐藏设置弹窗
     */
    hideSettingsPopup() {
        this.elements.settingsPopup.style.display = 'none';
    }

    /**
     * 删除单条记录
     */
    async deleteRecord(recordId) {
        try {
            const response = await fetch(`/api/records/${recordId}`, {
                method: 'DELETE'
            });

            if (!response.ok) {
                throw new Error('Delete failed');
            }

            this.showToast('已删除');
            await this.fetchRecords();
        } catch (error) {
            this.handleError(error);
        }
    }

    /**
     * 进入选择模式
     */
    enterSelectMode() {
        this.hideSettingsPopup();
        this.selectMode = true;
        this.selectedRecords.clear();
        this.elements.selectToolbar.style.display = 'flex';
        this.elements.selectAllCheckbox.checked = false;
        this.renderRecords();
    }

    /**
     * 退出选择模式
     */
    exitSelectMode() {
        this.selectMode = false;
        this.selectedRecords.clear();
        this.elements.selectToolbar.style.display = 'none';
        this.renderRecords();
    }

    /**
     * 切换记录选中状态
     */
    toggleRecordSelection(recordId) {
        if (this.selectedRecords.has(recordId)) {
            this.selectedRecords.delete(recordId);
        } else {
            this.selectedRecords.add(recordId);
        }

        // 更新复选框状态
        const checkbox = document.querySelector(`.record-checkbox[data-id="${recordId}"]`);
        if (checkbox) {
            checkbox.checked = this.selectedRecords.has(recordId);
        }

        // 更新全选复选框状态
        this.elements.selectAllCheckbox.checked = this.selectedRecords.size === this.records.length;
    }

    /**
     * 全选/取消全选
     */
    toggleSelectAll(checked) {
        if (checked) {
            this.records.forEach(r => this.selectedRecords.add(r.id));
        } else {
            this.selectedRecords.clear();
        }

        // 更新所有复选框
        document.querySelectorAll('.record-checkbox').forEach(checkbox => {
            checkbox.checked = checked;
        });
    }

    /**
     * 删除选中的记录
     */
    async deleteSelectedRecords() {
        if (this.selectedRecords.size === 0) {
            this.showToast('请选择要删除的记录');
            return;
        }

        if (!confirm(`确定删除选中的 ${this.selectedRecords.size} 条记录？`)) return;

        try {
            // 并行删除所有选中记录
            const deletePromises = Array.from(this.selectedRecords).map(id =>
                fetch(`/api/records/${id}`, { method: 'DELETE' })
            );

            await Promise.all(deletePromises);

            this.showToast(`已删除 ${this.selectedRecords.size} 条记录`);
            this.exitSelectMode();
            await this.fetchRecords();
        } catch (error) {
            this.handleError(error);
        }
    }

    /**
     * 复制记录文本
     */
    async copyRecordText(recordId) {
        const record = this.records.find(r => r.id === recordId);
        if (!record) return;

        const text = record.optimized_text || record.raw_text;
        if (!text) {
            this.showToast('暂无文本');
            return;
        }

        await navigator.clipboard.writeText(text);
        this.showToast('已复制到剪贴板');
    }

    /**
     * 复制当前文本
     */
    async copyText() {
        const text = this.elements.textDisplay.value;
        if (!text) {
            this.showToast('暂无文本');
            return;
        }

        await navigator.clipboard.writeText(text);
        this.showToast('已复制到剪贴板');
    }

    /**
     * 保存文本编辑
     */
    async saveText() {
        if (!this.currentRecordId) return;

        const text = this.elements.textDisplay.value;
        if (!text) return;

        try {
            await fetch(`/api/records/${this.currentRecordId}`, {
                method: 'PUT',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({ raw_text: text })
            });
        } catch (error) {
            console.error('Failed to save text:', error);
        }
    }

    /**
     * 优化当前记录
     */
    async optimizeCurrentRecord() {
        if (!this.currentRecordId) {
            this.showToast('请先录音');
            return;
        }

        this.elements.optimizeBtn.disabled = true;
        this.elements.optimizeBtn.textContent = '优化中...';

        try {
            const response = await fetch(`/api/records/${this.currentRecordId}/optimize?style=default`, {
                method: 'POST'
            });

            if (!response.ok) {
                throw new Error('Optimization failed');
            }

            const result = await response.json();
            this.elements.textDisplay.value = result.optimized_text;
            this.showToast('优化完成');

            // 刷新记录列表
            await this.fetchRecords();

        } catch (error) {
            this.handleError(error);
        } finally {
            this.elements.optimizeBtn.textContent = 'AI优化';
            this.updateButtonStates();
        }
    }

    /**
     * 优化指定记录（从历史记录中调用）
     */
    async optimizeRecord(recordId) {
        const record = this.records.find(r => r.id === recordId);
        if (!record || !record.raw_text) {
            this.showToast('暂无文本可优化');
            return;
        }

        // 找到对应的按钮并显示加载状态
        const btn = document.querySelector(`.btn-optimize[data-id="${recordId}"]`);
        if (btn) {
            btn.disabled = true;
            btn.innerHTML = '<span class="spinner-small"></span>优化中';
        }

        // 更新当前记录 ID
        this.currentRecordId = recordId;
        this.elements.textDisplay.value = record.raw_text;

        try {
            const response = await fetch(`/api/records/${recordId}/optimize?style=default`, {
                method: 'POST'
            });

            if (!response.ok) {
                throw new Error('Optimization failed');
            }

            const result = await response.json();
            this.elements.textDisplay.value = result.optimized_text;
            this.showToast('优化完成');

            // 刷新记录列表
            await this.fetchRecords();

        } catch (error) {
            this.handleError(error);
        }
    }

    /**
     * 更新按钮状态
     */
    updateButtonStates() {
        const hasText = this.elements.textDisplay.value.length > 0;
        const hasRecord = !!this.currentRecordId;

        this.elements.optimizeBtn.disabled = !hasRecord || this.isRecording;
        this.elements.copyBtn.disabled = !hasText;
    }

    /**
     * 显示 Toast 提示
     */
    showToast(message) {
        const toast = this.elements.toast;
        toast.textContent = message;
        toast.style.display = 'block';

        setTimeout(() => {
            toast.style.display = 'none';
        }, 2000);
    }

    /**
     * 错误处理
     */
    handleError(error) {
        console.error('Error:', error);
        this.showToast('发生错误: ' + error.message);
    }

    /**
     * 开始轮询转录状态
     */
    startPolling(recordId) {
        // 清除可能存在的旧轮询
        this.stopPolling(recordId);

        console.log('Start polling for record:', recordId);

        // 设置转写中状态
        this.isTranscribing = true;
        this.broadcastStatus({ transcribing: true, record_id: recordId });

        const pollInterval = setInterval(async () => {
            try {
                const response = await fetch(`/api/records/${recordId}/status`);
                if (!response.ok) {
                    console.error('Polling failed:', response.status);
                    return;
                }

                const data = await response.json();
                console.log('Poll result:', data.status);

                if (data.status === 'transcribed') {
                    // 转录完成
                    this.stopPolling(recordId);
                    this.isTranscribing = false;

                    // 计算并存储耗时
                    if (this.recordingStartTime) {
                        const elapsed = (Date.now() - this.recordingStartTime) / 1000;
                        this.processingTimes.set(recordId, elapsed);
                    }

                    // 如果是当前记录，更新文本显示
                    if (this.currentRecordId === recordId) {
                        this.elements.textDisplay.value = data.raw_text || '';

                        // 显示耗时
                        const elapsed = this.processingTimes.get(recordId);
                        if (elapsed) {
                            this.elements.transcriptionTime.textContent = `耗时: ${elapsed.toFixed(1)}s`;
                        }

                        this.updateButtonStates();
                    }

                    // 刷新记录列表
                    await this.fetchRecords();
                    this.showToast('转写完成');

                    // 广播状态
                    this.broadcastStatus({ transcribing: false });

                } else if (data.status === 'failed') {
                    // 转录失败
                    this.stopPolling(recordId);
                    this.isTranscribing = false;
                    this.showToast('转写失败');
                    await this.fetchRecords();

                    // 广播状态
                    this.broadcastStatus({ transcribing: false });
                }

            } catch (error) {
                console.error('Polling error:', error);
            }
        }, 2000);  // 每 2 秒轮询一次

        this.pollingIntervals.set(recordId, pollInterval);
    }

    /**
     * 停止轮询
     */
    stopPolling(recordId) {
        if (this.pollingIntervals.has(recordId)) {
            clearInterval(this.pollingIntervals.get(recordId));
            this.pollingIntervals.delete(recordId);
        }
    }

    /**
     * 停止所有轮询
     */
    stopAllPolling() {
        for (const interval of this.pollingIntervals.values()) {
            clearInterval(interval);
        }
        this.pollingIntervals.clear();
    }

    /**
     * 格式化日期时间
     */
    formatDateTime(isoString) {
        if (!isoString) return '';
        const date = new Date(isoString);
        return date.toLocaleString('zh-CN', {
            month: '2-digit',
            day: '2-digit',
            hour: '2-digit',
            minute: '2-digit'
        });
    }

    /**
     * 格式化时长
     */
    formatDuration(seconds) {
        if (!seconds) return '0:00';
        const mins = Math.floor(seconds / 60);
        const secs = Math.floor(seconds % 60).toString().padStart(2, '0');
        return `${mins}:${secs}`;
    }

    /**
     * HTML 转义
     */
    escapeHtml(text) {
        if (!text) return '';
        const div = document.createElement('div');
        div.textContent = text;
        return div.innerHTML;
    }
}

// 初始化应用
document.addEventListener('DOMContentLoaded', () => {
    window.app = new App();
});
