/**
 * HTTP API 客户端
 * 直接访问 Python FastAPI 后端，不经过 IPC 中转
 */

// 后端端口（通过 IPC 获取）
let backendPort = null

/**
 * 获取后端端口
 * 在 Electron 环境中通过 IPC 获取，Web 环境使用默认端口
 */
export async function getBackendPort() {
  if (backendPort) return backendPort

  // Electron 环境：通过 IPC 获取端口
  if (typeof window !== 'undefined' && window.electronAPI && window.electronAPI.getPort) {
    try {
      backendPort = await window.electronAPI.getPort()
      return backendPort
    } catch (e) {
      console.warn('[API] Failed to get port from Electron, using default:', e)
    }
  }

  // Web 环境或获取失败：使用默认端口
  backendPort = 8000
  return backendPort
}

/**
 * 获取 API 基础 URL
 */
export async function getBaseUrl() {
  const port = await getBackendPort()
  return `http://127.0.0.1:${port}/api`
}

/**
 * 发送 HTTP 请求
 * @param {string} method - HTTP 方法
 * @param {string} endpoint - API 端点
 * @param {object} options - 请求选项
 */
async function request(method, endpoint, options = {}) {
  const baseUrl = await getBaseUrl()
  const url = `${baseUrl}${endpoint}`

  const fetchOptions = {
    method,
    headers: {
      'Content-Type': 'application/json',
      ...options.headers
    }
  }

  if (options.body && method !== 'GET') {
    fetchOptions.body = JSON.stringify(options.body)
  }

  const response = await fetch(url, fetchOptions)

  if (!response.ok) {
    // 尝试获取详细错误信息
    let errorMessage = `HTTP ${response.status}: ${response.statusText}`
    try {
      const errorData = await response.json()
      if (errorData.detail) {
        errorMessage = errorData.detail
      } else if (errorData.message) {
        errorMessage = errorData.message
      }
    } catch (e) {
      // 无法解析 JSON，使用默认错误信息
    }
    throw new Error(errorMessage)
  }

  return response.json()
}

// === API 方法 ===

export const api = {
  // === 音频 ===

  /**
   * 上传音频文件
   * @param {Blob} blob - 音频 Blob
   * @param {string} filename - 文件名
   */
  async uploadAudio(blob, filename = 'recording.webm') {
    const baseUrl = await getBaseUrl()
    const url = `${baseUrl}/audio/upload`

    const formData = new FormData()
    formData.append('file', blob, filename)

    const response = await fetch(url, {
      method: 'POST',
      body: formData
      // 不设置 Content-Type，让浏览器自动设置 multipart/form-data 边界
    })

    if (!response.ok) {
      let errorMessage = `HTTP ${response.status}: ${response.statusText}`
      try {
        const errorData = await response.json()
        if (errorData.detail) {
          errorMessage = errorData.detail
        }
      } catch (e) {
        // 无法解析 JSON，使用默认错误信息
      }
      throw new Error(errorMessage)
    }

    return response.json()
  },

  // === 记录 ===

  getRecords: () => request('GET', '/records'),
  async searchRecords(params = {}) {
    const baseUrl = await getBaseUrl()
    const searchParams = new URLSearchParams()

    if (params.q) {
      searchParams.set('q', params.q)
    }
    if (params.search_type) {
      searchParams.set('search_type', params.search_type)
    }
    if (params.favorite_only) {
      searchParams.set('favorite_only', 'true')
    }
    if (params.page) {
      searchParams.set('page', String(params.page))
    }
    if (params.page_size) {
      searchParams.set('page_size', String(params.page_size))
    }

    const query = searchParams.toString()
    return request('GET', `/records/search${query ? `?${query}` : ''}`)
  },
  getRecord: (id) => request('GET', `/records/${id}`),
  getRecordStatus: (id) => request('GET', `/records/${id}/status`),
  updateRecord: (id, data) => request('PUT', `/records/${id}`, { body: data }),
  deleteRecord: (id) => request('DELETE', `/records/${id}`),
  optimizeRecord: (id) => request('POST', `/records/${id}/optimize`),

  async exportRecord(id, format = 'txt') {
    const baseUrl = await getBaseUrl()
    return `${baseUrl}/records/${id}/export?format=${format}`
  },

  async getRecordAudio(id) {
    const baseUrl = await getBaseUrl()
    return `${baseUrl}/records/${id}/audio`
  },

  // === 设置 ===

  getSettings: () => request('GET', '/settings'),
  saveSettings: (settings) => request('PUT', '/settings', { body: settings }),

  // === LLM 配置 ===

  getLLMConfigs: () => request('GET', '/llm/configs'),
  getLLMConfig: (id) => request('GET', `/llm/configs/${id}`),
  saveLLMConfig: (config) => {
    if (config.id) {
      return request('PUT', `/llm/configs/${config.id}`, { body: config })
    }
    return request('POST', '/llm/configs', { body: config })
  },
  deleteLLMConfig: (id) => request('DELETE', `/llm/configs/${id}`),
  testLLMConfig: (id) => request('POST', `/llm/configs/${id}/test`),
  setDefaultLLMConfig: (id) => request('PUT', `/llm/configs/${id}/default`),
  getLLMProviders: () => request('GET', '/llm/providers'),
  getLLMProviderModels: (provider) => request('GET', `/llm/providers/${provider}/models`),

  // === 预设 ===

  getPresets: () => request('GET', '/prompts'),
  getPreset: (id) => request('GET', `/prompts/${id}`),
  savePreset: (preset) => {
    if (preset.id) {
      return request('PUT', `/prompts/${preset.id}`, { body: preset })
    }
    return request('POST', '/prompts', { body: preset })
  },
  deletePreset: (id) => request('DELETE', `/prompts/${id}`),
  previewPreset: (id, text) => request('POST', `/prompts/${id}/preview`, { body: { text } }),
  setDefaultPreset: (id) => request('PUT', '/prompts/default', { body: { preset_id: id } }),

  // === WebSocket ===

  /**
   * 获取 WebSocket URL
   */
  async getWebSocketUrl(clientType = 'web') {
    const port = await getBackendPort()
    return `ws://127.0.0.1:${port}/api/ws/${clientType}`
  }
}

export default api
