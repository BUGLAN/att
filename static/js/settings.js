/**
 * Settings Management
 */
class SettingsManager {
    constructor() {
        this.settings = {};
        this.presets = [];
        this.llmConfigs = [];
        this.elements = {
            whisperModel: document.getElementById('settingWhisperModel'),
            whisperDevice: document.getElementById('settingWhisperDevice'),
            pauseAudio: document.getElementById('settingPauseAudio'),
            autoOptimize: document.getElementById('settingAutoOptimize'),
            hotkeyInput: document.getElementById('settingHotkey'),
            hotkeyResetBtn: document.getElementById('resetHotkeyBtn'),
            saveBtn: document.getElementById('saveSettingsBtn'),
            // New elements
            recordingMode: document.getElementById('settingRecordingMode'),
            silenceThreshold: document.getElementById('settingSilenceThreshold'),
            silenceThresholdSetting: document.getElementById('silenceThresholdSetting'),
            presetSelect: document.getElementById('settingPreset'),
            openPresetManagerBtn: document.getElementById('openPresetManagerBtn'),
            // LLM config elements
            llmConfigSelect: document.getElementById('settingLLMConfig'),
            openLLMManagerBtn: document.getElementById('openLLMManagerBtn')
        };

        // Initialize hotkey input handler
        this.hotkeyInput = null;

        // Initialize preset manager
        this.presetManager = null;

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

        // Recording mode change
        if (this.elements.recordingMode) {
            this.elements.recordingMode.addEventListener('change', () => {
                this.updateSilenceThresholdVisibility();
            });
        }

        // Preset manager
        if (this.elements.openPresetManagerBtn) {
            this.elements.openPresetManagerBtn.addEventListener('click', () => {
                this.openPresetManager();
            });
        }

        // LLM config manager
        if (this.elements.openLLMManagerBtn) {
            this.elements.openLLMManagerBtn.addEventListener('click', () => {
                this.openLLMManager();
            });
        }
    }

    /**
     * Update silence threshold visibility based on recording mode
     */
    updateSilenceThresholdVisibility() {
        if (this.elements.recordingMode && this.elements.silenceThresholdSetting) {
            const isMulti = this.elements.recordingMode.value === 'multi';
            this.elements.silenceThresholdSetting.style.display = isMulti ? 'flex' : 'none';
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

        // New settings
        if (this.elements.recordingMode) {
            this.elements.recordingMode.value = this.settings.recording_mode || 'single';
            this.updateSilenceThresholdVisibility();
        }
        if (this.elements.silenceThreshold) {
            this.elements.silenceThreshold.value = this.settings.silence_threshold || 1.5;
        }
        if (this.elements.presetSelect) {
            this.elements.presetSelect.value = this.settings.default_preset_id || 'builtin-general';
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
            auto_optimize: this.elements.autoOptimize?.checked,
            // New settings
            recording_mode: this.elements.recordingMode?.value,
            silence_threshold: parseFloat(this.elements.silenceThreshold?.value) || 1.5,
            default_preset_id: this.elements.presetSelect?.value
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
     * Open preset manager modal
     */
    openPresetManager() {
        if (!this.presetManager) {
            this.presetManager = new PresetManager();
        }
        this.presetManager.open();
    }

    /**
     * Open LLM config manager modal
     */
    openLLMManager() {
        if (window.llmConfigManager) {
            window.llmConfigManager.open();
        }
    }

    /**
     * Load LLM configurations
     */
    async loadLLMConfigs() {
        try {
            const response = await fetch('/api/llm/configs');
            const data = await response.json();
            this.llmConfigs = data.configs || [];
            this.updateLLMConfigUI();
        } catch (error) {
            console.error('Failed to load LLM configs:', error);
        }
    }

    /**
     * Update LLM config UI
     */
    updateLLMConfigUI() {
        if (!this.elements.llmConfigSelect) return;

        this.elements.llmConfigSelect.innerHTML = this.llmConfigs.map(config =>
            `<option value="${config.id}" ${config.is_default ? 'selected' : ''}>
                ${config.name} (${config.provider})
            </option>`
        ).join('');

        if (this.llmConfigs.length === 0) {
            this.elements.llmConfigSelect.innerHTML = '<option value="">暂无配置</option>';
        }
    }

    /**
     * Get selected LLM config ID
     */
    getSelectedLLMConfig() {
        return this.elements.llmConfigSelect?.value || null;
    }

    /**
     * Get a setting value
     */
    get(key) {
        return this.settings[key];
    }
}


/**
 * Preset Manager
 * Manages AI optimization presets
 */
class PresetManager {
    constructor() {
        this.presets = [];
        this.currentPreset = null;
        this.isEditing = false;

        this.elements = {
            modal: document.getElementById('presetModal'),
            list: document.getElementById('presetList'),
            editor: document.getElementById('presetEditor'),
            closeBtn: document.getElementById('closePresetModal'),
            newBtn: document.getElementById('newPresetBtn'),
            saveBtn: document.getElementById('savePresetBtn'),
            deleteBtn: document.getElementById('deletePresetBtn'),
            testBtn: document.getElementById('testPresetBtn'),
            nameInput: document.getElementById('presetName'),
            descInput: document.getElementById('presetDescription'),
            categorySelect: document.getElementById('presetCategory'),
            templateInput: document.getElementById('presetTemplate')
        };

        this.bindEvents();
    }

    bindEvents() {
        if (this.elements.closeBtn) {
            this.elements.closeBtn.addEventListener('click', () => this.close());
        }
        if (this.elements.newBtn) {
            this.elements.newBtn.addEventListener('click', () => this.newPreset());
        }
        if (this.elements.saveBtn) {
            this.elements.saveBtn.addEventListener('click', () => this.savePreset());
        }
        if (this.elements.deleteBtn) {
            this.elements.deleteBtn.addEventListener('click', () => this.deletePreset());
        }
        if (this.elements.testBtn) {
            this.elements.testBtn.addEventListener('click', () => this.testPreset());
        }

        // Close on outside click
        if (this.elements.modal) {
            this.elements.modal.addEventListener('click', (e) => {
                if (e.target === this.elements.modal) {
                    this.close();
                }
            });
        }
    }

    async open() {
        await this.loadPresets();
        if (this.elements.modal) {
            this.elements.modal.style.display = 'flex';
        }
    }

    close() {
        if (this.elements.modal) {
            this.elements.modal.style.display = 'none';
        }
        this.hideEditor();
    }

    async loadPresets() {
        try {
            const response = await fetch('/api/prompts');
            const data = await response.json();
            this.presets = data.presets || [];
            this.renderList();
        } catch (error) {
            console.error('Failed to load presets:', error);
        }
    }

    renderList() {
        if (!this.elements.list) return;

        this.elements.list.innerHTML = this.presets.map(preset => `
            <div class="preset-item ${preset.is_builtin ? 'builtin' : ''}" data-id="${preset.id}">
                <div class="preset-info">
                    <span class="preset-name">${preset.name}</span>
                    <span class="preset-category">${preset.category}</span>
                </div>
                <div class="preset-actions">
                    <button class="btn-icon edit-btn" title="编辑">✏️</button>
                    ${!preset.is_builtin ? '<button class="btn-icon delete-btn" title="删除">🗑️</button>' : ''}
                </div>
            </div>
        `).join('');

        // Bind click events
        this.elements.list.querySelectorAll('.preset-item').forEach(item => {
            item.addEventListener('click', (e) => {
                const id = item.dataset.id;
                if (e.target.classList.contains('delete-btn')) {
                    this.confirmDelete(id);
                } else if (e.target.classList.contains('edit-btn')) {
                    this.editPreset(id);
                } else {
                    this.selectPreset(id);
                }
            });
        });
    }

    selectPreset(id) {
        const preset = this.presets.find(p => p.id === id);
        if (!preset) return;

        this.currentPreset = preset;
        this.showEditor(preset);
    }

    showEditor(preset = null) {
        if (!this.elements.editor) return;

        this.elements.editor.style.display = 'block';
        this.isEditing = !!preset;

        if (preset) {
            this.elements.nameInput.value = preset.name || '';
            this.elements.descInput.value = preset.description || '';
            this.elements.categorySelect.value = preset.category || 'custom';
            this.elements.templateInput.value = preset.prompt_template || '';

            // Show/hide buttons
            this.elements.saveBtn.style.display = 'inline-block';
            this.elements.deleteBtn.style.display = preset.is_builtin ? 'none' : 'inline-block';
        } else {
            this.elements.nameInput.value = '';
            this.elements.descInput.value = '';
            this.elements.categorySelect.value = 'custom';
            this.elements.templateInput.value = '';

            this.elements.saveBtn.style.display = 'inline-block';
            this.elements.deleteBtn.style.display = 'none';
        }
    }

    hideEditor() {
        if (this.elements.editor) {
            this.elements.editor.style.display = 'none';
        }
        this.currentPreset = null;
        this.isEditing = false;
    }

    newPreset() {
        this.currentPreset = null;
        this.showEditor();
    }

    editPreset(id) {
        const preset = this.presets.find(p => p.id === id);
        if (preset) {
            this.currentPreset = preset;
            this.showEditor(preset);
        }
    }

    async savePreset() {
        const data = {
            name: this.elements.nameInput.value,
            description: this.elements.descInput.value,
            category: this.elements.categorySelect.value,
            prompt_template: this.elements.templateInput.value
        };

        if (!data.name || !data.prompt_template) {
            alert('请填写名称和提示词模板');
            return;
        }

        try {
            const url = this.isEditing && this.currentPreset
                ? `/api/prompts/${this.currentPreset.id}`
                : '/api/prompts';
            const method = this.isEditing ? 'PUT' : 'POST';

            const response = await fetch(url, {
                method,
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify(data)
            });

            if (!response.ok) {
                throw new Error('Failed to save preset');
            }

            await this.loadPresets();
            this.hideEditor();

            if (window.app) {
                window.app.showToast('预设已保存');
            }
        } catch (error) {
            console.error('Failed to save preset:', error);
            alert('保存预设失败');
        }
    }

    async deletePreset() {
        if (!this.currentPreset || this.currentPreset.is_builtin) return;
        this.confirmDelete(this.currentPreset.id);
    }

    async confirmDelete(id) {
        if (!confirm('确定要删除此预设吗？')) return;

        try {
            const response = await fetch(`/api/prompts/${id}`, {
                method: 'DELETE'
            });

            if (!response.ok) {
                throw new Error('Failed to delete preset');
            }

            await this.loadPresets();
            this.hideEditor();

            if (window.app) {
                window.app.showToast('预设已删除');
            }
        } catch (error) {
            console.error('Failed to delete preset:', error);
            alert('删除预设失败');
        }
    }

    async testPreset() {
        const template = this.elements.templateInput.value;
        if (!template) {
            alert('请输入提示词模板');
            return;
        }

        // Test with sample text
        const testText = '这是一段测试文本，用于预览提示词效果。';
        const rendered = template.replace('{text}', testText);

        alert('预览结果:\n\n' + rendered);
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


/**
 * LLM Config Manager
 * Manages LLM provider configurations
 */
class LLMConfigManager {
    constructor() {
        this.configs = [];
        this.providers = [];
        this.currentConfig = null;
        this.isEditing = false;

        this.elements = {
            modal: document.getElementById('llmModal'),
            list: document.getElementById('llmConfigList'),
            editor: document.getElementById('llmConfigEditor'),
            closeBtn: document.getElementById('closeLLMModal'),
            newBtn: document.getElementById('newLLMConfigBtn'),
            saveBtn: document.getElementById('saveLLMConfigBtn'),
            deleteBtn: document.getElementById('deleteLLMConfigBtn'),
            testBtn: document.getElementById('testLLMConfigBtn'),
            testResult: document.getElementById('llmTestResult'),
            providerSelect: document.getElementById('llmProvider'),
            nameInput: document.getElementById('llmConfigName'),
            apiKeyInput: document.getElementById('llmApiKey'),
            apiBaseInput: document.getElementById('llmApiBase'),
            modelSelect: document.getElementById('llmModel'),
            temperatureInput: document.getElementById('llmTemperature'),
            maxTokensInput: document.getElementById('llmMaxTokens')
        };

        this.bindEvents();
    }

    bindEvents() {
        if (this.elements.closeBtn) {
            this.elements.closeBtn.addEventListener('click', () => this.close());
        }
        if (this.elements.newBtn) {
            this.elements.newBtn.addEventListener('click', () => this.newConfig());
        }
        if (this.elements.saveBtn) {
            this.elements.saveBtn.addEventListener('click', () => this.saveConfig());
        }
        if (this.elements.deleteBtn) {
            this.elements.deleteBtn.addEventListener('click', () => this.deleteConfig());
        }
        if (this.elements.testBtn) {
            this.elements.testBtn.addEventListener('click', () => this.testConnection());
        }
        if (this.elements.providerSelect) {
            this.elements.providerSelect.addEventListener('change', () => this.updateModels());
        }

        if (this.elements.modal) {
            this.elements.modal.addEventListener('click', (e) => {
                if (e.target === this.elements.modal) {
                    this.close();
                }
            });
        }
    }

    async open() {
        await this.loadProviders();
        await this.loadConfigs();
        if (this.elements.modal) {
            this.elements.modal.style.display = 'flex';
        }
    }

    close() {
        if (this.elements.modal) {
            this.elements.modal.style.display = 'none';
        }
        this.hideEditor();
    }

    async loadProviders() {
        try {
            const response = await fetch('/api/llm/providers');
            const data = await response.json();
            this.providers = data.providers || [];
        } catch (error) {
            console.error('Failed to load providers:', error);
        }
    }

    async loadConfigs() {
        try {
            const response = await fetch('/api/llm/configs');
            const data = await response.json();
            this.configs = data.configs || [];
            this.defaultConfigId = data.default_config_id;
            this.renderList();
        } catch (error) {
            console.error('Failed to load LLM configs:', error);
        }
    }

    renderList() {
        if (!this.elements.list) return;

        this.elements.list.innerHTML = this.configs.map(config => `
            <div class="llm-config-item ${config.is_default ? 'default' : ''}" data-id="${config.id}">
                <div class="llm-config-info">
                    <span class="llm-config-name">${config.name}</span>
                    <span class="llm-config-provider">${config.provider} - ${config.model}</span>
                    ${config.is_default ? '<span class="default-badge">默认</span>' : ''}
                </div>
                <div class="llm-config-actions">
                    <button class="btn-icon set-default-btn" title="设为默认">⭐</button>
                    <button class="btn-icon edit-btn" title="编辑">✏️</button>
                    <button class="btn-icon delete-btn" title="删除">🗑️</button>
                </div>
            </div>
        `).join('');

        this.elements.list.querySelectorAll('.llm-config-item').forEach(item => {
            item.addEventListener('click', (e) => {
                const id = item.dataset.id;
                if (e.target.classList.contains('delete-btn')) {
                    this.confirmDelete(id);
                } else if (e.target.classList.contains('edit-btn')) {
                    this.editConfig(id);
                } else if (e.target.classList.contains('set-default-btn')) {
                    this.setAsDefault(id);
                }
            });
        });
    }

    async updateModels() {
        const provider = this.elements.providerSelect.value;
        try {
            const response = await fetch(`/api/llm/providers/${provider}/models`);
            const data = await response.json();
            const models = data.models || [];

            this.elements.modelSelect.innerHTML = models.map(m =>
                `<option value="${m}">${m}</option>`
            ).join('');

            if (models.length === 0) {
                this.elements.modelSelect.innerHTML = '<option value="">请输入模型名称</option>';
            }
        } catch (error) {
            console.error('Failed to load models:', error);
        }
    }

    showEditor(config = null) {
        if (!this.elements.editor) return;

        this.elements.editor.style.display = 'block';
        this.isEditing = !!config;

        if (config) {
            this.elements.providerSelect.value = config.provider;
            this.elements.nameInput.value = config.name || '';
            this.elements.apiKeyInput.value = ''; // Don't show existing key
            this.elements.apiBaseInput.value = config.api_base || '';
            this.elements.modelSelect.innerHTML = `<option value="${config.model}">${config.model}</option>`;
            this.elements.temperatureInput.value = config.temperature || 0.7;
            this.elements.maxTokensInput.value = config.max_tokens || 4096;

            this.elements.saveBtn.style.display = 'inline-block';
            this.elements.deleteBtn.style.display = 'inline-block';
        } else {
            this.elements.providerSelect.value = 'deepseek';
            this.elements.nameInput.value = '';
            this.elements.apiKeyInput.value = '';
            this.elements.apiBaseInput.value = '';
            this.elements.temperatureInput.value = 0.7;
            this.elements.maxTokensInput.value = 4096;
            this.updateModels();

            this.elements.saveBtn.style.display = 'inline-block';
            this.elements.deleteBtn.style.display = 'none';
        }

        this.elements.testResult.textContent = '';
    }

    hideEditor() {
        if (this.elements.editor) {
            this.elements.editor.style.display = 'none';
        }
        this.currentConfig = null;
        this.isEditing = false;
    }

    newConfig() {
        this.currentConfig = null;
        this.showEditor();
    }

    editConfig(id) {
        const config = this.configs.find(c => c.id === id);
        if (config) {
            this.currentConfig = config;
            this.showEditor(config);
        }
    }

    async saveConfig() {
        const data = {
            provider: this.elements.providerSelect.value,
            name: this.elements.nameInput.value,
            api_key: this.elements.apiKeyInput.value,
            api_base: this.elements.apiBaseInput.value || null,
            model: this.elements.modelSelect.value,
            temperature: parseFloat(this.elements.temperatureInput.value) || 0.7,
            max_tokens: parseInt(this.elements.maxTokensInput.value) || 4096
        };

        if (!data.name) {
            alert('请输入配置名称');
            return;
        }

        if (!this.isEditing && !data.api_key) {
            alert('请输入 API 密钥');
            return;
        }

        try {
            const url = this.isEditing && this.currentConfig
                ? `/api/llm/configs/${this.currentConfig.id}`
                : '/api/llm/configs';
            const method = this.isEditing ? 'PUT' : 'POST';

            const response = await fetch(url, {
                method,
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify(data)
            });

            if (!response.ok) {
                throw new Error('Failed to save config');
            }

            await this.loadConfigs();
            this.hideEditor();

            // Update LLM config select in settings
            if (window.settingsManager) {
                window.settingsManager.loadLLMConfigs();
            }

            if (window.app) {
                window.app.showToast('配置已保存');
            }
        } catch (error) {
            console.error('Failed to save config:', error);
            alert('保存配置失败');
        }
    }

    async deleteConfig() {
        if (!this.currentConfig) return;
        this.confirmDelete(this.currentConfig.id);
    }

    async confirmDelete(id) {
        if (!confirm('确定要删除此配置吗？')) return;

        try {
            const response = await fetch(`/api/llm/configs/${id}`, {
                method: 'DELETE'
            });

            if (!response.ok) {
                throw new Error('Failed to delete config');
            }

            await this.loadConfigs();
            this.hideEditor();

            if (window.settingsManager) {
                window.settingsManager.loadLLMConfigs();
            }

            if (window.app) {
                window.app.showToast('配置已删除');
            }
        } catch (error) {
            console.error('Failed to delete config:', error);
            alert('删除配置失败');
        }
    }

    async setAsDefault(id) {
        try {
            const response = await fetch(`/api/llm/configs/${id}/default`, {
                method: 'PUT'
            });

            if (!response.ok) {
                throw new Error('Failed to set default');
            }

            await this.loadConfigs();

            if (window.settingsManager) {
                window.settingsManager.loadLLMConfigs();
            }

            if (window.app) {
                window.app.showToast('已设为默认服务商');
            }
        } catch (error) {
            console.error('Failed to set default:', error);
            alert('设置默认失败');
        }
    }

    async testConnection() {
        if (!this.currentConfig) {
            // For new config, just validate inputs
            if (!this.elements.apiKeyInput.value) {
                alert('请先输入 API 密钥');
                return;
            }
            this.elements.testResult.textContent = '请先保存配置后再测试';
            return;
        }

        this.elements.testResult.textContent = '测试中...';

        try {
            const response = await fetch(`/api/llm/configs/${this.currentConfig.id}/test`, {
                method: 'POST'
            });
            const data = await response.json();

            if (data.success) {
                this.elements.testResult.innerHTML = '<span style="color: green;">✓ 连接成功</span>';
            } else {
                this.elements.testResult.innerHTML = `<span style="color: red;">✗ ${data.error || '连接失败'}</span>`;
            }
        } catch (error) {
            this.elements.testResult.innerHTML = `<span style="color: red;">✗ 测试失败</span>`;
        }
    }
}

// Global settings manager
window.settingsManager = new SettingsManager();
window.llmConfigManager = new LLMConfigManager();
