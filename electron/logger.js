const fs = require('fs')
const path = require('path')

const logDir = path.join(__dirname, '..', 'logs')
const logFile = path.join(logDir, 'electron-overlay.log')

function serializeDetails(details) {
  if (details == null) {
    return ''
  }

  try {
    return ` ${JSON.stringify(details)}`
  } catch (error) {
    return ` ${JSON.stringify({ serialization_error: error.message })}`
  }
}

function logElectronOverlay(scope, message, details) {
  try {
    fs.mkdirSync(logDir, { recursive: true })
    fs.appendFileSync(
      logFile,
      `${new Date().toISOString()} [${scope}] ${message}${serializeDetails(details)}\n`,
      'utf8'
    )
  } catch (error) {
    console.error('[ElectronOverlayLogger] Failed to write log:', error)
  }
}

module.exports = {
  logElectronOverlay,
  electronOverlayLogFile: logFile
}
