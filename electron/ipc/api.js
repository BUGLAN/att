/**
 * IPC API 代理
 * 将渲染进程的 API 请求代理到 Python 后端
 */

const { ipcMain } = require('electron')
const fs = require('fs')
const path = require('path')
const os = require('os')
const FormData = require('form-data')
const fetch = require('node-fetch')

class APIProxy {
  constructor(pythonManager) {
    this.pythonManager = pythonManager
  }

  get baseUrl() {
    return `http://127.0.0.1:${this.pythonManager.port}/api`
  }

  /**
   * 发送 HTTP 请求
   */
  async request(method, endpoint, data = null) {
    const url = `${this.baseUrl}${endpoint}`
    const options = {
      method,
      headers: { 'Content-Type': 'application/json' }
    }

    if (data && method !== 'GET') {
      options.body = JSON.stringify(data)
    }

    const response = await fetch(url, options)
    if (!response.ok) {
      throw new Error(`HTTP ${response.status}: ${response.statusText}`)
    }
    return response.json()
  }

  /**
   * 设置 IPC 处理器
   */
  setupIPC() {
    // === 音频上传 ===
    ipcMain.handle('api:audio:upload', async (event, { buffer, filename }) => {
      const url = `${this.baseUrl}/audio/upload`

      // 将 Buffer 写入临时文件
      const tempPath = path.join(os.tmpdir(), filename)
      fs.writeFileSync(tempPath, Buffer.from(buffer))

      console.log('[API] Uploading audio file:', tempPath, 'size:', Buffer.from(buffer).length)

      try {
        // 使用 form-data 包，与 node-fetch 兼容
        const form = new FormData()
        form.append('file', fs.createReadStream(tempPath), {
          filename: filename,
          contentType: 'audio/webm'
        })

        const response = await fetch(url, {
          method: 'POST',
          headers: form.getHeaders(),
          body: form
        })

        if (!response.ok) {
          const errorText = await response.text()
          console.error('[API] Upload failed:', response.status, errorText)
          throw new Error(`HTTP ${response.status}: ${errorText}`)
        }

        const result = await response.json()
        console.log('[API] Audio upload result:', result)
        return result
      } catch (error) {
        console.error('[API] Audio upload failed:', error)
        throw error
      } finally {
        // 清理临时文件
        if (fs.existsSync(tempPath)) {
          fs.unlinkSync(tempPath)
        }
      }
    })

    // === 记录管理 ===
    ipcMain.handle('api:records:list', async () => {
      return this.request('GET', '/records')
    })

    ipcMain.handle('api:records:get', async (event, id) => {
      return this.request('GET', `/records/${id}`)
    })

    ipcMain.handle('api:records:status', async (event, id) => {
      return this.request('GET', `/records/${id}/status`)
    })

    ipcMain.handle('api:records:update', async (event, { id, data }) => {
      return this.request('PUT', `/records/${id}`, data)
    })

    ipcMain.handle('api:records:delete', async (event, id) => {
      return this.request('DELETE', `/records/${id}`)
    })

    ipcMain.handle('api:records:optimize', async (event, id) => {
      return this.request('POST', `/records/${id}/optimize`)
    })

    ipcMain.handle('api:records:export', async (event, { id, format }) => {
      return this.request('GET', `/records/${id}/export?format=${format}`)
    })

    ipcMain.handle('api:records:audio', async (event, id) => {
      // 返回音频 URL
      return `${this.baseUrl}/records/${id}/audio`
    })

    // === 设置 ===
    ipcMain.handle('api:settings:get', async () => {
      return this.request('GET', '/settings')
    })

    ipcMain.handle('api:settings:save', async (event, settings) => {
      return this.request('PUT', '/settings', settings)
    })

    // === LLM 配置 ===
    ipcMain.handle('api:llm:list', async () => {
      return this.request('GET', '/llm/configs')
    })

    ipcMain.handle('api:llm:get', async (event, id) => {
      return this.request('GET', `/llm/configs/${id}`)
    })

    ipcMain.handle('api:llm:save', async (event, config) => {
      if (config.id) {
        return this.request('PUT', `/llm/configs/${config.id}`, config)
      }
      return this.request('POST', '/llm/configs', config)
    })

    ipcMain.handle('api:llm:delete', async (event, id) => {
      return this.request('DELETE', `/llm/configs/${id}`)
    })

    ipcMain.handle('api:llm:test', async (event, id) => {
      return this.request('POST', `/llm/configs/${id}/test`)
    })

    ipcMain.handle('api:llm:set-default', async (event, id) => {
      return this.request('PUT', `/llm/configs/${id}/default`)
    })

    ipcMain.handle('api:llm:providers', async () => {
      return this.request('GET', '/llm/providers')
    })

    ipcMain.handle('api:llm:provider-models', async (event, provider) => {
      return this.request('GET', `/llm/providers/${provider}/models`)
    })

    // === 预设 ===
    ipcMain.handle('api:prompts:list', async () => {
      return this.request('GET', '/prompts')
    })

    ipcMain.handle('api:prompts:get', async (event, id) => {
      return this.request('GET', `/prompts/${id}`)
    })

    ipcMain.handle('api:prompts:save', async (event, preset) => {
      if (preset.id) {
        return this.request('PUT', `/prompts/${preset.id}`, preset)
      }
      return this.request('POST', '/prompts', preset)
    })

    ipcMain.handle('api:prompts:delete', async (event, id) => {
      return this.request('DELETE', `/prompts/${id}`)
    })

    ipcMain.handle('api:prompts:preview', async (event, { id, text }) => {
      return this.request('POST', `/prompts/${id}/preview`, { text })
    })

    ipcMain.handle('api:prompts:set-default', async (event, id) => {
      return this.request('PUT', '/prompts/default', { preset_id: id })
    })
  }
}

module.exports = { APIProxy }
