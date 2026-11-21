// 设置 → 诊断 →「存储」的展示模型（不依赖 Vue，供 tests/test_storage_usage.mjs 锁定）。
//
// 后端：GET /api/storage → { items: [{ key, label, path, files, bytes }], total_bytes, measured_at }（缓存 1 分钟）；
// POST /api/storage/orphans/clear 清空 audio/orphans/。
// 录音音频保留天数（settings.audio_retention_days，0 = 永久）：每天检查一次，只删音频文件，记录与文字保留，收藏的不删；
// 改紧了（永久→有天数、天数变小）会立即删掉过期音频，先确认。

import { isTighter, normalizeLimit } from './clipboardLimits.mjs'

export { isTighter, normalizeLimit }

export const AUDIO_RETENTION_CHOICES = Object.freeze([0, 7, 30, 90, 180, 365])

const UNITS = ['B', 'KB', 'MB', 'GB', 'TB']

/** 字节数 → 「12.3 MB」（1024 进制，B 不带小数，其余保留一位、整数去掉 .0） */
export function formatBytes(bytes) {
  let value = Math.max(0, Number(bytes) || 0)
  let unit = 0
  while (value >= 1024 && unit < UNITS.length - 1) {
    value /= 1024
    unit += 1
  }
  if (unit === 0) {
    return `${Math.round(value)} B`
  }
  const text = value.toFixed(1).replace(/\.0$/, '')
  return `${text} ${UNITS[unit]}`
}

/** 数据目录里的路径显示成相对路径（`audio/records`），目录外的（模型目录可能在别处）显示完整路径 */
export function displayPath(path, dataDir) {
  const full = String(path || '')
  const root = String(dataDir || '').replace(/[\\/]+$/, '')
  if (root && full.length > root.length && full.startsWith(root) && /[\\/]/.test(full[root.length])) {
    return full.slice(root.length + 1).replace(/\\/g, '/')
  }
  return full
}

/** 占用表的每一行：{ key, label, path（显示用）, fullPath, size, files, empty } */
export function storageRows(report) {
  const items = Array.isArray(report?.items) ? report.items : []
  return items.map((item) => ({
    key: item.key,
    label: item.label || item.key,
    path: displayPath(item.path, report?.data_dir),
    fullPath: item.path || '',
    size: formatBytes(item.bytes),
    files: `${Number(item.files) || 0} 个文件`,
    empty: !(Number(item.files) > 0)
  }))
}

/** 孤儿音频还有没有可清理的 */
export function hasOrphans(report) {
  const orphans = (Array.isArray(report?.items) ? report.items : []).find((item) => item.key === 'orphans')
  return Number(orphans?.files) > 0
}

/** 录音音频保留天数的下拉：0 = 永久保留；当前值不在预设里也列出来 */
export function audioRetentionOptions(current = 0) {
  const value = normalizeLimit(current)
  const values = AUDIO_RETENTION_CHOICES.includes(value)
    ? [...AUDIO_RETENTION_CHOICES]
    : [...AUDIO_RETENTION_CHOICES, value].sort((a, b) => a - b)
  return values.map((days) => ({ value: days, label: days === 0 ? '永久保留' : `${days} 天后删除音频` }))
}

/** 改紧保留天数前的确认文案 */
export function audioRetentionConfirm(days) {
  const value = normalizeLimit(days)
  return {
    title: `只保留 ${value} 天内的录音音频？`,
    description: `${value} 天前的录音音频会立即删除，无法恢复；记录和文字都保留，收藏的记录不受影响。之后每天自动检查一次。`
  }
}
