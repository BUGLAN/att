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
            hotkeyInput: document.getElementById('settingHotkey'),
            hotkeyResetBtn: document.getElementById('resetHotkeyBtn'),
            saveBtn: document.getElementById('saveSettingsBtn')
        };

        // Initialize hotkey input handler
        this.hotkeyInput = null;

        this.bindEvents();
    }

    /**
     * Bind events
     */
    bindEvents() {
        if (this.elements.saveBtn) {
            this.elements.saveBtn.addEventListener('click', () => this.save());
        }

        // Initialize hotkey input
        if (this.elements.hotkeyInput) {
            this.hotkeyInput = new HotkeyInput(
                this.elements.hotkeyInput,
                this.elements.hotkeyResetBtn,
                (hotkeyData) => this.saveHotkey(hotkeyData)
            );
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

        // Update hotkey display
        if (this.hotkeyInput && this.settings.hotkey) {
            this.hotkeyInput.setValue(this.settings.hotkey);
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

            // Notify tray tool for settings that need immediate effect
            const wsClient = window.app?.wsClient;
            if (wsClient && wsClient.isConnected) {
                wsClient.send({
                    type: 'settings_update',
                    pause_audio_on_recording: updates.pause_audio_on_recording
                });
            }

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
     * Save hotkey settings
     */
    async saveHotkey(hotkeyData) {
        try {
            // Save via REST API
            const response = await fetch('/api/settings', {
                method: 'PUT',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({ hotkey: hotkeyData })
            });

            if (!response.ok) {
                throw new Error('Failed to save hotkey');
            }

            this.settings.hotkey = hotkeyData;

            // Also notify via WebSocket for immediate effect
            const wsClient = window.app?.wsClient;
            console.log('Saving hotkey:', hotkeyData);
            console.log('WebSocket client:', wsClient);
            console.log('WebSocket connected:', wsClient?.isConnected);

            if (wsClient && wsClient.isConnected) {
                const sent = wsClient.send({
                    type: 'hotkey_update',
                    hotkey: hotkeyData
                });
                console.log('WebSocket send result:', sent);
            }

            if (window.app) {
                window.app.showToast('快捷键已更新');
            }

        } catch (error) {
            console.error('Failed to save hotkey:', error);
            if (window.app) {
                window.app.showToast('保存快捷键失败');
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


/**
 * Hotkey Input Handler
 * Captures keyboard combinations and displays them
 */
class HotkeyInput {
    constructor(inputElement, resetBtn, onSave) {
        this.input = inputElement;
        this.resetBtn = resetBtn;
        this.onSave = onSave;
        this.isListening = false;
        this.currentKeys = { modifiers: [], key: null };
        this.isMac = navigator.platform.toUpperCase().indexOf('MAC') >= 0;

        this.bindEvents();
    }

    bindEvents() {
        // Click to start listening
        this.input.addEventListener('click', () => this.startListening());
        this.input.addEventListener('focus', () => this.startListening());

        // Keyboard capture (use capture phase to intercept before other handlers)
        document.addEventListener('keydown', (e) => this.handleKeyDown(e), true);
        document.addEventListener('keyup', (e) => this.handleKeyUp(e), true);

        // Reset button
        if (this.resetBtn) {
            this.resetBtn.addEventListener('click', (e) => {
                e.preventDefault();
                this.reset();
            });
        }
    }

    startListening() {
        if (this.isListening) return;

        this.isListening = true;
        this.input.classList.add('listening');
        this.input.value = '';
        this.input.placeholder = '按下组合键...';
        this.currentKeys = { modifiers: [], key: null };
    }

    stopListening() {
        this.isListening = false;
        this.input.classList.remove('listening');
        this.input.placeholder = '点击输入快捷键...';
    }

    handleKeyDown(e) {
        if (!this.isListening) return;

        // Prevent default behavior
        e.preventDefault();
        e.stopPropagation();

        // Collect modifiers
        const modifiers = [];
        if (e.metaKey) modifiers.push('cmd');
        if (e.ctrlKey) modifiers.push('ctrl');
        if (e.altKey) modifiers.push('alt');
        if (e.shiftKey) modifiers.push('shift');

        // Get the actual key (non-modifier)
        let key = null;
        const keyName = e.key;
        if (keyName && keyName.length === 1 && /[a-zA-Z0-9]/.test(keyName)) {
            key = keyName.toLowerCase();
        }

        this.currentKeys = {
            modifiers: [...new Set(modifiers)],  // Remove duplicates
            key: key
        };

        // Update display
        this.updateDisplay();
    }

    handleKeyUp(e) {
        if (!this.isListening) return;

        // Check if we have a valid combination
        const { modifiers, key } = this.currentKeys;

        // Validate: need at least 1 modifier (or 2 if no key)
        const isValid = modifiers.length >= 1 && (key || modifiers.length >= 2);

        if (isValid) {
            this.stopListening();
            this.saveHotkey();
        }
    }

    updateDisplay() {
        const { modifiers, key } = this.currentKeys;
        const display = this.formatDisplay(modifiers, key);
        this.input.value = display;
    }

    formatDisplay(modifiers, key) {
        const symbolMap = this.isMac ? {
            'cmd': '⌘',
            'alt': '⌥',
            'ctrl': '⌃',
            'shift': '⇧'
        } : {
            'cmd': 'Ctrl',
            'alt': 'Alt',
            'ctrl': 'Ctrl',
            'shift': 'Shift'
        };

        let display = modifiers.map(m => symbolMap[m] || m).join('');
        if (key) {
            display += key.toUpperCase();
        }

        return display;
    }

    async saveHotkey() {
        const hotkeyData = {
            ...this.currentKeys,
            display: this.input.value
        };

        if (this.onSave) {
            await this.onSave(hotkeyData);
        }
    }

    setValue(hotkeyConfig) {
        if (!hotkeyConfig) return;

        this.currentKeys = {
            modifiers: hotkeyConfig.modifiers || [],
            key: hotkeyConfig.key
        };
        this.input.value = hotkeyConfig.display || this.formatDisplay(
            hotkeyConfig.modifiers,
            hotkeyConfig.key
        );
    }

    reset() {
        // Reset to platform default
        const defaultHotkey = this.isMac ? {
            modifiers: ['alt', 'cmd'],
            key: null,
            display: '⌥⌘'
        } : {
            modifiers: ['alt', 'ctrl'],
            key: null,
            display: 'Alt+Ctrl'
        };

        this.setValue(defaultHotkey);
        this.saveHotkey();
    }
}

// Global settings manager
window.settingsManager = new SettingsManager();
