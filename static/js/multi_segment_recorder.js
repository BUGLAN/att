/**
 * Multi-Segment Recorder
 * Handles continuous recording with silence detection and segment splitting
 */
class MultiSegmentRecorder {
    /**
     * Create a multi-segment recorder
     * @param {Object} options - Configuration options
     * @param {Object} options.wsClient - WebSocket client for real-time updates
     * @param {number} options.silenceThreshold - Silence duration threshold (default: 1.5s)
     * @param {number} options.silenceLevel - Silence volume threshold in dB (default: -40)
     * @param {Function} options.onSegmentTranscribed - Callback when segment is transcribed
     * @param {Function} options.onStatusChange - Callback when recording status changes
     */
    constructor(options = {}) {
        this.wsClient = options.wsClient;
        this.silenceThreshold = options.silenceThreshold || 1.5;
        this.silenceLevel = options.silenceLevel || -40;
        this.onSegmentTranscribed = options.onSegmentTranscribed;
        this.onStatusChange = options.onStatusChange;

        this.sessionId = null;
        this.segments = [];
        this.currentSegmentIndex = 0;
        this.mediaStream = null;
        this.recorder = null;
        this.silenceDetector = null;
        this.isRecording = false;
        this.isPaused = false;
        this.audioChunks = [];
    }

    /**
     * Initialize and start recording
     */
    async start() {
        try {
            // Get audio stream
            this.mediaStream = await navigator.mediaDevices.getUserMedia({
                audio: {
                    echoCancellation: true,
                    noiseSuppression: true,
                    sampleRate: 16000
                }
            });

            // Initialize silence detector
            this.silenceDetector = new SilenceDetector({
                threshold: this.silenceThreshold,
                silenceLevel: this.silenceLevel,
                onSegmentEnd: () => this._splitAndUpload()
            });
            await this.silenceDetector.init(this.mediaStream);

            // Create recording session
            const response = await fetch('/api/sessions', { method: 'POST' });
            const data = await response.json();
            this.sessionId = data.session_id;

            // Setup MediaRecorder
            this.recorder = new MediaRecorder(this.mediaStream, {
                mimeType: 'audio/webm;codecs=opus'
            });

            this.recorder.ondataavailable = (event) => {
                if (event.data.size > 0) {
                    this.audioChunks.push(event.data);
                }
            };

            // Start recording
            this.recorder.start(1000); // Collect chunks every second
            this.silenceDetector.startMonitoring();
            this.isRecording = true;
            this.isPaused = false;

            // Setup WebSocket listener
            if (this.wsClient) {
                this.wsClient.on('segment_transcribed', (data) => {
                    if (data.session_id === this.sessionId) {
                        this._handleSegmentTranscribed(data);
                    }
                });
            }

            if (this.onStatusChange) {
                this.onStatusChange('recording', { sessionId: this.sessionId });
            }

            console.log('Multi-segment recording started', { sessionId: this.sessionId });
            return this.sessionId;

        } catch (error) {
            console.error('Failed to start recording', error);
            throw error;
        }
    }

    /**
     * Pause recording
     */
    pause() {
        if (this.recorder && this.isRecording && !this.isPaused) {
            this.recorder.pause();
            this.silenceDetector.stopMonitoring();
            this.isPaused = true;

            if (this.onStatusChange) {
                this.onStatusChange('paused');
            }
        }
    }

    /**
     * Resume recording
     */
    resume() {
        if (this.recorder && this.isRecording && this.isPaused) {
            this.recorder.resume();
            this.silenceDetector.startMonitoring();
            this.isPaused = false;

            if (this.onStatusChange) {
                this.onStatusChange('recording');
            }
        }
    }

    /**
     * Stop recording and merge all segments
     */
    async stop() {
        if (!this.recorder) return null;

        this.isRecording = false;
        this.silenceDetector.stopMonitoring();

        // Upload last segment if there's audio
        if (this.audioChunks.length > 0) {
            await this._uploadCurrentChunks();
        }

        // Stop recorder
        this.recorder.stop();

        // Stop media stream
        if (this.mediaStream) {
            this.mediaStream.getTracks().forEach(track => track.stop());
            this.mediaStream = null;
        }

        // Merge session
        if (this.sessionId) {
            try {
                const response = await fetch(`/api/sessions/${this.sessionId}/merge`, {
                    method: 'POST'
                });
                const data = await response.json();

                if (this.onStatusChange) {
                    this.onStatusChange('completed', { recordId: data.record_id });
                }

                console.log('Session merged', data);
                return data;

            } catch (error) {
                console.error('Failed to merge session', error);
                throw error;
            }
        }

        return null;
    }

    /**
     * Force split current segment (manual split)
     */
    async forceSplit() {
        if (this.isRecording && !this.isPaused && this.audioChunks.length > 0) {
            await this._splitAndUpload();
        }
    }

    /**
     * Update silence detection settings
     * @param {Object} options - New settings
     */
    updateSettings(options) {
        if (options.silenceThreshold !== undefined) {
            this.silenceThreshold = options.silenceThreshold;
        }
        if (options.silenceLevel !== undefined) {
            this.silenceLevel = options.silenceLevel;
        }
        if (this.silenceDetector) {
            this.silenceDetector.updateConfig({
                threshold: this.silenceThreshold,
                silenceLevel: this.silenceLevel
            });
        }
    }

    /**
     * Internal: Split and upload current audio
     * @private
     */
    async _splitAndUpload() {
        if (this.audioChunks.length === 0) return;

        const chunksToUpload = [...this.audioChunks];
        this.audioChunks = [];

        await this._uploadChunks(chunksToUpload);
    }

    /**
     * Internal: Upload current chunks
     * @private
     */
    async _uploadCurrentChunks() {
        const chunksToUpload = [...this.audioChunks];
        this.audioChunks = [];
        await this._uploadChunks(chunksToUpload);
    }

    /**
     * Internal: Upload audio chunks
     * @private
     */
    async _uploadChunks(chunks) {
        if (!this.sessionId || chunks.length === 0) return;

        try {
            const blob = new Blob(chunks, { type: 'audio/webm' });
            const formData = new FormData();
            formData.append('file', blob, `segment_${this.currentSegmentIndex}.webm`);

            const response = await fetch(`/api/sessions/${this.sessionId}/segments`, {
                method: 'POST',
                body: formData
            });

            const data = await response.json();
            console.log('Segment uploaded', data);

            this.currentSegmentIndex++;

            if (this.onStatusChange) {
                this.onStatusChange('segment_uploaded', {
                    segmentId: data.segment_id,
                    sequence: data.sequence
                });
            }

        } catch (error) {
            console.error('Failed to upload segment', error);
        }
    }

    /**
     * Internal: Handle segment transcribed event
     * @private
     */
    _handleSegmentTranscribed(data) {
        this.segments.push({
            id: data.segment_id,
            text: data.text,
            duration: data.duration
        });

        if (this.onSegmentTranscribed) {
            this.onSegmentTranscribed({
                sessionId: this.sessionId,
                segmentId: data.segment_id,
                text: data.text,
                duration: data.duration,
                totalSegments: this.segments.length
            });
        }
    }

    /**
     * Get all transcribed text
     */
    getTranscribedText() {
        return this.segments.map(s => s.text).join('\n\n');
    }

    /**
     * Get recording duration
     */
    getDuration() {
        return this.segments.reduce((sum, s) => sum + (s.duration || 0), 0);
    }

    /**
     * Cleanup resources
     */
    cleanup() {
        if (this.silenceDetector) {
            this.silenceDetector.close();
            this.silenceDetector = null;
        }
        if (this.mediaStream) {
            this.mediaStream.getTracks().forEach(track => track.stop());
            this.mediaStream = null;
        }
        this.recorder = null;
        this.audioChunks = [];
        this.segments = [];
        this.sessionId = null;
        this.isRecording = false;
        this.isPaused = false;
    }
}

// Export for module usage
if (typeof module !== 'undefined' && module.exports) {
    module.exports = MultiSegmentRecorder;
}
