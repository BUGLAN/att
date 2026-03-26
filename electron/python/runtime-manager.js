const { spawn } = require('child_process')
const path = require('path')
const http = require('http')

class PythonManager {
  constructor() {
    this.process = null
    this.port = null
    this.isRunning = false
    this.startTime = null
  }

  getPythonPath() {
    if (process.env.NODE_ENV === 'development') {
      return process.platform === 'win32' ? 'python' : 'python3'
    }

    return process.platform === 'win32'
      ? path.join(process.resourcesPath, 'python', 'python.exe')
      : path.join(process.resourcesPath, 'python', 'bin', 'python3')
  }

  getRuntimeRoot() {
    if (process.env.NODE_ENV === 'development') {
      return path.join(__dirname, '..', '..')
    }

    return process.resourcesPath
  }

  async findAvailablePort(startPort = 8000, maxAttempts = 100) {
    for (let port = startPort; port < startPort + maxAttempts; port += 1) {
      const available = await this.isPortAvailable(port)
      if (available) {
        return port
      }
    }

    throw new Error('No available port found')
  }

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

  async waitForReady(maxWaitTime = 30000) {
    const startTime = Date.now()

    while (Date.now() - startTime < maxWaitTime) {
      try {
        const response = await this.healthCheck()
        if (response) {
          return true
        }
      } catch (error) {
        // Continue waiting for the backend.
      }

      await this.sleep(500)
    }

    return false
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

  getStartupTimeout() {
    const rawTimeout = process.env.BACKEND_START_TIMEOUT_MS || '120000'
    const parsedTimeout = Number.parseInt(rawTimeout, 10)

    if (Number.isFinite(parsedTimeout) && parsedTimeout > 0) {
      return parsedTimeout
    }

    return 120000
  }

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

  async start() {
    if (this.isRunning) {
      return this.port
    }

    this.port = await this.findAvailablePort()

    const pythonPath = this.getPythonPath()
    const runtimeRoot = this.getRuntimeRoot()
    const env = {
      ...process.env,
      PYTHONUNBUFFERED: '1',
      PORT: this.port.toString(),
      PYTHONPATH: process.env.PYTHONPATH
        ? `${runtimeRoot}${path.delimiter}${process.env.PYTHONPATH}`
        : runtimeRoot
    }

    this.process = this.spawnBackendProcess(pythonPath, [
      '-m', 'uvicorn',
      'app.main:app',
      '--host', '127.0.0.1',
      '--port', this.port.toString()
    ], {
      cwd: runtimeRoot,
      env,
      stdio: ['pipe', 'pipe', 'pipe']
    })

    this.process.stdout.on('data', (data) => {
      console.log(`[Python] ${data.toString().trim()}`)
    })

    this.process.stderr.on('data', (data) => {
      console.error(`[Python Error] ${data.toString().trim()}`)
    })

    this.process.on('close', (code) => {
      console.log(`Python process exited with code ${code}`)
      this.isRunning = false
    })

    this.process.on('error', (error) => {
      console.error('Failed to start Python process:', error)
      this.isRunning = false
    })

    const ready = await this.waitForReady(this.getStartupTimeout())

    if (ready) {
      this.isRunning = true
      this.startTime = Date.now()
      return this.port
    }

    this.stop()
    throw new Error('Python backend failed to start within timeout')
  }

  stop() {
    if (!this.process) {
      return
    }

    if (process.platform === 'win32') {
      spawn('taskkill', ['/pid', this.process.pid.toString(), '/f', '/t'])
    } else {
      this.process.kill('SIGTERM')
    }

    this.process = null
    this.isRunning = false
  }

  async restart() {
    this.stop()
    await this.sleep(1000)
    return this.start()
  }

  getStatus() {
    return {
      isRunning: this.isRunning,
      port: this.port,
      uptime: this.startTime ? Date.now() - this.startTime : 0
    }
  }

  sleep(ms) {
    return new Promise((resolve) => setTimeout(resolve, ms))
  }
}

module.exports = { PythonManager }
