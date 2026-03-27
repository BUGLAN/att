const { ipcRenderer } = require('electron')

window.addEventListener('mousedown', (event) => {
  ipcRenderer.send('clipboard-overlay:backdrop-pointer-down', {
    button: event.button
  })
})

window.addEventListener('contextmenu', (event) => {
  event.preventDefault()
})
