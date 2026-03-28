const fs = require('fs')
const path = require('path')
const { app } = require('electron')

function hasDevelopmentMarkers() {
  const repoRoot = path.join(__dirname, '..', '..')
  const markers = [
    path.join(repoRoot, 'package.json'),
    path.join(repoRoot, 'app', 'main.py'),
    path.join(repoRoot, 'src')
  ]

  return markers.every((markerPath) => fs.existsSync(markerPath))
}

function isDevelopmentRuntime() {
  if (process.env.NODE_ENV === 'development') {
    return true
  }

  if (app && app.isPackaged === false) {
    return true
  }

  return hasDevelopmentMarkers()
}

module.exports = {
  isDevelopmentRuntime
}
