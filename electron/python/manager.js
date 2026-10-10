/**
 * Python 进程管理器
 * 负责启动、停止和监控 Python 后端进程
 */

const { spawn } = require('child_process')
const path = require('path')
const fs = require('fs')
const http = require('http')

let electronApp = null
try {
  // 允许在没有 Electron 运行时（例如 Node 单测）下加载本模块
  ;({ app: electronApp } = require('electron'))
} catch (error) {
  electronApp = null
}

class PythonManager {
  constructor() {
    this.process = null
    this.port = null
    this.isRunning = false
    this.startTime = null
    this.externalBackend = false
  }

  isDevelopmentRuntime() {
    const runtimeRoot = path.join(__dirname, '..', '..')
    const markers = [
      path.join(runtimeRoot, 'app', 'main.py'),
      path.join(runtimeRoot, 'package.json'),
      path.join(runtimeRoot, '.venv')
    ]

    // 打包态（app.isPackaged === true）一律算生产运行，避免安装目录里恰好存在同名文件时误判
    if (electronApp && electronApp.isPackaged === true) {
      return false
    }

    return process.env.NODE_ENV === 'development' || markers.some((marker) => fs.existsSync(marker))
  }

  /**
   * 判断当前是否运行在「已打包」的安装态。
   * 只有安装态才把数据目录切到用户目录；开发态必须继续使用仓库内 data/。
   */
  isPackagedRuntime() {
    if (electronApp && typeof electronApp.isPackaged === 'boolean') {
      return electronApp.isPackaged
    }

    return !this.isDevelopmentRuntime()
  }

  /**
   * 解析安装态应使用的数据目录：<userData>/data。
   *
   * 这里必须只用 app.getPath('userData')，不得退回 resourcesPath：
   * 安装目录（Program Files）通常不可写，而且安装目录里的 data/ 会在卸载时被删除。
   * 返回 null 表示「不要注入 DATA_DIR」，交给后端使用默认的仓库内 data/。
   */
  getPackagedDataDir() {
    if (!this.isPackagedRuntime()) {
      return null
    }

    if (!electronApp || typeof electronApp.getPath !== 'function') {
      console.warn('[PythonManager] 运行在打包态但拿不到 userData 路径，回退到默认数据目录')
      return null
    }

    const dataDir = path.join(electronApp.getPath('userData'), 'data')

    // 先建目录：后端首次启动时 data/ 子目录由后端自己创建，但根目录需要先存在
    try {
      fs.mkdirSync(dataDir, { recursive: true })
    } catch (error) {
      console.error(`[PythonManager] 创建用户数据目录失败：${dataDir}`, error)
    }

    return dataDir
  }

  getPreferredBackendPort() {
    const rawPort = process.env.ATT_BACKEND_PORT || process.env.PORT || '8000'
    const parsedPort = Number.parseInt(rawPort, 10)
    return Number.isFinite(parsedPort) && parsedPort > 0 ? parsedPort : 8000
  }

  shouldRequireExistingBackend() {
    const explicitValue = process.env.ATT_REQUIRE_EXISTING_BACKEND
    if (explicitValue === '1') {
      return true
    }

    if (explicitValue === '0') {
      return false
    }

    return false
  }

  getRuntimeRoot() {
    const isDev = this.isDevelopmentRuntime()

    if (isDev) {
      return path.join(__dirname, '..', '..')
    }

    return process.resourcesPath
  }

  getProjectVenvPython() {
    const runtimeRoot = this.getRuntimeRoot()
    return process.platform === 'win32'
      ? path.join(runtimeRoot, '.venv', 'Scripts', 'python.exe')
      : path.join(runtimeRoot, '.venv', 'bin', 'python3')
  }

  /**
   * 获取后端启动命令
   */
  getBackendCommand() {
    const isDev = this.isDevelopmentRuntime()

    if (isDev) {
      const projectPython = this.getProjectVenvPython()
      if (fs.existsSync(projectPython)) {
        return projectPython
      }

      return 'uv'
    }

    const pythonDir = process.platform === 'win32'
      ? path.join(process.resourcesPath, 'python', 'Scripts', 'python.exe')
      : path.join(process.resourcesPath, 'python', 'bin', 'python3')
    return pythonDir
  }

  /**
   * 获取后端启动参数
   */
  getBackendArgs(port) {
    const isDev = this.isDevelopmentRuntime()

    if (isDev) {
      const projectPython = this.getProjectVenvPython()
      if (fs.existsSync(projectPython)) {
        return [
          '-m',
          'uvicorn',
          'app.main:app',
          '--host',
          '127.0.0.1',
          '--port',
          port.toString()
        ]
      }

      return [
        'run',
        '--no-sync',
        'python',
        '-m',
        'uvicorn',
        'app.main:app',
        '--host',
        '127.0.0.1',
        '--port',
        port.toString()
      ]
    }

    return [
      '-m', 'uvicorn',
      'app.main:app',
      '--host', '127.0.0.1',
      '--port', port.toString()
    ]
  }

  /**
   * 获取用于执行辅助脚本的 Python 路径
   */
  getPythonPath() {
    const isDev = this.isDevelopmentRuntime()

    if (isDev) {
      const projectPython = this.getProjectVenvPython()
      if (process.platform === 'win32' && fs.existsSync(projectPython)) {
        return projectPython
      }

      return process.platform === 'win32' ? 'python' : 'python3'
    }

    return process.platform === 'win32'
      ? path.join(process.resourcesPath, 'python', 'Scripts', 'python.exe')
      : path.join(process.resourcesPath, 'python', 'bin', 'python3')
  }

  /**
   * 查找可用端口
   */
  async findAvailablePort(startPort = 8000, maxAttempts = 100) {
    for (let port = startPort; port < startPort + maxAttempts; port++) {
      const available = await this.isPortAvailable(port)
      if (available) {
        return port
      }
    }
    throw new Error('No available port found')
  }

  /**
   * 检查端口是否可用
   */
  isPortAvailable(port) {
    return new Promise((resolve) => {
      const server = http.createServer()
      server.once('error', () => resolve(false))
      server.once('listening', () => {
        server.close()
        resolve(true)
      })
      server.listen(port)
    })
  }

  /**
   * 等待后端就绪
   */
  async waitForReady(maxWaitTime = 30000) {
    const startTime = Date.now()

    while (Date.now() - startTime < maxWaitTime) {
      try {
        const response = await this.healthCheck()
        if (response) {
          return true
        }
      } catch (error) {
        // 继续等待
      }
      await this.sleep(500)
    }

    return false
  }

  /**
   * 健康检查
   */
  healthCheck() {
    return new Promise((resolve, reject) => {
      const req = http.request({
        hostname: '127.0.0.1',
        port: this.port,
        path: '/api/health',
        method: 'GET',
        timeout: 5000
      }, (res) => {
        if (res.statusCode === 200) {
          res.resume()
          resolve(true)
        } else {
          res.resume()
          reject(new Error(`Health check failed: ${res.statusCode}`))
        }
      })

      req.on('error', reject)
      req.on('timeout', () => {
        req.destroy()
        reject(new Error('Health check timeout'))
      })
      req.end()
    })
  }

  /**
   * 启动 Python 后端
   */
  async start() {
    if (this.isRunning) {
      return this.port
    }

    const preferredPort = this.getPreferredBackendPort()
    this.port = preferredPort

    try {
      const isHealthy = await this.healthCheck()
      if (isHealthy) {
        this.isRunning = true
        this.externalBackend = true
        this.startTime = Date.now()
        console.log(`Using existing backend on port ${this.port}`)
        return this.port
      }
    } catch (error) {
      console.log(`[PythonManager] No reusable backend detected on port ${preferredPort}: ${error.message}`)
    }

    if (this.shouldRequireExistingBackend()) {
      this.port = null
      throw new Error(
        `Existing backend is required but not reachable on 127.0.0.1:${preferredPort}. ` +
        'Please start the backend first, or run the default Electron dev command to auto-start a local backend.'
      )
    }

    this.externalBackend = false

    // 查找可用端口
    this.port = await this.findAvailablePort(preferredPort)

    const backendCommand = this.getBackendCommand()
    const backendArgs = this.getBackendArgs(this.port)

    // 设置环境变量
    const env = {
      ...process.env,
      PYTHONUNBUFFERED: '1',
      HF_HUB_DISABLE_SYMLINKS_WARNING: '1',
      PORT: this.port.toString(),
      UV_CACHE_DIR: process.env.UV_CACHE_DIR || path.join(__dirname, '..', '..', 'tmp', 'uv-cache'),
      UV_PROJECT_ENVIRONMENT: process.env.UV_PROJECT_ENVIRONMENT || path.join(__dirname, '..', '..', '.venv')
    }

    // 仅安装态注入 DATA_DIR：开发态保持仓库内 data/ 不变（用户真实数据在那里）
    const packagedDataDir = this.getPackagedDataDir()
    if (packagedDataDir) {
      env.DATA_DIR = packagedDataDir
      console.log(`[PythonManager] 安装态数据目录：${packagedDataDir}`)
    }

    // 启动进程 - cwd 必须是项目根目录，才能找到 app 模块
    this.process = spawn(backendCommand, backendArgs, {
      cwd: this.getRuntimeRoot(),  // 开发态=仓库根目录，安装态=resources/
      env: env,
      stdio: ['pipe', 'pipe', 'pipe']
    })

    // 处理输出
    this.process.stdout.on('data', (data) => {
      console.log(`[Python] ${data.toString().trim()}`)
    })

    this.process.stderr.on('data', (data) => {
      console.error(`[Python Error] ${data.toString().trim()}`)
    })

    // 处理进程退出
    this.process.on('close', (code) => {
      console.log(`Python process exited with code ${code}`)
      this.isRunning = false
      this.externalBackend = false
    })

    // 处理进程错误
    this.process.on('error', (error) => {
      console.error('Failed to start Python process:', error)
      this.isRunning = false
      this.externalBackend = false
    })

    // 等待后端就绪
    const ready = await this.waitForReady()

    if (ready) {
      this.isRunning = true
      this.startTime = Date.now()
      return this.port
    } else {
      this.stop()
      throw new Error('Python backend failed to start within timeout')
    }
  }

  /**
   * 停止 Python 后端
   */
  stop() {
    if (this.externalBackend) {
      this.isRunning = false
      this.externalBackend = false
      this.port = null
      return
    }

    if (this.process) {
      // Windows 使用 taskkill，其他平台使用 SIGTERM
      if (process.platform === 'win32' && this.process.pid) {
        spawn('taskkill', ['/pid', this.process.pid.toString(), '/f', '/t'])
      } else if (this.process.pid) {
        this.process.kill('SIGTERM')
      }

      this.process = null
      this.isRunning = false
    }
  }

  /**
   * 重启 Python 后端
   */
  async restart() {
    this.stop()
    await this.sleep(1000)
    return this.start()
  }

  /**
   * 获取运行状态
   */
  getStatus() {
    return {
      isRunning: this.isRunning,
      port: this.port,
      externalBackend: this.externalBackend,
      uptime: this.startTime ? Date.now() - this.startTime : 0
    }
  }

  /**
   * 辅助函数：睡眠
   */
  sleep(ms) {
    return new Promise(resolve => setTimeout(resolve, ms))
  }
}

module.exports = { PythonManager }
