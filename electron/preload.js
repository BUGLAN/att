const { contextBridge, ipcRenderer } = require('electron')

contextBridge.exposeInMainWorld('electronAPI', {
  platform: process.platform,
  getPort: () => ipcRenderer.invoke('backend:port'),
  openMediaFile: () => ipcRenderer.invoke('dialog:open-media-file'),
  readClipboardText: () => ipcRenderer.invoke('clipboard:read-text'),
  minimizeWindow: () => ipcRenderer.send('window:minimize'),
  maximizeWindow: () => ipcRenderer.send('window:maximize'),
  closeWindow: () => ipcRenderer.send('window:close'),
  setTrayIcon: (iconType) => ipcRenderer.send('tray:set-icon', iconType),
  updateTrayMenu: (records) => ipcRenderer.send('tray:update-menu', records),
  writeClipboard: (text) => ipcRenderer.invoke('clipboard:write', text),
  writeClipboardImage: (payload) => ipcRenderer.invoke('clipboard:write-image', payload),
  pasteClipboard: () => ipcRenderer.invoke('clipboard:paste-active'),
  syncInputSettings: (settings) => ipcRenderer.invoke('desktop:sync-input-settings', settings),
  syncDesktopSettings: (settings) => ipcRenderer.invoke('desktop:sync-settings', settings),
  syncRecordingState: (payload) => ipcRenderer.invoke('desktop:recording-state', payload),
  getDesktopSettings: () => ipcRenderer.invoke('desktop:settings:get'),
  saveDesktopSettings: (settings) => ipcRenderer.invoke('desktop:settings:save', settings),
  showFloatingWindow: () => ipcRenderer.send('floating:show'),
  hideFloatingWindow: () => ipcRenderer.send('floating:hide'),
  toggleFloatingWindow: () => ipcRenderer.send('floating:toggle'),
  updateFloatingWindowState: (payload) => ipcRenderer.send('floating:update-state', payload),
  debugLog: (scope, message, details) => ipcRenderer.send('debug:log', { scope, message, details }),
  onHotkeyPress: (callback) => {
    ipcRenderer.on('hotkey:press', (event, payload) => callback(payload))
  },
  onHotkeyRelease: (callback) => {
    ipcRenderer.on('hotkey:release', (event, payload) => callback(payload))
  },
  onPasteText: (callback) => {
    ipcRenderer.on('paste-text', () => callback())
  },
  onNavigateSettings: (callback) => {
    ipcRenderer.on('navigate:settings', () => callback())
  },
  onClipboardChanged: (callback) => {
    ipcRenderer.on('clipboard:changed', (event, payload) => callback(payload))
  },
  onDesktopPermissionRequired: (callback) => {
    ipcRenderer.on('desktop:permission-required', (event, payload) => callback(payload))
  },
  onDesktopAgentError: (callback) => {
    ipcRenderer.on('desktop:agent-error', (event, payload) => callback(payload))
  },
  removeAllListeners: (channel) => {
    ipcRenderer.removeAllListeners(channel)
  }
})
