const { EventEmitter } = require('events')
const { spawn } = require('child_process')

class DesktopAgentManager extends EventEmitter {
  constructor(options = {}) {
    super()
    this.pythonPath = options.pythonPath
    this.scriptPath = options.scriptPath
    this.runtimeRoot = options.runtimeRoot
    this.spawnProcess = options.spawnProcess || spawn
    this.log = options.log || (() => {})
    this.requestId = 0
    this.process = null
    this.stdoutBuffer = ''
    this.pendingRequests = new Map()
    this.isReady = false
    this.isStopping = false
  }

  async start(settings = {}) {
    if (this.process) {
      return
    }

    this.isStopping = false
    this.process = this.spawnProcess(this.pythonPath, [this.scriptPath], {
      cwd: this.runtimeRoot,
      env: {
        ...process.env,
        PYTHONUNBUFFERED: '1'
      },
      stdio: ['pipe', 'pipe', 'pipe']
    })

    this.process.stdout.on('data', (chunk) => {
      this.handleStdoutChunk(chunk)
    })
    this.process.stderr.on('data', (chunk) => {
      const message = chunk.toString()
      this.log('Desktop agent stderr', { message })
      // pynput 在 macOS 缺少辅助功能权限时会向 stderr 输出 "not trusted"
      // 警告并静默失效，这里兜底转成权限事件，确保界面一定能收到提示
      if (/not trusted/i.test(message)) {
        this.emit('permission:required', {
          capability: 'keyboard',
          message: '桌面热键监听需要系统辅助功能权限，请在 系统设置 → 隐私与安全性 → 辅助功能 中授权后重启应用'
        })
      }
    })
    this.process.on('close', (code) => {
      const expected = this.isStopping
      this.log('Desktop agent exited', { code })
      this.rejectPendingRequests(new Error('Desktop agent exited before completing pending requests'))
      this._resetState()
      this.emit('exit', { code, expected })
    })
    this.process.on('error', (error) => {
      this.log('Desktop agent failed', { error: error.message })
      this.rejectPendingRequests(error)
      this._resetState()
      this.emit('agent:error', { error: error.message })
    })

    await this.sendRequest('agent.init', {
      platform: process.platform,
      settings
    })
    this.isReady = true
  }

  async updateHotkey(hotkey) {
    return this.sendRequest('hotkey.update', { hotkey })
  }

  async updateMutePolicy(settings) {
    return this.sendRequest('mute.set_policy', settings)
  }

  async setRecordingActive(recording) {
    return this.sendRequest('recording.set_active', { recording: recording === true })
  }

  async stop() {
    if (!this.process) {
      return
    }

    this.isStopping = true
    try {
      await this.sendRequest('agent.shutdown', {})
    } catch (error) {
      this.log('Desktop agent shutdown request failed', {
        error: error.message
      })
    }

    if (this.process) {
      this.process.kill()
      this.process = null
      this.isReady = false
    }
  }

  sendRequest(method, params = {}) {
    if (!this.process?.stdin) {
      return Promise.reject(new Error('Desktop agent is not running'))
    }

    const id = `${Date.now()}-${this.requestId += 1}`
    const message = {
      jsonrpc: '2.0',
      id,
      method,
      params
    }

    return new Promise((resolve, reject) => {
      this.pendingRequests.set(id, { resolve, reject })
      this.process.stdin.write(`${JSON.stringify(message)}\n`)
    })
  }

  handleStdoutChunk(chunk) {
    this.stdoutBuffer += chunk.toString()

    while (true) {
      const newlineIndex = this.stdoutBuffer.indexOf('\n')
      if (newlineIndex < 0) {
        return
      }

      const line = this.stdoutBuffer.slice(0, newlineIndex).trim()
      this.stdoutBuffer = this.stdoutBuffer.slice(newlineIndex + 1)

      if (!line) {
        continue
      }

      try {
        const message = JSON.parse(line)
        this.handleMessage(message)
      } catch (error) {
        this.log('Desktop agent stdout parse failed', {
          line,
          error: error.message
        })
      }
    }
  }

  _resetState() {
    this.process = null
    this.isReady = false
    this.isStopping = false
  }

  rejectPendingRequests(error) {
    if (this.pendingRequests.size === 0) {
      return
    }

    for (const [requestId, pending] of this.pendingRequests.entries()) {
      pending.reject(error)
      this.pendingRequests.delete(requestId)
    }
  }

  handleMessage(message) {
    if (message.id && this.pendingRequests.has(message.id)) {
      const pending = this.pendingRequests.get(message.id)
      this.pendingRequests.delete(message.id)
      if (message.error) {
        pending.reject(new Error(message.error.message || 'Desktop agent request failed'))
        return
      }
      pending.resolve(message.result)
      return
    }

    if (message.method === 'hotkey.press') {
      this.emit('hotkey:press', message.params || {})
      return
    }

    if (message.method === 'hotkey.release') {
      this.emit('hotkey:release', message.params || {})
      return
    }

    if (message.method === 'permission.required') {
      this.emit('permission:required', message.params || {})
      return
    }

    if (message.method === 'agent.error') {
      this.emit('agent:error', message.params || {})
      return
    }

    if (message.method === 'agent.ready') {
      this.emit('ready', message.params || {})
    }
  }
}

module.exports = {
  DesktopAgentManager
}
