const { contextBridge, ipcRenderer } = require('electron')

contextBridge.exposeInMainWorld('electronAPI', {
  platform: process.platform,
  getPort: () => ipcRenderer.invoke('backend:port'),
  minimizeWindow: () => ipcRenderer.send('window:minimize'),
  maximizeWindow: () => ipcRenderer.send('window:maximize'),
  closeWindow: () => ipcRenderer.send('window:close'),
  setTrayIcon: (iconType) => ipcRenderer.send('tray:set-icon', iconType),
  updateTrayMenu: (records) => ipcRenderer.send('tray:update-menu', records),
  writeClipboard: (text) => ipcRenderer.invoke('clipboard:write', text),
  pasteClipboard: () => ipcRenderer.invoke('clipboard:paste-active'),
  updateHotkey: (hotkey) => ipcRenderer.invoke('hotkey:update', hotkey),
  syncDesktopSettings: (settings) => ipcRenderer.invoke('desktop:sync-settings', settings),
  syncRecordingState: (payload) => ipcRenderer.invoke('desktop:recording-state', payload),
  onHotkeyStart: (callback) => {
    ipcRenderer.on('hotkey:start', () => callback())
  },
  onHotkeyStop: (callback) => {
    ipcRenderer.on('hotkey:stop', () => callback())
  },
  onPasteText: (callback) => {
    ipcRenderer.on('paste-text', () => callback())
  },
  onNavigateSettings: (callback) => {
    ipcRenderer.on('navigate:settings', () => callback())
  },
  removeAllListeners: (channel) => {
    ipcRenderer.removeAllListeners(channel)
  }
})
