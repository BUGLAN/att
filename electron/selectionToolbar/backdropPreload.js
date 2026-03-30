const { ipcRenderer } = require('electron')

window.addEventListener('mousedown', (event) => {
  ipcRenderer.send('selection-toolbar:backdrop-pointer-down', {
    button: event.button
  })
})

window.addEventListener('contextmenu', (event) => {
  event.preventDefault()
})
