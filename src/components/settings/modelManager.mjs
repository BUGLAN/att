// 本地模型管理（设置页）的纯逻辑：体积文案、状态标签、下载任务进度与 VAD 提示。
// 不依赖 Vue，便于 tests/test_settings_model_manager.mjs 锁定文案与边界。

const UNITS = ['B', 'KB', 'MB', 'GB', 'TB']

/** 字节数 → 人类可读（二进制单位，保留一位小数；0 显示 0 B）。 */
export function formatBytes(bytes) {
  const value = Number(bytes)
  if (!Number.isFinite(value) || value <= 0) {
    return '0 B'
  }
  let index = 0
  let size = value
  while (size >= 1024 && index < UNITS.length - 1) {
    size /= 1024
    index += 1
  }
  const digits = index === 0 ? 0 : 1
  return `${size.toFixed(digits)} ${UNITS[index]}`
}

/** 模型行下面的一句状态：已安装 / 部分安装 / 未安装，都带占用或下载体积。 */
export function modelStatusText(model) {
  if (!model) {
    return ''
  }
  if (model.status === 'installed') {
    return `已安装 · ${formatBytes(model.installed_bytes)}`
  }
  if (model.status === 'partial') {
    return `安装不完整 · ${formatBytes(model.installed_bytes)} / ${formatBytes(model.download_size)}`
  }
  return `未安装 · 需下载 ${formatBytes(model.download_size)}`
}

/** 下载按钮文案（含体积）；已安装时不显示按钮。 */
export function downloadLabel(model) {
  if (!model) {
    return '下载'
  }
  return model.status === 'partial'
    ? `继续下载（共 ${formatBytes(model.download_size)}）`
    : `下载 ${formatBytes(model.download_size)}`
}

/** 当前识别引擎对应的模型清单；VAD 只给 Whisper 展示（Qwen / SenseVoice 不走 Silero VAD）。 */
export function modelsForProvider(models, provider) {
  const items = Array.isArray(models) ? models : []
  if (provider === 'whisper') {
    return items.filter((model) => model.engine === 'whisper' || model.engine === 'vad')
  }
  if (provider === 'qwen_asr') {
    return items.filter((model) => model.engine === 'qwen_asr')
  }
  if (provider === 'sense_voice') {
    return items.filter((model) => model.engine === 'sense_voice')
  }
  return []
}

/** 按设置值找 Whisper 清单项（下拉框标「已安装」用）。 */
export function whisperModelMap(models) {
  const map = new Map()
  for (const model of Array.isArray(models) ? models : []) {
    if (model.engine === 'whisper' && model.select_value) {
      map.set(model.select_value, model)
    }
  }
  return map
}

/** 下拉项文案：`large-v3-turbo · 已安装` / `small · 未安装（需下载 461 MB）`。 */
export function whisperOptionLabel(value, models) {
  const model = whisperModelMap(models).get(value)
  if (!model) {
    return value
  }
  return model.status === 'installed' ? `${value} · 已安装` : `${value} · 未安装（${formatBytes(model.download_size)}）`
}

/** 任务是否还在进行（决定轮询与取消按钮）。 */
export function isJobActive(job) {
  return Boolean(job && ['downloading', 'verifying', 'extracting', 'installing', 'cancelling'].includes(job.status))
}

const STATUS_TEXT = {
  downloading: '正在下载',
  verifying: '正在校验',
  extracting: '正在解压',
  installing: '正在安装',
  cancelling: '正在取消',
  done: '已完成',
  cancelled: '已取消（重新下载会续传）',
  error: '下载失败'
}

/**
 * 下载任务 → 进度展示模型。
 * 返回 { active, canCancel, percent, label, speed, file, error }。
 */
export function jobView(job) {
  if (!job) {
    return { active: false, canCancel: false, percent: 0, label: '', speed: '', file: '', error: '' }
  }
  const active = isJobActive(job)
  const total = Number(job.total) || 0
  const received = Number(job.received) || 0
  const percent = total > 0 ? Math.max(0, Math.min(100, (received / total) * 100)) : 0
  const base = STATUS_TEXT[job.status] || job.status || ''
  let label = base
  if (job.status === 'downloading') {
    label = `${base} · ${formatBytes(received)} / ${formatBytes(total)}`
  } else if (job.status === 'error' && job.error) {
    label = `${base}：${job.error}`
  }
  const speed = job.status === 'downloading' && job.speed_bps > 0 ? `${formatBytes(job.speed_bps)}/s` : ''
  const file = job.current_file ? String(job.current_file).split('/').pop() : ''
  return {
    active,
    canCancel: ['downloading', 'verifying'].includes(job.status),
    percent,
    label,
    speed,
    file,
    error: job.error || ''
  }
}

/** 选中 Whisper 但 VAD 未安装时的提示（其它引擎返回空串）。 */
export function vadWarning(provider, models) {
  if (provider !== 'whisper') {
    return ''
  }
  const vad = (Array.isArray(models) ? models : []).find((model) => model.id === 'silero-vad')
  if (!vad || vad.status === 'installed') {
    return ''
  }
  return '未安装 VAD（噪声过滤），噪声或音乐素材上可能出现整段幻觉文本；下载任意 Whisper 模型会一并安装。'
}

/** 下载源只有两个合法值；旧数据或异常值一律回落官方源。 */
export function normalizeDownloadSource(value) {
  return value === 'mirror' ? 'mirror' : 'official'
}

/** 下载完成后，如果清单项带有设置键，就返回应自动选中的配置。 */
export function installedModelSelection(job, models) {
  if (!job || job.status !== 'done') {
    return null
  }
  const model = (Array.isArray(models) ? models : []).find((item) => item.id === job.model_id)
  const configKeys = new Set(['whisper_model', 'qwen_asr_model', 'sense_voice_model'])
  if (!model || model.status !== 'installed' || !configKeys.has(model.config_key) || !model.select_value) {
    return null
  }
  return { key: model.config_key, value: model.select_value }
}

export const DOWNLOAD_SOURCES = Object.freeze([
  { value: 'official', label: '官方源（Hugging Face / GitHub）' },
  { value: 'mirror', label: '国内镜像（hf-mirror.com；GitHub 项仍走官方）' }
])
