const { spawn } = require('child_process')
const path = require('path')

let electronApp = null
try {
  // 允许在没有 Electron 运行时（例如 Node 单测）下加载本模块
  ;({ app: electronApp } = require('electron'))
} catch (error) {
  electronApp = null
}

// 注意：本类只承担「路径解析 + 后端进程 spawn」工具职责。
// 真正启动/停止后端的是 electron/python/manager.js 的 pythonManager；
// 历史上这里的 start()/stop() 启动路径从未被调用，已在功能减法中删除。
class PythonManager {
  getProjectVenvPython() {
    const runtimeRoot = this.getRuntimeRoot()
    return process.platform === 'win32'
      ? path.join(runtimeRoot, '.venv', 'Scripts', 'python.exe')
      : path.join(runtimeRoot, '.venv', 'bin', 'python3')
  }

  getPythonPath() {
    if (process.env.NODE_ENV === 'development') {
      const projectPython = this.getProjectVenvPython()
      return process.platform === 'win32' && require('fs').existsSync(projectPython)
        ? projectPython
        : (process.platform === 'win32' ? 'python' : 'python3')
    }

    return process.platform === 'win32'
      ? path.join(process.resourcesPath, 'python', 'python.exe')
      : path.join(process.resourcesPath, 'python', 'bin', 'python3')
  }

  /**
   * 判断当前是否运行在开发态（仓库内运行）。
   * 打包态（app.isPackaged === true）优先判定，避免安装目录内存在 app/ 时误判成开发态。
   */
  isDevelopmentRuntime() {
    if (electronApp && electronApp.isPackaged === true) {
      return false
    }

    return process.env.NODE_ENV === 'development'
  }

  getRuntimeRoot() {
    if (this.isDevelopmentRuntime()) {
      return path.join(__dirname, '..', '..')
    }

    return process.resourcesPath
  }

  escapeWindowsArg(value) {
    if (!value) {
      return '""'
    }

    if (!/[ \t"]/u.test(value)) {
      return value
    }

    return `"${value.replace(/(\\*)"/g, '$1$1\\"').replace(/(\\+)$/g, '$1$1')}"`
  }

  spawnBackendProcess(pythonPath, args, options) {
    try {
      return spawn(pythonPath, args, options)
    } catch (error) {
      if (process.platform !== 'win32' || error.code !== 'EPERM') {
        throw error
      }

      console.warn(`[Python] Direct spawn failed with EPERM, retrying through cmd.exe: ${pythonPath}`)
      const commandLine = [pythonPath, ...args]
        .map((value) => this.escapeWindowsArg(value))
        .join(' ')

      return spawn('cmd.exe', ['/d', '/s', '/c', commandLine], {
        ...options,
        windowsVerbatimArguments: true
      })
    }
  }
}

module.exports = { PythonManager }
