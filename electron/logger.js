const fs = require('fs')
const path = require('path')

const logDir = path.join(__dirname, '..', 'logs')
const logFile = path.join(logDir, 'electron.log')

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

// Electron 主进程通用日志：后端进程、桌面代理、剪贴板监控、托盘等共用一份文件
function logElectron(scope, message, details) {
  try {
    fs.mkdirSync(logDir, { recursive: true })
    fs.appendFileSync(
      logFile,
      `${new Date().toISOString()} [${scope}] ${message}${serializeDetails(details)}\n`,
      'utf8'
    )
  } catch (error) {
    console.error('[ElectronLogger] Failed to write log:', error)
  }
}

module.exports = {
  logElectron,
  electronLogFile: logFile
}
