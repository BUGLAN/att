const { contextBridge, ipcRenderer } = require('electron')

const OVERLAY_CHANNELS = {
  openSelection: 'overlay:open-selection',
  openClipboard: 'overlay:open-clipboard',
  hide: 'overlay:hide',
  pin: 'overlay:pin',
  ready: 'overlay:ready',
  activate: 'overlay:activate',
  pinnedChanged: 'overlay:pinned-changed',
  actionComplete: 'overlay:action-complete'
}

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
  writeClipboardImage: (payload) => ipcRenderer.invoke('clipboard:write-image', payload),
  pasteClipboard: () => ipcRenderer.invoke('clipboard:paste-active'),
  updateHotkey: (hotkey) => ipcRenderer.invoke('hotkey:update', hotkey),
  syncInputSettings: (settings) => ipcRenderer.invoke('desktop:sync-input-settings', settings),
  syncDesktopSettings: (settings) => ipcRenderer.invoke('desktop:sync-settings', settings),
  syncRecordingState: (payload) => ipcRenderer.invoke('desktop:recording-state', payload),
  showFloatingWindow: () => ipcRenderer.send('floating:show'),
  hideFloatingWindow: () => ipcRenderer.send('floating:hide'),
  toggleFloatingWindow: () => ipcRenderer.send('floating:toggle'),
  updateFloatingWindowState: (payload) => ipcRenderer.send('floating:update-state', payload),
  openSelectionOverlay: (payload) => ipcRenderer.send(OVERLAY_CHANNELS.openSelection, payload),
  openClipboardOverlay: (payload) => ipcRenderer.send(OVERLAY_CHANNELS.openClipboard, payload),
  hideOverlay: () => ipcRenderer.send(OVERLAY_CHANNELS.hide),
  setOverlayPinned: (payload) => ipcRenderer.send(OVERLAY_CHANNELS.pin, payload),
  notifyOverlayReady: () => ipcRenderer.send(OVERLAY_CHANNELS.ready),
  notifyOverlayActionComplete: (payload) => ipcRenderer.send(OVERLAY_CHANNELS.actionComplete, payload),
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
  onOverlayActivate: (callback) => {
    ipcRenderer.on(OVERLAY_CHANNELS.activate, (event, payload) => callback(payload))
  },
  onOverlayPinnedChanged: (callback) => {
    ipcRenderer.on(OVERLAY_CHANNELS.pinnedChanged, (event, payload) => callback(payload))
  },
  onClipboardChanged: (callback) => {
    ipcRenderer.on('clipboard:changed', (event, payload) => callback(payload))
  },
  removeAllListeners: (channel) => {
    ipcRenderer.removeAllListeners(channel)
  }
})
