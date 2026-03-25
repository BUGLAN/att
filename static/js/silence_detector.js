/**
 * Silence Detector
 * Detects silence in audio stream for segment splitting
 */
class SilenceDetector {
    /**
     * Create a silence detector
     * @param {Object} options - Configuration options
     * @param {number} options.threshold - Silence duration threshold in seconds (default: 1.5)
     * @param {number} options.silenceLevel - Silence volume threshold in dB (default: -40)
     * @param {Function} options.onSegmentEnd - Callback when silence threshold is reached
     */
    constructor(options = {}) {
        this.threshold = options.threshold || 1.5;
        this.silenceLevel = options.silenceLevel || -40;
        this.onSegmentEnd = options.onSegmentEnd;

        this.audioContext = null;
        this.analyser = null;
        this.isMonitoring = false;
        this.silenceStartTime = null;
        this.animationFrameId = null;
    }

    /**
     * Initialize the detector with an audio stream
     * @param {MediaStream} stream - Audio stream from getUserMedia
     */
    async init(stream) {
        this.audioContext = new (window.AudioContext || window.webkitAudioContext)();
        const source = this.audioContext.createMediaStreamSource(stream);
        this.analyser = this.audioContext.createAnalyser();
        this.analyser.fftSize = 2048;
        this.analyser.smoothingTimeConstant = 0.8;
        source.connect(this.analyser);
    }

    /**
     * Start monitoring for silence
     */
    startMonitoring() {
        this.isMonitoring = true;
        this.silenceStartTime = null;
        this._checkAudio();
    }

    /**
     * Stop monitoring
     */
    stopMonitoring() {
        this.isMonitoring = false;
        if (this.animationFrameId) {
            cancelAnimationFrame(this.animationFrameId);
            this.animationFrameId = null;
        }
        this.silenceStartTime = null;
    }

    /**
     * Update configuration
     * @param {Object} options - New configuration
     */
    updateConfig(options) {
        if (options.threshold !== undefined) {
            this.threshold = options.threshold;
        }
        if (options.silenceLevel !== undefined) {
            this.silenceLevel = options.silenceLevel;
        }
    }

    /**
     * Get current audio level in dB
     * @returns {number} Current audio level
     */
    getCurrentLevel() {
        if (!this.analyser) return -100;

        const dataArray = new Uint8Array(this.analyser.frequencyBinCount);
        this.analyser.getByteFrequencyData(dataArray);

        const average = dataArray.reduce((a, b) => a + b) / dataArray.length;
        return average > 0 ? 20 * Math.log10(average / 255) : -100;
    }

    /**
     * Internal method to check audio levels
     * @private
     */
    _checkAudio() {
        if (!this.isMonitoring) return;

        const dB = this.getCurrentLevel();
        const now = Date.now();

        if (dB < this.silenceLevel) {
            // Silence detected
            if (!this.silenceStartTime) {
                this.silenceStartTime = now;
            } else if ((now - this.silenceStartTime) / 1000 >= this.threshold) {
                // Silence exceeded threshold
                if (this.onSegmentEnd) {
                    this.onSegmentEnd();
                }
                this.silenceStartTime = null;
            }
        } else {
            // Sound detected, reset silence timer
            this.silenceStartTime = null;
        }

        this.animationFrameId = requestAnimationFrame(() => this._checkAudio());
    }

    /**
     * Close and cleanup
     */
    close() {
        this.stopMonitoring();
        if (this.audioContext) {
            this.audioContext.close();
            this.audioContext = null;
        }
        this.analyser = null;
    }
}

// Export for module usage
if (typeof module !== 'undefined' && module.exports) {
    module.exports = SilenceDetector;
}
