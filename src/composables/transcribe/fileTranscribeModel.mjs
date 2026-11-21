/**
 * 「转文字」页的纯逻辑模型（不依赖 Vue / 浏览器全局，可在 Node 里直接测试）。
 *
 * 覆盖四块：
 * 1. 请求构造与响应归一 —— 与后端 POST /api/transcribe 契约一一对应；
 * 2. 202 异步任务的轮询策略 —— 1s 起步、逐次退避到 5s；进度停滞 15 分钟才放弃等待；
 * 3. 识别 / 校对进度的展示与多文件排队；
 * 4. 展示格式化 —— 时长、文件名、扩展名预检。
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
 * 识别引擎下拉的元数据（与后端 engine 参数一一对应）。
 * 文件转写只支持这几个引擎（后端 att-core transcribe::resolve_engine），ChatGPT 听写不在其中。
 */
export const TRANSCRIBE_ENGINES = [
  {
    value: 'doubao_asr',
    label: '豆包 · 实时大模型',
    tag: '在线',
    desc: '火山在线识别，质量高、不占本机算力，按音频时长计费。'
  },
  {
    value: 'deepseek_asr',
    label: 'DeepSeek 语音识别',
    tag: '在线',
    desc: 'DeepSeek 在线批量识别，自动判定语种，按音频时长计费。'
  },
  {
    value: 'whisper',
    label: 'faster-whisper',
    tag: '本地',
    desc: '本机离线识别，需要先安装模型。'
  },
  {
    value: 'qwen_asr',
    label: 'Qwen3-ASR',
    tag: '本地',
    desc: '本机离线识别，多语种更好，需要先安装模型和推理库。'
  }
]

/** 设置里的引擎不能用于文件转写（如 ChatGPT 听写）时退到的引擎。 */
export const FALLBACK_TRANSCRIBE_ENGINE = 'doubao_asr'

function isTranscribeEngine(value) {
  return TRANSCRIBE_ENGINES.some((item) => item.value === value)
}

/**
 * 转文字页的默认引擎：跟随设置里的 `asr_provider`；文件转写不支持时退到豆包。
 */
export function defaultTranscribeEngine(asrProvider) {
  return isTranscribeEngine(asrProvider) ? asrProvider : FALLBACK_TRANSCRIBE_ENGINE
}

/**
 * 引擎下拉选项 + 能低成本拿到的可用性：
 * - 豆包：设置里没保存 Access Token（`doubao_asr_api_key_set`）即不可用；
 * - 本地引擎：/api/health 的 `local_engine` 只描述设置里选中的那个本地引擎，
 *   它报缺模型 / 推理库时标出原因；其余本地引擎拿不到状态，不标（提交时后端 409 兜底）。
 * 返回 [{ value, label, tag, desc, disabled, badge, hint }]：badge 是下拉里的短标注，hint 是一句原因。
 */
export function transcribeEngineOptions({ doubaoKeySet = false, deepseekKeySet = false, localEngine = null } = {}) {
  return TRANSCRIBE_ENGINES.map((item) => {
    if (item.value === 'doubao_asr' && !doubaoKeySet) {
      return { ...item, disabled: true, badge: '未配置', hint: '豆包需要先在「设置 → 识别引擎」填写 Access Token。' }
    }
    if (item.value === 'deepseek_asr' && !deepseekKeySet) {
      return { ...item, disabled: true, badge: '未配置', hint: 'DeepSeek 需要先在「设置 → 识别引擎」填写 API Key。' }
    }
    if (localEngine && localEngine.provider === item.value && localEngine.available === false) {
      const summary = String(localEngine.error || '').split('\n')[0].trim() || `${item.label} 模型未安装`
      return { ...item, disabled: true, badge: '未安装', hint: `${summary}，可在「设置 → 识别引擎」下载模型。` }
    }
    return { ...item, disabled: false, badge: '', hint: '' }
  })
}

/**
 * 构造 POST /api/transcribe 请求体（M2：engine + LLM 校对参数）。
 * 路径为空时返回 null（调用方据此提示，不发请求）；引擎不在 {@link TRANSCRIBE_ENGINES} 里时退到豆包。
 */
export function buildTranscribeRequest({
  path,
  language = 'zh',
  wantSegments = false,
  engine = FALLBACK_TRANSCRIBE_ENGINE,
  proofread = false,
  proofreadModelId = null,
  proofreadBatchSegments = 40,
  proofreadContextChars = 200,
  asyncJob = false
}) {
  const trimmed = String(path ?? '').trim()
  if (!trimmed) {
    return null
  }

  const request = {
    path: trimmed,
    language: String(language || 'zh'),
    want_segments: Boolean(wantSegments),
    engine: defaultTranscribeEngine(engine),
    proofread: Boolean(proofread)
  }
  // 一律走后台任务：拿得到识别进度、可以取消（缓存命中仍直接返回结果）
  if (asyncJob) {
    request.async_job = true
  }

  if (request.proofread) {
    if (proofreadModelId) {
      request.proofread_model_id = String(proofreadModelId)
    }
    request.proofread_options = {
      batch_segments: Number.isFinite(Number(proofreadBatchSegments))
        ? Number(proofreadBatchSegments)
        : 40,
      context_chars: Number.isFinite(Number(proofreadContextChars))
        ? Number(proofreadContextChars)
        : 200
    }
  }
  return request
}

/**
 * 把文本按句切分（保留分隔符），用于校对对照视图。
 */
export function splitSentences(text) {
  const raw = String(text ?? '')
  if (!raw.trim()) {
    return []
  }
  const parts = raw.match(/[^。！？!?.\n]*[。！？!?.\n]?/g) || []
  return parts.map((part) => part.trim()).filter(Boolean)
}

/**
 * 构造「原文 × 校对后」的逐句对照（前端 diff，设计 §4.7）。
 *
 * 句数一致时逐句配对；不一致（校对并句等）退化为整段一对，保证视图永不错位。
 * 返回 { pairs: [{raw, corrected, changed}], changedCount }。
 */
export function buildProofreadDiff(rawText, correctedText) {
  const raw = String(rawText ?? '')
  const corrected = String(correctedText ?? '')
  const rawParts = splitSentences(raw)
  const correctedParts = splitSentences(corrected)

  let pairs
  if (rawParts.length === correctedParts.length && rawParts.length > 0) {
    pairs = rawParts.map((part, index) => ({
      raw: part,
      corrected: correctedParts[index],
      changed: part !== correctedParts[index]
    }))
  } else {
    pairs = [{ raw, corrected, changed: raw !== corrected }]
  }

  return { pairs, changedCount: pairs.filter((pair) => pair.changed).length }
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

/** 任务已经结束（成功 / 失败 / 已取消）。 */
export function isJobFinished(job) {
  return ['done', 'failed', 'cancelled'].includes(job?.status)
}

/**
 * 任务进度的签名：阶段或进度一变签名就变（用来判断「还在推进」，停滞才算超时）。
 */
export function jobProgressSignature(job) {
  const progress = job?.progress || {}
  return [
    job?.stage || '',
    progress.done_seconds ?? '',
    progress.window_done ?? ''
  ].join('|')
}

/** 秒 → "m:ss" 或 "h:mm:ss"（进度文案用）。 */
export function formatClock(seconds) {
  const safe = Number.isFinite(Number(seconds)) && Number(seconds) > 0 ? Math.floor(Number(seconds)) : 0
  const hours = Math.floor(safe / 3600)
  const minutes = Math.floor((safe % 3600) / 60)
  const rest = String(safe % 60).padStart(2, '0')
  return hours > 0 ? `${hours}:${String(minutes).padStart(2, '0')}:${rest}` : `${minutes}:${rest}`
}

/**
 * 后台任务进度 → 展示：{ percent: 0~100 | null, label }。
 * - 识别阶段 `{stage:"asr", done_seconds, total_seconds}` → 「识别中 · 3:20 / 12:00」；
 * - 校对阶段 `{window_done, window_total}` → 「LLM 校对中 · 窗口 2/5」；
 * - 拿不到进度时 percent 为 null（界面退回不定进度的流动线）。
 * `cancelRequested` 为真时文案改成「正在取消…」，进度保留。
 */
export function describeJobProgress(job, { cancelRequested = false } = {}) {
  const stage = String(job?.stage || '')
  const progress = job?.progress || null
  let percent = null
  let label = ''

  if (stage === 'proofreading') {
    const total = Number(progress?.window_total)
    const done = Number(progress?.window_done)
    if (Number.isFinite(total) && total > 0 && Number.isFinite(done)) {
      percent = Math.min(100, Math.max(0, Math.floor((done / total) * 100)))
      label = `LLM 校对中 · 窗口 ${done}/${total}`
    } else {
      label = 'LLM 校对中…'
    }
  } else if (stage === 'asr' || stage === '') {
    const total = Number(progress?.total_seconds)
    const done = Number(progress?.done_seconds)
    if (Number.isFinite(total) && total > 0 && Number.isFinite(done)) {
      percent = Math.min(100, Math.max(0, Math.floor((done / total) * 100)))
      label = percent >= 100
        ? '识别中 · 等待识别结果…'
        : `识别中 · ${formatClock(done)} / ${formatClock(total)}`
    } else {
      label = '识别中…'
    }
  }

  if (cancelRequested || job?.cancel_requested) {
    label = '正在取消…'
  }
  return { percent, label }
}

// ---------- 多文件排队 ----------

/** 队列项状态：排队中 / 转写中 / 完成 / 失败 / 已取消。 */
export const QUEUE_STATUS_LABELS = {
  pending: '排队中',
  running: '转写中',
  done: '完成',
  failed: '失败',
  cancelled: '已取消'
}

let queueSeq = 0

/**
 * 把一批路径加进队列（按给出的顺序排在末尾）：不支持的类型、已在队列里还没转完的同一路径都跳过。
 * 返回 { queue, added, unsupported, duplicate }（不修改入参）。
 */
export function enqueuePaths(queue, paths, { makeId = () => `q${++queueSeq}` } = {}) {
  const next = Array.isArray(queue) ? [...queue] : []
  const added = []
  const unsupported = []
  const duplicate = []
  for (const raw of Array.isArray(paths) ? paths : [paths]) {
    const path = String(raw ?? '').trim()
    if (!path) {
      continue
    }
    if (!isSupportedMediaPath(path)) {
      unsupported.push(path)
      continue
    }
    if (next.some((item) => item.path === path && (item.status === 'pending' || item.status === 'running'))) {
      duplicate.push(path)
      continue
    }
    const item = { id: makeId(), path, fileName: fileNameOf(path), status: 'pending', error: '' }
    next.push(item)
    added.push(item)
  }
  return { queue: next, added, unsupported, duplicate }
}

/** 下一个要转写的项（最靠前的排队中项）；没有时返回 null。 */
export function nextQueuedItem(queue) {
  return (queue || []).find((item) => item.status === 'pending') || null
}

/** 改一项的字段（返回新数组）。 */
export function updateQueueItem(queue, id, patch) {
  return (queue || []).map((item) => (item.id === id ? { ...item, ...patch } : item))
}

/** 移除一项：只有还没开始的能移除（转写中的要先取消）。 */
export function removeQueueItem(queue, id) {
  return (queue || []).filter((item) => item.id !== id || item.status !== 'pending')
}

/** 清掉已经结束的项（完成 / 失败 / 已取消），保留排队中与转写中的。 */
export function clearFinishedQueueItems(queue) {
  return (queue || []).filter((item) => item.status === 'pending' || item.status === 'running')
}

/** 队列计数：{ total, pending, running, done, failed, cancelled }。 */
export function queueSummary(queue) {
  const summary = { total: 0, pending: 0, running: 0, done: 0, failed: 0, cancelled: 0 }
  for (const item of queue || []) {
    summary.total += 1
    if (item.status in summary) {
      summary[item.status] += 1
    }
  }
  return summary
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
    recordId: typeof payload.record_id === 'string' ? payload.record_id : '',
    text,
    durationSeconds,
    engine: String(payload.engine ?? ''),
    engineKey: String(payload.engine ?? '').split(':')[0],
    cached: Boolean(payload.cached),
    segments: Array.isArray(payload.segments) ? payload.segments : [],
    rawText: typeof payload.raw_text === 'string' ? payload.raw_text : '',
    proofread: payload.proofread && typeof payload.proofread === 'object'
      ? {
          applied: Boolean(payload.proofread.applied),
          llmModel: String(payload.proofread.llm_model ?? ''),
          windows: Number(payload.proofread.windows ?? 0),
          fallbackBatches: Array.isArray(payload.proofread.fallback_batches)
            ? payload.proofread.fallback_batches
            : []
        }
      : null
  }
}

/** 转文字页「最近转写」条数（从记录集读带 source_file 的记录）。 */
export const TRANSCRIBE_HISTORY_LIMIT = 20

/**
 * 记录集里的文件转写记录 → 转文字页结果视图（与 {@link normalizeTranscribeResult} 同形）。
 * 记录不存分段时间戳，`segments` 为空（回看历史不能直接导出字幕）；没有 `source_file` 的返回 null。
 */
export function recordToTranscribeResult(record) {
  if (!record || typeof record !== 'object' || !record.source_file) {
    return null
  }
  const raw = typeof record.raw_text === 'string' ? record.raw_text : ''
  const optimized = typeof record.optimized_text === 'string' && record.optimized_text.trim() ? record.optimized_text : ''
  const engine = String(record.engine ?? '')
  return {
    path: '',
    fileName: String(record.source_file),
    recordId: String(record.id ?? ''),
    createdAt: String(record.created_at ?? ''),
    text: optimized || raw,
    durationSeconds: Number.isFinite(Number(record.duration)) ? Number(record.duration) : 0,
    engine,
    engineKey: engine.split(':')[0],
    cached: false,
    fromRecord: true,
    segments: [],
    rawText: optimized ? raw : '',
    proofread: optimized ? { applied: true, llmModel: '', windows: 0, fallbackBatches: [] } : null
  }
}

/** 记录集搜索结果 → 最近转写列表（只留文件转写的记录，最多 {@link TRANSCRIBE_HISTORY_LIMIT} 条）。 */
export function historyFromRecords(items, limit = TRANSCRIBE_HISTORY_LIMIT) {
  return (Array.isArray(items) ? items : [])
    .map(recordToTranscribeResult)
    .filter(Boolean)
    .slice(0, limit)
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
