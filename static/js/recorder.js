/**
 * 录音模块 - 使用 MediaRecorder API
 * 策略：录音时收集 chunks，停止时合并上传完整音频
 */
class AudioRecorder {
    constructor(options = {}) {
        this.chunkInterval = options.chunkInterval || 5000;
        this.mimeType = 'audio/webm;codecs=opus';
        this.mediaRecorder = null;
        this.mediaStream = null;
        this.isRecording = false;
        this.startTime = null;
        this.durationTimer = null;
        this.audioChunks = []; // 收集音频片段

        // 回调函数
        this.onUploadComplete = options.onUploadComplete || (() => {});
        this.onRecordingStart = options.onRecordingStart || (() => {});
        this.onRecordingStop = options.onRecordingStop || (() => {});
        this.onError = options.onError || (() => {});

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

    async start() {
        if (this.isRecording) {
            console.warn('Already recording');
            return;
        }

        try {
            this.mediaStream = await navigator.mediaDevices.getUserMedia({
                audio: {
                    echoCancellation: true,
                    noiseSuppression: true,
                    sampleRate: 16000
                }
            });

            this.mediaRecorder = new MediaRecorder(this.mediaStream, {
                mimeType: this.mimeType
            });

            this.isRecording = true;
            this.startTime = Date.now();
            this.audioChunks = [];

            // 收集数据 - 不立即上传
            this.mediaRecorder.ondataavailable = (event) => {
                if (event.data.size > 0) {
                    this.audioChunks.push(event.data);
                    console.log('Collected chunk:', event.data.size, 'bytes, total:', this.audioChunks.length);
                }
            };

            this.mediaRecorder.onerror = (event) => {
                console.error('MediaRecorder error:', event);
                this.onError(event.error);
            };

            this.mediaRecorder.start(this.chunkInterval);
            this._startDurationTimer();

            this.onRecordingStart();
            console.log('Recording started');

        } catch (error) {
            console.error('Failed to start recording:', error);
            this.onError(error);
        }
    }

    async stop() {
        if (!this.isRecording) {
            return;
        }

        return new Promise((resolve) => {
            this.mediaRecorder.onstop = async () => {
                this.isRecording = false;
                this._stopDurationTimer();

                if (this.mediaStream) {
                    this.mediaStream.getTracks().forEach(track => track.stop());
                    this.mediaStream = null;
                }

                // 合并并上传所有音频片段
                if (this.audioChunks.length > 0) {
                    console.log('Uploading', this.audioChunks.length, 'chunks...');
                    await this._uploadMergedAudio();
                }

                this.onRecordingStop();
                console.log('Recording stopped');
                resolve();
            };

            this.mediaRecorder.stop();
        });
    }

    async _uploadMergedAudio() {
        // 合并所有 chunk 为一个完整的 webm 文件
        const blob = new Blob(this.audioChunks, { type: this.mimeType });
        console.log('Merged audio size:', blob.size, 'bytes');

        const formData = new FormData();
        const extension = this.mimeType.includes('webm') ? 'webm' :
                         this.mimeType.includes('mp4') ? 'm4a' : 'ogg';

        formData.append('file', blob, `recording_${Date.now()}.${extension}`);
        formData.append('auto_transcribe', 'true');

        try {
            const response = await fetch('/api/audio/upload', {
                method: 'POST',
                body: formData
            });

            if (!response.ok) {
                const errorText = await response.text();
                console.error('Upload failed:', response.status, errorText);
                throw new Error(`Upload failed: ${response.status}`);
            }

            const result = await response.json();
            console.log('Upload result:', result);

            this.onUploadComplete(result);

        } catch (error) {
            console.error('Failed to upload audio:', error);
            this.onError(error);
        }
    }

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

    _stopDurationTimer() {
        if (this.durationTimer) {
            clearInterval(this.durationTimer);
            this.durationTimer = null;
        }
    }

    getDuration() {
        if (!this.startTime) return 0;
        return Math.floor((Date.now() - this.startTime) / 1000);
    }
}

window.AudioRecorder = AudioRecorder;
