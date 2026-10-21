/**
 * 「转文字」页的纯逻辑模型（不依赖 Vue / 浏览器全局，可在 Node 里直接测试）。
 *
 * 覆盖三块：
 * 1. 请求构造与响应归一 —— 与后端 POST /api/transcribe 契约一一对应；
 * 2. 202 异步任务的轮询策略 —— 1s 起步、逐次退避到 5s、总等待 15 分钟封顶；
 * 3. 展示格式化 —— 时长、文件名、扩展名预检。
 */

// 与后端 file_transcription_service 的可识别扩展名保持一致（仅用于前端预检提示，后端仍是权威）
export const SUPPORTED_MEDIA_EXTENSIONS = [
  '.mp4', '.flv', '.webm', '.mkv', '.mov', '.avi', '.m4v', '.ts',
  '.wav', '.mp3', '.m4a', '.flac', '.ogg', '.opus', '.aac', '.wma', '.amr'
]

// 同步请求的读取超时：契约约定调用方对短文件给 15 分钟；更长的文件后端直接 202
export const SYNC_REQUEST_TIMEOUT_MS = 15 * 60 * 1000

// 轮询策略默认值
export const JOB_POLL_INITIAL_DELAY_MS = 1000
export const JOB_POLL_MAX_DELAY_MS = 5000
export const JOB_POLL_MAX_WAIT_MS = 15 * 60 * 1000

/**
 * 构造 POST /api/transcribe 请求体。
 * 路径为空时返回 null（调用方据此提示，不发请求）。
 */
export function buildTranscribeRequest({ path, language = 'zh', wantSegments = false }) {
  const trimmed = String(path ?? '').trim()
  if (!trimmed) {
    return null
  }
  return {
    path: trimmed,
    language: String(language || 'zh'),
    want_segments: Boolean(wantSegments)
  }
}

/**
 * 响应分流：POST 可能返回同步结果（含 text）或 202 任务受理（含 job_id）。
 */
export function isAsyncJobAccepted(payload) {
  return payload != null && typeof payload === 'object'
    && typeof payload.job_id === 'string' && payload.job_id.length > 0
}

/**
 * 任务轮询策略：返回下一次轮询延迟（毫秒）。
 * 1s 起步逐次 +500ms 退避，到 5s 封顶；非法入参按第 1 次处理。
 */
export function nextJobPollDelay(attempt, { initialDelayMs = JOB_POLL_INITIAL_DELAY_MS, maxDelayMs = JOB_POLL_MAX_DELAY_MS } = {}) {
  const safeAttempt = Number.isFinite(attempt) && attempt > 0 ? Math.floor(attempt) : 1
  const delay = initialDelayMs + (safeAttempt - 1) * 500
  return Math.min(Math.max(delay, 0), maxDelayMs)
}

/**
 * 任务轮询是否已超过总等待上限（默认 15 分钟）。
 */
export function isJobPollExpired(elapsedMs, { maxWaitMs = JOB_POLL_MAX_WAIT_MS } = {}) {
  const safeElapsed = Number.isFinite(elapsedMs) && elapsedMs > 0 ? elapsedMs : 0
  return safeElapsed >= maxWaitMs
}

/**
 * 从路径取文件名（兼容反斜杠与正斜杠）。
 */
export function fileNameOf(path) {
  const normalized = String(path ?? '').replace(/\\/g, '/')
  const parts = normalized.split('/').filter(Boolean)
  return parts.length ? parts[parts.length - 1] : ''
}

/**
 * 把同步结果 / 任务结果归一成页面视图模型；未知形状返回 null。
 */
export function normalizeTranscribeResult(payload, { path = '' } = {}) {
  if (payload == null || typeof payload !== 'object') {
    return null
  }

  const text = typeof payload.text === 'string' ? payload.text : ''
  const durationSeconds = Number.isFinite(Number(payload.duration_seconds))
    ? Number(payload.duration_seconds)
    : 0

  return {
    path: String(path || ''),
    fileName: fileNameOf(path),
    text,
    durationSeconds,
    engine: String(payload.engine ?? ''),
    cached: Boolean(payload.cached),
    segments: Array.isArray(payload.segments) ? payload.segments : []
  }
}

/**
 * 时长格式化：61 → "1 分 01 秒"；0 / 非法 → "0 秒"。
 */
export function formatDuration(seconds) {
  const safe = Number.isFinite(Number(seconds)) && Number(seconds) > 0 ? Math.floor(Number(seconds)) : 0
  const minutes = Math.floor(safe / 60)
  const rest = safe % 60
  if (minutes <= 0) {
    return rest + ' 秒'
  }
  return minutes + ' 分 ' + String(rest).padStart(2, '0') + ' 秒'
}

/**
 * 扩展名预检：仅用于选择文件后立刻给出友好提示；后端校验仍是权威。
 */
export function isSupportedMediaPath(path) {
  const normalized = String(path ?? '').trim().toLowerCase()
  if (!normalized) {
    return false
  }
  return SUPPORTED_MEDIA_EXTENSIONS.some((ext) => normalized.endsWith(ext))
}
