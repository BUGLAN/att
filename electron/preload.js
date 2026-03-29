const { contextBridge, ipcRenderer } = require('electron')

contextBridge.exposeInMainWorld('electronAPI', {
  platform: process.platform,
  getPort: () => ipcRenderer.invoke('backend:port'),
  readClipboardText: () => ipcRenderer.invoke('clipboard:read-text'),
  minimizeWindow: () => ipcRenderer.send('window:minimize'),
  maximizeWindow: () => ipcRenderer.send('window:maximize'),
  closeWindow: () => ipcRenderer.send('window:close'),
  setTrayIcon: (iconType) => ipcRenderer.send('tray:set-icon', iconType),
  updateTrayMenu: (records) => ipcRenderer.send('tray:update-menu', records),
  writeClipboard: (text) => ipcRenderer.invoke('clipboard:write', text),
  pasteClipboard: () => ipcRenderer.invoke('clipboard:paste-active'),
  notifyClipboardOverlayDrag: (active) => ipcRenderer.send('clipboard-overlay:drag-state', { active }),
  updateHotkey: (hotkey) => ipcRenderer.invoke('hotkey:update', hotkey),
  syncInputSettings: (settings) => ipcRenderer.invoke('desktop:sync-input-settings', settings),
  syncDesktopSettings: (settings) => ipcRenderer.invoke('desktop:sync-settings', settings),
  syncRecordingState: (payload) => ipcRenderer.invoke('desktop:recording-state', payload),
  showFloatingWindow: () => ipcRenderer.send('floating:show'),
  hideFloatingWindow: () => ipcRenderer.send('floating:hide'),
  toggleFloatingWindow: () => ipcRenderer.send('floating:toggle'),
  updateFloatingWindowState: (payload) => ipcRenderer.send('floating:update-state', payload),
  showClipboardOverlay: (payload) => ipcRenderer.send('clipboard-overlay:show', payload),
  hideClipboardOverlay: () => ipcRenderer.send('clipboard-overlay:hide'),
  toggleClipboardOverlay: (payload) => ipcRenderer.send('clipboard-overlay:toggle', payload),
  toggleClipboardOverlayPinned: () => ipcRenderer.send('clipboard-overlay:toggle-pin'),
  startClipboardOverlayNativeDrag: (payload) => ipcRenderer.send('clipboard-overlay:start-native-drag', payload),
  debugLog: (scope, message, details) => ipcRenderer.send('debug:log', { scope, message, details }),
  openSettingsPage: () => ipcRenderer.send('main:open-settings'),
  onHotkeyToggle: (callback) => {
    ipcRenderer.on('hotkey:toggle', () => callback())
  },
  onPasteText: (callback) => {
    ipcRenderer.on('paste-text', () => callback())
  },
  onNavigateSettings: (callback) => {
    ipcRenderer.on('navigate:settings', () => callback())
  },
  onClipboardOverlayActivate: (callback) => {
    ipcRenderer.on('clipboard-overlay:activate', (event, payload) => callback(payload))
  },
  onClipboardOverlayPinnedChanged: (callback) => {
    ipcRenderer.on('clipboard-overlay:pinned-changed', (event, payload) => callback(payload))
  },
  onClipboardChanged: (callback) => {
    ipcRenderer.on('clipboard:changed', (event, payload) => callback(payload))
  },
  removeAllListeners: (channel) => {
    ipcRenderer.removeAllListeners(channel)
  }
})
