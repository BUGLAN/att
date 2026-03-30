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
    let errorMessage = `HTTP ${response.status}: ${response.statusText}`
    try {
      const errorData = await response.json()
      if (errorData.detail) {
        errorMessage = errorData.detail
      } else if (errorData.message) {
        errorMessage = errorData.message
      }
    } catch (error) {
      // Ignore parse failures and use the default message.
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

  const response = await fetch(url, {
    method: 'POST',
    body: formData
  })

  if (!response.ok) {
    let errorMessage = `HTTP ${response.status}: ${response.statusText}`
    try {
      const errorData = await response.json()
      if (errorData.detail) {
        errorMessage = errorData.detail
      }
    } catch (error) {
      // Ignore parse failures and use the default message.
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
  mergeSession: (sessionId) => request('POST', `/sessions/${sessionId}/merge`),
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
  optimizeRecord: (id) => request('POST', `/records/${id}/optimize`),

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
    if (params.limit) {
      searchParams.set('limit', String(params.limit))
    }

    const query = searchParams.toString()
    return request('GET', `/clipboard/items${query ? `?${query}` : ''}`)
  },
  getClipboardStats: () => request('GET', '/clipboard/stats'),
  createClipboardItem: (data) => request('POST', '/clipboard/items', { body: data }),
  getClipboardItem: (id) => request('GET', `/clipboard/items/${id}`),
  updateClipboardItem: (id, data) => request('PUT', `/clipboard/items/${id}`, { body: data }),
  deleteClipboardItem: (id) => request('DELETE', `/clipboard/items/${id}`),
  async getWordbookEntries(params = {}) {
    const searchParams = new URLSearchParams()

    if (params.query) {
      searchParams.set('query', params.query)
    }
    if (params.entry_type) {
      searchParams.set('entry_type', params.entry_type)
    }
    if (params.favorite_only) {
      searchParams.set('favorite_only', 'true')
    }
    if (params.limit) {
      searchParams.set('limit', String(params.limit))
    }

    const query = searchParams.toString()
    return request('GET', `/wordbook${query ? `?${query}` : ''}`)
  },
  getWordbookStats: () => request('GET', '/wordbook/stats'),
  createWordbookEntry: (data) => request('POST', '/wordbook', { body: data }),
  getWordbookEntry: (id) => request('GET', `/wordbook/${id}`),
  updateWordbookEntry: (id, data) => request('PUT', `/wordbook/${id}`, { body: data }),
  runAITranslate: (data) => request('POST', '/ai-actions/translate', { body: data }),
  runAIExplain: (data) => request('POST', '/ai-actions/explain', { body: data }),
  runAISummarize: (data) => request('POST', '/ai-actions/summarize', { body: data }),
  runAIRewrite: (data) => request('POST', '/ai-actions/rewrite', { body: data }),

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
  testLLMConfig: (id) => request('POST', `/llm/configs/${id}/test`),
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
