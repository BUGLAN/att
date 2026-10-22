/**
 * HTTP API client
 * 直接访问 Python FastAPI 后端，不经过 IPC 中转。
 */
import { createNdjsonParser } from '@/utils/ndjson.mjs'

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
// 文件转写同步请求的读取超时：契约约定短文件给 15 分钟；更长的文件后端直接 202
const TRANSCRIBE_SYNC_TIMEOUT_MS = 15 * 60 * 1000

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
    let errorData = null
    let errorMessage

    try {
      errorData = await response.json()
      errorMessage = extractErrorMessage(errorData, response)
    } catch (error) {
      // 解析失败时回退到状态码文案
      errorMessage = `HTTP ${response.status}: ${response.statusText}`
    }

    const error = new Error(errorMessage)
    // 保留原始状态码与 detail（409 冲突 / structured detail 的调用方需要自行分支）
    error.status = response.status
    error.detail = errorData?.detail
    throw error
  }

  return response.json()
}

/**
 * 读取 NDJSON 流式响应：每行一个 JSON 事件，边读边回调。
 * 返回 { eventCount }，供调用方判断是否已经收到过增量（决定能否安全降级）。
 */
async function streamNdjson(endpoint, options = {}) {
  const baseUrl = await getBaseUrl()
  const url = `${baseUrl}${endpoint}`
  const timeout = options.timeout ?? SLOW_TIMEOUT_MS
  const controller = new AbortController()
  let timedOut = false
  const timer = setTimeout(() => {
    timedOut = true
    controller.abort()
  }, timeout)

  const externalSignal = options.signal
  const onExternalAbort = () => controller.abort()
  if (externalSignal) {
    if (externalSignal.aborted) {
      controller.abort()
    } else {
      externalSignal.addEventListener('abort', onExternalAbort, { once: true })
    }
  }

  let eventCount = 0
  const onEvent = options.onEvent
  const parser = createNdjsonParser((event) => {
    eventCount += 1
    if (typeof onEvent === 'function') {
      onEvent(event)
    }
  })

  let response
  try {
    response = await fetch(url, {
      method: options.method || 'POST',
      headers: {
        'Content-Type': 'application/json',
        ...options.headers
      },
      body: options.body ? JSON.stringify(options.body) : undefined,
      signal: controller.signal
    })

    if (!response.ok) {
      let errorData = null
      let errorMessage = `HTTP ${response.status}: ${response.statusText}`
      try {
        errorData = await response.json()
        errorMessage = extractErrorMessage(errorData, response)
      } catch (error) {
        // 解析失败时保留状态码文案
      }
      const httpError = new Error(errorMessage)
      httpError.status = response.status
      httpError.detail = errorData?.detail
      throw httpError
    }

    if (!response.body) {
      throw new Error('当前环境不支持流式读取（response.body 为空）')
    }

    const reader = response.body.getReader()
    const decoder = new TextDecoder()
    try {
      while (true) {
        const { done, value } = await reader.read()
        if (done) {
          break
        }
        parser.push(decoder.decode(value, { stream: true }))
      }
      parser.push(decoder.decode())
      parser.flush()
    } finally {
      reader.releaseLock()
    }
  } catch (error) {
    if (error?.name === 'AbortError') {
      if (timedOut) {
        throw new Error(`流式请求超时（超过 ${Math.round(timeout / 1000)} 秒未完成）`)
      }
      const cancelled = new Error('已取消')
      cancelled.name = 'AbortError'
      cancelled.cancelled = true
      throw cancelled
    }

    if (error?.status) {
      throw error
    }

    if (error?.name === 'SyntaxError') {
      throw new Error(`流式响应解析失败：${error.message}`)
    }

    throw new Error(`流式连接中断：${error?.message || error}`)
  } finally {
    clearTimeout(timer)
    if (externalSignal) {
      externalSignal.removeEventListener('abort', onExternalAbort)
    }
  }

  return { eventCount }
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
  optimizeRecordStream: (id, options = {}) => streamNdjson(`/records/${id}/optimize/stream`, {
    ...options,
    method: 'POST',
    timeout: options.timeout ?? SLOW_TIMEOUT_MS
  }),
  cancelOptimizeRecord: (id) => request('POST', `/records/${id}/optimize/cancel`, { timeout: 10000 }),

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

  // 本地文件转写：短文件同步 200；长文件 202 {job_id}，再用 getTranscribeJob 轮询
  transcribeFile: (payload) => request('POST', '/transcribe', {
    body: payload,
    timeout: TRANSCRIBE_SYNC_TIMEOUT_MS
  }),
  getTranscribeJob: (jobId) => request('GET', `/transcribe/jobs/${encodeURIComponent(jobId)}`),

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

  // ===== LLM v2（供应商档案 + 模型条目，见 docs/LLM配置模块重设计.md §6）=====
  getLLMState: () => request('GET', '/llm/state'),
  getLLMProviders: () => request('GET', '/llm/providers'),
  createLLMProfile: (payload) => request('POST', '/llm/profiles', { body: payload }),
  updateLLMProfile: (id, payload) => request('PUT', `/llm/profiles/${id}`, { body: payload }),
  deleteLLMProfile: (id) => request('DELETE', `/llm/profiles/${id}`),
  verifyLLMProfile: (id) => request('POST', `/llm/profiles/${id}/verify`, { timeout: SLOW_TIMEOUT_MS }),
  getLLMRemoteModels: (id) => request('POST', `/llm/profiles/${id}/remote-models`, { timeout: SLOW_TIMEOUT_MS }),
  // 保存前验证：草稿密钥只在后端内存中用一次，不落盘
  verifyLLMDraft: (payload) => request('POST', '/llm/verify-draft', { body: payload, timeout: SLOW_TIMEOUT_MS }),
  createLLMModels: (items) => request('POST', '/llm/models', { body: { items } }),
  updateLLMModel: (id, payload) => request('PUT', `/llm/models/${id}`, { body: payload }),
  deleteLLMModel: (id) => request('DELETE', `/llm/models/${id}`),
  verifyLLMModel: (id) => request('POST', `/llm/models/${id}/verify`, { timeout: SLOW_TIMEOUT_MS }),
  activateLLMModel: (id) => request('POST', `/llm/models/${id}/activate`),
  updateLLMSettings: (payload) => request('PUT', '/llm/settings', { body: payload }),

  // 预设是只读的（唯一真相源是 prompts/*.txt）：只保留列出与设默认两个接口
  getPresets: () => request('GET', '/prompts'),
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
