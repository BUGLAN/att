/**
 * 主应用逻辑
 */
class App {
    constructor() {
        // 状态
        this.sessionId = null;
        this.fragments = [];
        this.isRecording = false;
        this.pollInterval = null;

        // 录音器
        this.recorder = new AudioRecorder({
            chunkInterval: 5000, // 5秒一片段
            onRecordingStart: () => this.handleRecordingStart(),
            onRecordingStop: () => this.handleRecordingStop(),
            onChunkUploaded: (result) => this.handleChunkUploaded(result),
            onError: (error) => this.handleError(error)
        });

        // DOM 元素
        this.elements = {
            recordBtn: document.getElementById('recordBtn'),
            recordingStatus: document.getElementById('recordingStatus'),
            textDisplay: document.getElementById('textDisplay'),
            fragmentsList: document.getElementById('fragmentsList'),
            fragmentCount: document.getElementById('fragmentCount'),
            optimizeBtn: document.getElementById('optimizeBtn'),
            mergeBtn: document.getElementById('mergeBtn'),
            exportBtn: document.getElementById('exportBtn'),
            exportModal: document.getElementById('exportModal'),
            closeModal: document.getElementById('closeModal'),
            duration: document.getElementById('duration')
        };

        // 绑定事件
        this.bindEvents();
    }

    /**
     * 绑定事件监听
     */
    bindEvents() {
        // 录音按钮
        this.elements.recordBtn.addEventListener('click', () => this.toggleRecording());

        // 优化按钮
        this.elements.optimizeBtn.addEventListener('click', () => this.optimizeCurrentSession());

        // 合并按钮
        this.elements.mergeBtn.addEventListener('click', () => this.mergeFragments());

        // 导出按钮
        this.elements.exportBtn.addEventListener('click', () => this.showExportModal());

        // 关闭弹窗
        this.elements.closeModal.addEventListener('click', () => this.hideExportModal());

        // 导出选项
        document.querySelectorAll('.export-option').forEach(btn => {
            btn.addEventListener('click', (e) => {
                const format = e.currentTarget.dataset.format;
                this.exportSession(format);
            });
        });

        // 点击弹窗外部关闭
        this.elements.exportModal.addEventListener('click', (e) => {
            if (e.target === this.elements.exportModal) {
                this.hideExportModal();
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
        // 每次录音都创建新 session
        try {
            const response = await fetch('/api/sessions?name=' + encodeURIComponent('录音会话 ' + new Date().toLocaleString()), {
                method: 'POST'
            });
            const session = await response.json();
            this.sessionId = session.id;
            console.log('Session created:', this.sessionId);
        } catch (error) {
            this.handleError(error);
            return;
        }

        // 开始录音
        await this.recorder.start(this.sessionId);
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

        // 停止轮询
        this.stopPolling();

        await this.recorder.stop();

        // 最后一次获取片段
        await this.fetchFragments();

        // 统一设置最终状态
        this.elements.recordBtn.disabled = false;
        this.elements.recordBtn.querySelector('.btn-text').textContent = '开始录音';
        this.elements.recordingStatus.querySelector('.status-text').textContent = '录音结束';
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

        // 清空显示
        this.elements.textDisplay.innerHTML = '<p class="placeholder">录音中，转写结果将实时显示...</p>';
        this.fragments = [];

        // 开始轮询获取片段
        this.startPolling();
    }

    /**
     * 录音停止回调
     */
    handleRecordingStop() {
        this.isRecording = false;
        // 文本状态由 stopRecording() 统一管理，避免 disabled 期间文本不一致
    }

    /**
     * 片段上传完成回调
     */
    handleChunkUploaded(result) {
        console.log('Chunk uploaded:', result);
        // 同步 session_id（后端可能创建了新 session）
        if (result.session_id && result.session_id !== this.sessionId) {
            this.sessionId = result.session_id;
            console.log('Session synced:', this.sessionId);
        }
        // 立即获取最新片段
        this.fetchFragments();
    }

    /**
     * 错误处理
     */
    handleError(error) {
        console.error('Error:', error);
        alert('发生错误: ' + error.message);
    }

    /**
     * 开始轮询获取片段
     */
    startPolling() {
        this.pollInterval = setInterval(() => {
            this.fetchFragments();
        }, 2000); // 每2秒轮询一次
    }

    /**
     * 停止轮询
     */
    stopPolling() {
        if (this.pollInterval) {
            clearInterval(this.pollInterval);
            this.pollInterval = null;
        }
    }

    /**
     * 获取片段列表
     */
    async fetchFragments() {
        if (!this.sessionId) return;

        try {
            const response = await fetch(`/api/fragments?session_id=${this.sessionId}&limit=100`);
            this.fragments = await response.json();
            this.renderFragments();
            this.updateButtonStates();
        } catch (error) {
            console.error('Failed to fetch fragments:', error);
        }
    }

    /**
     * 渲染片段列表
     */
    renderFragments() {
        // 更新片段数量
        this.elements.fragmentCount.textContent = `${this.fragments.length} 个片段`;

        // 更新文本显示
        if (this.fragments.length === 0) {
            this.elements.textDisplay.innerHTML = '<p class="placeholder">暂无转写内容</p>';
            this.elements.fragmentsList.innerHTML = '<p class="placeholder">暂无片段</p>';
            return;
        }

        // 显示合并后的文本
        const allText = this.fragments
            .map(f => f.optimized_text || f.raw_text || '')
            .filter(t => t)
            .join('\n\n');

        this.elements.textDisplay.innerHTML = allText || '<p class="placeholder">正在转写中...</p>';

        // 渲染片段列表
        this.elements.fragmentsList.innerHTML = this.fragments.map(f => `
            <div class="fragment-item" data-id="${f.id}">
                <div class="fragment-info">
                    <div class="fragment-time">
                        ${this.formatTime(f.start_time)} - ${this.formatTime(f.end_time)}
                    </div>
                    <div class="fragment-preview">
                        ${this.escapeHtml(f.optimized_text || f.raw_text || '处理中...')}
                    </div>
                </div>
                <span class="fragment-status ${f.status}">
                    ${this.getStatusText(f.status)}
                </span>
            </div>
        `).join('');
    }

    /**
     * 更新按钮状态
     */
    updateButtonStates() {
        const hasFragments = this.fragments.length > 0;
        const hasTranscribed = this.fragments.some(f => f.status === 'transcribed');
        const hasOptimized = this.fragments.some(f => f.status === 'optimized');

        this.elements.optimizeBtn.disabled = !hasTranscribed || this.isRecording;
        this.elements.mergeBtn.disabled = !hasFragments || this.isRecording;
        this.elements.exportBtn.disabled = !hasFragments || this.isRecording;
    }

    /**
     * 优化当前会话的所有片段
     */
    async optimizeCurrentSession() {
        const transcribedFragments = this.fragments.filter(f =>
            f.status === 'transcribed' && f.raw_text
        );

        if (transcribedFragments.length === 0) {
            alert('没有可优化的片段');
            return;
        }

        this.elements.optimizeBtn.disabled = true;
        this.elements.optimizeBtn.textContent = '⏳ 优化中...';

        try {
            // 逐个优化
            for (const fragment of transcribedFragments) {
                const response = await fetch(`/api/fragments/${fragment.id}/optimize?style=formal`, {
                    method: 'POST'
                });
                if (response.ok) {
                    console.log('Optimized:', fragment.id);
                }
            }

            // 刷新片段列表
            await this.fetchFragments();

        } catch (error) {
            this.handleError(error);
        } finally {
            this.elements.optimizeBtn.textContent = '✨ AI优化';
            this.updateButtonStates();
        }
    }

    /**
     * 合并片段
     */
    async mergeFragments() {
        if (this.fragments.length === 0) {
            alert('没有可合并的片段');
            return;
        }

        try {
            const fragmentIds = this.fragments.map(f => f.id);
            const response = await fetch('/api/fragments/merge', {
                method: 'POST',
                headers: {
                    'Content-Type': 'application/json'
                },
                body: JSON.stringify({
                    fragment_ids: fragmentIds,
                    use_optimized: true
                })
            });

            const result = await response.json();

            // 显示合并结果
            this.elements.textDisplay.innerHTML = `
                <div class="merged-text">${this.escapeHtml(result.merged_text)}</div>
            `;

        } catch (error) {
            this.handleError(error);
        }
    }

    /**
     * 显示导出弹窗
     */
    showExportModal() {
        this.elements.exportModal.classList.add('active');
    }

    /**
     * 隐藏导出弹窗
     */
    hideExportModal() {
        this.elements.exportModal.classList.remove('active');
    }

    /**
     * 导出会话
     */
    exportSession(format) {
        if (!this.sessionId) {
            alert('没有可导出的内容');
            return;
        }

        // 打开下载链接
        const url = `/api/export/${this.sessionId}?format=${format}&use_optimized=true`;
        window.open(url, '_blank');
        this.hideExportModal();
    }

    /**
     * 格式化时间
     */
    formatTime(seconds) {
        if (!seconds) return '00:00';
        const mins = Math.floor(seconds / 60).toString().padStart(2, '0');
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
