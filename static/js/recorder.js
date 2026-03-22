/**
 * 录音模块 - 使用 MediaRecorder API
 */
class AudioRecorder {
    constructor(options = {}) {
        this.chunkInterval = options.chunkInterval || 5000; // 默认5秒一片段
        this.mimeType = 'audio/webm;codecs=opus';
        this.mediaRecorder = null;
        this.mediaStream = null;
        this.isRecording = false;
        this.sessionId = null;
        this.startTime = null;
        this.durationTimer = null;

        // 回调函数
        this.onChunkUploaded = options.onChunkUploaded || (() => {});
        this.onRecordingStart = options.onRecordingStart || (() => {});
        this.onRecordingStop = options.onRecordingStop || (() => {});
        this.onError = options.onError || (() => {});

        // 检查支持的 MIME 类型
        this._checkMimeType();
    }

    _checkMimeType() {
        const types = [
            'audio/webm;codecs=opus',
            'audio/webm',
            'audio/mp4',
            'audio/ogg;codecs=opus'
        ];

        for (const type of types) {
            if (MediaRecorder.isTypeSupported(type)) {
                this.mimeType = type;
                console.log('Using MIME type:', type);
                break;
            }
        }
    }

    /**
     * 开始录音
     */
    async start(sessionId) {
        if (this.isRecording) {
            console.warn('Already recording');
            return;
        }

        try {
            // 获取麦克风权限
            this.mediaStream = await navigator.mediaDevices.getUserMedia({
                audio: {
                    echoCancellation: true,
                    noiseSuppression: true,
                    sampleRate: 16000
                }
            });

            // 创建 MediaRecorder
            this.mediaRecorder = new MediaRecorder(this.mediaStream, {
                mimeType: this.mimeType
            });

            this.sessionId = sessionId;
            this.isRecording = true;
            this.startTime = Date.now();

            // 处理数据
            this.mediaRecorder.ondataavailable = async (event) => {
                if (event.data.size > 0) {
                    await this._uploadChunk(event.data);
                }
            };

            this.mediaRecorder.onerror = (event) => {
                console.error('MediaRecorder error:', event);
                this.onError(event.error);
            };

            // 开始录音，每 chunkInterval 毫秒触发一次 ondataavailable
            this.mediaRecorder.start(this.chunkInterval);

            // 开始计时
            this._startDurationTimer();

            this.onRecordingStart();
            console.log('Recording started, sessionId:', sessionId);

        } catch (error) {
            console.error('Failed to start recording:', error);
            this.onError(error);
        }
    }

    /**
     * 停止录音
     */
    async stop() {
        if (!this.isRecording) {
            return;
        }

        return new Promise((resolve) => {
            this.mediaRecorder.onstop = async () => {
                this.isRecording = false;
                this._stopDurationTimer();

                // 停止所有音轨
                if (this.mediaStream) {
                    this.mediaStream.getTracks().forEach(track => track.stop());
                    this.mediaStream = null;
                }

                this.onRecordingStop();
                console.log('Recording stopped');
                resolve();
            };

            this.mediaRecorder.stop();
        });
    }

    /**
     * 上传音频片段
     */
    async _uploadChunk(blob) {
        const formData = new FormData();

        // 根据类型确定文件扩展名
        const extension = this.mimeType.includes('webm') ? 'webm' :
                         this.mimeType.includes('mp4') ? 'm4a' : 'ogg';

        formData.append('file', blob, `chunk_${Date.now()}.${extension}`);
        formData.append('auto_transcribe', 'true');

        // 只有在 sessionId 有效时才传递
        if (this.sessionId && this.sessionId.trim() !== '') {
            formData.append('session_id', this.sessionId);
        }

        try {
            const response = await fetch('/api/audio/upload', {
                method: 'POST',
                body: formData
            });

            if (!response.ok) {
                const errorText = await response.text();
                console.error('Upload failed:', response.status, errorText);
                throw new Error(`Upload failed: ${response.status} - ${errorText}`);
            }

            const result = await response.json();
            console.log('Chunk uploaded:', result);

            // 如果返回了新的 session_id，更新本地
            if (result.session_id && !this.sessionId) {
                this.sessionId = result.session_id;
            }

            this.onChunkUploaded(result);

        } catch (error) {
            console.error('Failed to upload chunk:', error);
            this.onError(error);
        }
    }

    /**
     * 开始计时器
     */
    _startDurationTimer() {
        const durationEl = document.getElementById('duration');
        if (!durationEl) return;

        this.durationTimer = setInterval(() => {
            const elapsed = Math.floor((Date.now() - this.startTime) / 1000);
            const minutes = Math.floor(elapsed / 60).toString().padStart(2, '0');
            const seconds = (elapsed % 60).toString().padStart(2, '0');
            durationEl.textContent = `${minutes}:${seconds}`;
        }, 1000);
    }

    /**
     * 停止计时器
     */
    _stopDurationTimer() {
        if (this.durationTimer) {
            clearInterval(this.durationTimer);
            this.durationTimer = null;
        }
    }

    /**
     * 获取录音时长（秒）
     */
    getDuration() {
        if (!this.startTime) return 0;
        return Math.floor((Date.now() - this.startTime) / 1000);
    }
}

// 导出
window.AudioRecorder = AudioRecorder;
