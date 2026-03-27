const { contextBridge, ipcRenderer } = require('electron')

contextBridge.exposeInMainWorld('floatingAPI', {
  close: () => ipcRenderer.send('floating:hide'),
  openSettings: () => ipcRenderer.send('main:open-settings'),
  onStateChange: (callback) => {
    ipcRenderer.on('floating:state', (event, payload) => callback(payload))
  },
  removeAllListeners: (channel) => {
    ipcRenderer.removeAllListeners(channel)
  }
})
