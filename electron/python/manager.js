/**
 * Python 进程管理器
 * 负责启动、停止和监控 Python 后端进程
 */

const { spawn } = require('child_process')
const path = require('path')
const fs = require('fs')
const http = require('http')

class PythonManager {
  constructor() {
    this.process = null
    this.port = null
    this.isRunning = false
    this.startTime = null
  }

  /**
   * 获取 Python 可执行文件路径
   */
  getPythonPath() {
    const isDev = process.env.NODE_ENV === 'development'

    if (isDev) {
      // 开发模式：使用系统 Python
      return process.platform === 'win32' ? 'python' : 'python3'
    } else {
      // 生产模式：使用嵌入的 Python 环境
      const pythonDir = process.platform === 'win32'
        ? path.join(process.resourcesPath, 'python', 'Scripts', 'python.exe')
        : path.join(process.resourcesPath, 'python', 'bin', 'python3')
      return pythonDir
    }
  }

  /**
   * 获取后端入口文件路径
   */
  getBackendPath() {
    const isDev = process.env.NODE_ENV === 'development'

    if (isDev) {
      return path.join(__dirname, '..', '..', 'app', 'main.py')
    } else {
      return path.join(process.resourcesPath, 'app', 'main.py')
    }
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
        timeout: 2000
      }, (res) => {
        if (res.statusCode === 200) {
          resolve(true)
        } else {
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

    // 查找可用端口
    this.port = await this.findAvailablePort()

    const pythonPath = this.getPythonPath()
    const backendPath = this.getBackendPath()

    // 设置环境变量
    const env = {
      ...process.env,
      PYTHONUNBUFFERED: '1',
      PORT: this.port.toString()
    }

    // 启动进程 - cwd 必须是项目根目录，才能找到 app 模块
    this.process = spawn(pythonPath, [
      '-m', 'uvicorn',
      'app.main:app',
      '--host', '127.0.0.1',
      '--port', this.port.toString()
    ], {
      cwd: path.join(__dirname, '..', '..'),  // 项目根目录
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
    })

    // 处理进程错误
    this.process.on('error', (error) => {
      console.error('Failed to start Python process:', error)
      this.isRunning = false
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
    if (this.process) {
      // Windows 使用 taskkill，其他平台使用 SIGTERM
      if (process.platform === 'win32') {
        spawn('taskkill', ['/pid', this.process.pid.toString(), '/f', '/t'])
      } else {
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
