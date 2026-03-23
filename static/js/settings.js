/**
 * Settings Management
 */
class SettingsManager {
    constructor() {
        this.settings = {};
        this.elements = {
            whisperModel: document.getElementById('settingWhisperModel'),
            whisperDevice: document.getElementById('settingWhisperDevice'),
            pauseAudio: document.getElementById('settingPauseAudio'),
            autoOptimize: document.getElementById('settingAutoOptimize'),
            saveBtn: document.getElementById('saveSettingsBtn')
        };

        this.bindEvents();
    }

    /**
     * Bind events
     */
    bindEvents() {
        if (this.elements.saveBtn) {
            this.elements.saveBtn.addEventListener('click', () => this.save());
        }
    }

    /**
     * Load settings from server
     */
    async load() {
        try {
            const response = await fetch('/api/settings');
            this.settings = await response.json();
            this.updateUI();
        } catch (error) {
            console.error('Failed to load settings:', error);
        }
    }

    /**
     * Update UI with current settings
     */
    updateUI() {
        if (this.elements.whisperModel) {
            this.elements.whisperModel.value = this.settings.whisper_model || 'large-v3';
        }
        if (this.elements.whisperDevice) {
            this.elements.whisperDevice.value = this.settings.whisper_device || 'auto';
        }
        if (this.elements.pauseAudio) {
            this.elements.pauseAudio.checked = this.settings.pause_audio_on_recording !== false;
        }
        if (this.elements.autoOptimize) {
            this.elements.autoOptimize.checked = this.settings.auto_optimize === true;
        }
    }

    /**
     * Save settings to server
     */
    async save() {
        const updates = {
            whisper_model: this.elements.whisperModel?.value,
            whisper_device: this.elements.whisperDevice?.value,
            pause_audio_on_recording: this.elements.pauseAudio?.checked,
            auto_optimize: this.elements.autoOptimize?.checked
        };

        try {
            const response = await fetch('/api/settings', {
                method: 'PUT',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify(updates)
            });

            if (!response.ok) {
                throw new Error('Failed to save settings');
            }

            this.settings = await response.json();

            // Show success message
            if (window.app) {
                window.app.showToast('设置已保存');
            }

            // Some settings require server restart to take effect
            if (updates.whisper_model !== this.settings.whisper_model ||
                updates.whisper_device !== this.settings.whisper_device) {
                if (window.app) {
                    window.app.showToast('模型设置需重启服务器生效');
                }
            }

        } catch (error) {
            console.error('Failed to save settings:', error);
            if (window.app) {
                window.app.showToast('保存设置失败');
            }
        }
    }

    /**
     * Get a setting value
     */
    get(key) {
        return this.settings[key];
    }
}

// Global settings manager
window.settingsManager = new SettingsManager();
