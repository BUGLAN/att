/**
 * HTTP API client
 * 直接访问 Python FastAPI 后端，不经过 IPC 中转。
 */

let backendPort = null

export async function getBackendPort() {
  if (backendPort) {
    return backendPort
  }

  if (typeof window !== 'undefined' && window.electronAPI?.getPort) {
    try {
      backendPort = await window.electronAPI.getPort()
      return backendPort
    } catch (error) {
      console.warn('[API] Failed to get port from Electron, using default:', error)
    }
  }

  backendPort = 8000
  return backendPort
}

export async function getBaseUrl() {
  const port = await getBackendPort()
  return `http://127.0.0.1:${port}/api`
}

// 默认请求超时：普通 CRUD 30s；慢接口（LLM 优化 / 连接测试）单独放宽
const DEFAULT_TIMEOUT_MS = 30000
const SLOW_TIMEOUT_MS = 180000
const UPLOAD_TIMEOUT_MS = 300000

/**
 * 从错误响应体里提取可读的中文错误信息。
 * FastAPI 的 422 校验错误里 detail 是对象数组，直接 new Error(detail) 会得到 "[object Object]"。
 */
function extractErrorMessage(errorData, response) {
  const detail = errorData?.detail

  if (typeof detail === 'string' && detail) {
    return detail
  }

  if (Array.isArray(detail)) {
    const parts = detail
      .map((item) => {
        if (typeof item === 'string') {
          return item
        }

        const location = Array.isArray(item?.loc)
          ? item.loc.filter((segment) => segment !== 'body').join('.')
          : ''
        const message = item?.msg || ''

        if (location && message) {
          return `${location}: ${message}`
        }
        return message
      })
      .filter(Boolean)

    if (parts.length) {
      return parts.join('；')
    }
  }

  if (detail && typeof detail === 'object') {
    return detail.message || JSON.stringify(detail)
  }

  if (typeof errorData?.message === 'string' && errorData.message) {
    return errorData.message
  }

  return `HTTP ${response.status}: ${response.statusText}`
}

async function request(method, endpoint, options = {}) {
  const baseUrl = await getBaseUrl()
  const url = `${baseUrl}${endpoint}`
  const timeout = options.timeout ?? DEFAULT_TIMEOUT_MS
  const controller = new AbortController()
  const timer = setTimeout(() => controller.abort(), timeout)

  const fetchOptions = {
    method,
    headers: {
      'Content-Type': 'application/json',
      ...options.headers
    },
    signal: controller.signal
  }

  if (options.body && method !== 'GET') {
    fetchOptions.body = JSON.stringify(options.body)
  }

  let response
  try {
    response = await fetch(url, fetchOptions)
  } catch (error) {
    if (error?.name === 'AbortError') {
      throw new Error(`请求超时（超过 ${Math.round(timeout / 1000)} 秒未响应）`)
    }
    throw new Error(`无法连接后端服务：${error?.message || error}`)
  } finally {
    clearTimeout(timer)
  }

  if (!response.ok) {
    let errorMessage

    try {
      errorMessage = extractErrorMessage(await response.json(), response)
    } catch (error) {
      // 解析失败时回退到状态码文案
      errorMessage = `HTTP ${response.status}: ${response.statusText}`
    }

    throw new Error(errorMessage)
  }

  return response.json()
}

async function uploadForm(endpoint, fileField, blob, filename) {
  const baseUrl = await getBaseUrl()
  const url = `${baseUrl}${endpoint}`
  const formData = new FormData()
  formData.append(fileField, blob, filename)

  const controller = new AbortController()
  const timer = setTimeout(() => controller.abort(), UPLOAD_TIMEOUT_MS)

  let response
  try {
    response = await fetch(url, {
      method: 'POST',
      body: formData,
      signal: controller.signal
    })
  } catch (error) {
    if (error?.name === 'AbortError') {
      throw new Error(`上传超时（超过 ${Math.round(UPLOAD_TIMEOUT_MS / 1000)} 秒未完成）`)
    }
    throw new Error(`上传失败：${error?.message || error}`)
  } finally {
    clearTimeout(timer)
  }

  if (!response.ok) {
    let errorMessage = `HTTP ${response.status}: ${response.statusText}`
    try {
      errorMessage = extractErrorMessage(await response.json(), response)
    } catch (error) {
      // 解析失败时保留状态码文案
    }
    throw new Error(errorMessage)
  }

  return response.json()
}

export const api = {
  async uploadAudio(blob, filename = 'recording.webm') {
    return uploadForm('/audio/upload', 'file', blob, filename)
  },

  createSession: () => request('POST', '/sessions'),
  async uploadSessionSegment(sessionId, blob, filename = 'segment.webm') {
    return uploadForm(`/sessions/${sessionId}/segments`, 'file', blob, filename)
  },
  mergeSession: (sessionId) => request('POST', `/sessions/${sessionId}/merge`, { timeout: SLOW_TIMEOUT_MS }),
  deleteSession: (sessionId) => request('DELETE', `/sessions/${sessionId}`),

  getRecords: () => request('GET', '/records'),
  async searchRecords(params = {}) {
    const searchParams = new URLSearchParams()

    if (params.q) searchParams.set('q', params.q)
    if (params.search_type) searchParams.set('search_type', params.search_type)
    if (params.favorite_only) searchParams.set('favorite_only', 'true')
    if (params.page) searchParams.set('page', String(params.page))
    if (params.page_size) searchParams.set('page_size', String(params.page_size))

    const query = searchParams.toString()
    return request('GET', `/records/search${query ? `?${query}` : ''}`)
  },
  getRecord: (id) => request('GET', `/records/${id}`),
  getRecordStatus: (id) => request('GET', `/records/${id}/status`),
  updateRecord: (id, data) => request('PUT', `/records/${id}`, { body: data }),
  deleteRecord: (id) => request('DELETE', `/records/${id}`),
  optimizeRecord: (id) => request('POST', `/records/${id}/optimize`, { timeout: SLOW_TIMEOUT_MS }),

  async getClipboardItems(params = {}) {
    const searchParams = new URLSearchParams()

    if (params.query) {
      searchParams.set('query', params.query)
    }
    if (params.item_type) {
      searchParams.set('item_type', params.item_type)
    }
    if (params.favorite_only) {
      searchParams.set('favorite_only', 'true')
    }
    if (params.pinned_only) {
      searchParams.set('pinned_only', 'true')
    }
    if (params.limit) {
      searchParams.set('limit', String(params.limit))
    }

    const query = searchParams.toString()
    return request('GET', `/clipboard/items${query ? `?${query}` : ''}`)
  },
  getClipboardStats: () => request('GET', '/clipboard/stats'),
  createClipboardItem: (data) => request('POST', '/clipboard/items', { body: data }),
  getClipboardItem: (id) => request('GET', `/clipboard/items/${id}`),
  async getClipboardItemImageUrl(id, version = '') {
    const baseUrl = await getBaseUrl()
    const suffix = version ? `?v=${encodeURIComponent(version)}` : ''
    return `${baseUrl}/clipboard/items/${id}/image${suffix}`
  },
  updateClipboardItem: (id, data) => request('PUT', `/clipboard/items/${id}`, { body: data }),
  deleteClipboardItem: (id) => request('DELETE', `/clipboard/items/${id}`),

  async exportRecord(id, format = 'txt') {
    const baseUrl = await getBaseUrl()
    return `${baseUrl}/records/${id}/export?format=${format}`
  },

  async getRecordAudio(id) {
    const baseUrl = await getBaseUrl()
    return `${baseUrl}/records/${id}/audio`
  },

  getSettings: () => request('GET', '/settings'),
  saveSettings: (settings) => request('PUT', '/settings', { body: settings }),
  getHealth: (probeRuntime = false) => request('GET', `/health${probeRuntime ? '?probe_runtime=true' : ''}`),
  getStreamingServiceStatus: () => request('GET', '/model-service'),
  startStreamingService: (waitUntilReady = true) => request('POST', '/model-service/start', {
    timeout: SLOW_TIMEOUT_MS,
    body: {
      wait_until_ready: waitUntilReady
    }
  }),
  stopStreamingService: () => request('POST', '/model-service/stop'),

  getLLMConfigs: () => request('GET', '/llm/configs'),
  getLLMConfig: (id) => request('GET', `/llm/configs/${id}`),
  saveLLMConfig: (config) => (
    config.id
      ? request('PUT', `/llm/configs/${config.id}`, { body: config })
      : request('POST', '/llm/configs', { body: config })
  ),
  deleteLLMConfig: (id) => request('DELETE', `/llm/configs/${id}`),
  testLLMConfig: (id) => request('POST', `/llm/configs/${id}/test`, { timeout: SLOW_TIMEOUT_MS }),
  setDefaultLLMConfig: (id) => request('PUT', `/llm/configs/${id}/default`),
  getLLMProviders: () => request('GET', '/llm/providers'),
  getLLMProviderModels: (provider) => request('GET', `/llm/providers/${provider}/models`),

  getPresets: () => request('GET', '/prompts'),
  getPreset: (id) => request('GET', `/prompts/${id}`),
  savePreset: (preset) => (
    preset.id
      ? request('PUT', `/prompts/${preset.id}`, { body: preset })
      : request('POST', '/prompts', { body: preset })
  ),
  deletePreset: (id) => request('DELETE', `/prompts/${id}`),
  previewPreset: (id, text) => request('POST', `/prompts/${id}/preview`, { body: { text } }),
  setDefaultPreset: (id) => request('PUT', '/prompts/default', { body: { preset_id: id } }),

  async getWebSocketUrl() {
    const port = await getBackendPort()
    return `ws://127.0.0.1:${port}/api/ws`
  },

  async getStreamingWebSocketUrl() {
    const port = await getBackendPort()
    return `ws://127.0.0.1:${port}/api/asr/stream`
  }
}

export default api
