/**
 * 主应用逻辑
 */
class App {
    constructor() {
        // 状态
        this.currentRecordId = null;  // 当前录音的 record ID
        this.records = [];             // 所有记录列表
        this.isRecording = false;

        // 录音器
        this.recorder = new AudioRecorder({
            onRecordingStart: () => this.handleRecordingStart(),
            onRecordingStop: () => this.handleRecordingStop(),
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
            toast: document.getElementById('toast')
        };

        // 当前播放的音频
        this.currentAudio = null;

        // 绑定事件
        this.bindEvents();

        // 加载历史记录
        this.fetchRecords();
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

        // 开始录音
        await this.recorder.start();
    }

    /**
     * 停止录音
     */
    async stopRecording() {
        // 显示处理中状态
        this.elements.recordBtn.classList.remove('recording');
        this.elements.recordBtn.querySelector('.btn-text').textContent = '处理中...';
        this.elements.recordingStatus.classList.remove('recording');
        this.elements.recordingStatus.querySelector('.status-text').textContent = '上传转写中...';
        this.elements.recordBtn.disabled = true;

        await this.recorder.stop();

        // 统一设置最终状态
        this.elements.recordBtn.disabled = false;
        this.elements.recordBtn.querySelector('.btn-text').textContent = '开始录音';
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
    }

    /**
     * 录音停止回调
     */
    handleRecordingStop() {
        this.isRecording = false;
        // 文本状态由 stopRecording() 统一管理
    }

    /**
     * 上传完成回调
     */
    async handleUploadComplete(result) {
        console.log('Upload complete:', result);
        this.currentRecordId = result.record_id;

        // 更新文本显示
        if (result.transcription_text) {
            this.elements.textDisplay.value = result.transcription_text;
        }

        // 刷新记录列表
        await this.fetchRecords();
        this.updateButtonStates();
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
        // 更新记录数量
        this.elements.recordCount.textContent = `${this.records.length} 条记录`;

        if (this.records.length === 0) {
            this.elements.recordsList.innerHTML = '<p class="placeholder">暂无录音记录</p>';
            return;
        }

        // 渲染记录卡片
        this.elements.recordsList.innerHTML = this.records.map(r => `
            <div class="record-item" data-id="${r.id}">
                <div class="record-audio">
                    <button class="audio-play-btn" data-id="${r.id}" title="试听">
                        <span class="play-icon">▶</span>
                    </button>
                </div>
                <div class="record-content">
                    <div class="record-time">${this.formatDateTime(r.created_at)}</div>
                    <div class="record-text">${this.escapeHtml(r.optimized_text || r.raw_text || '处理中...')}</div>
                </div>
                <div class="record-actions">
                    <span class="record-duration">${this.formatDuration(r.duration)}</span>
                    <span class="record-status ${r.status}">${this.getStatusText(r.status)}</span>
                </div>
            </div>
        `).join('');

        // 绑定事件
        this.bindRecordEvents();
    }

    /**
     * 绑定记录项事件
     */
    bindRecordEvents() {
        // 点击记录项复制文本
        this.elements.recordsList.querySelectorAll('.record-item').forEach(item => {
            item.addEventListener('click', (e) => {
                // 忽略播放按钮点击
                if (e.target.closest('.audio-play-btn')) return;

                const recordId = item.dataset.id;
                this.copyRecordText(recordId);
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
            const response = await fetch(`/api/records/${this.currentRecordId}/optimize?style=formal`, {
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
     * 获取状态文本
     */
    getStatusText(status) {
        const statusMap = {
            pending: '等待中',
            transcribing: '转写中',
            transcribed: '已转写',
            optimizing: '优化中',
            optimized: '已优化',
            failed: '失败'
        };
        return statusMap[status] || status;
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
