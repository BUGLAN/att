// 录音记录列表的纯展示模型（首页「最近」与记录集共用；不依赖 Vue，供 tests/test_record_list_model.mjs 锁定）。
//
// 列表按天分组（今天 / 昨天 / 10 月 29 日），行内只放时刻、正文与时长；
// 状态只在「进行中 / 失败 / 已编辑」时用小圆点 + 灰字标出，已优化是常态不再标。

const NO_VOICE_PREFIX = '没有听到人声'

function toDate(value) {
  const date = value instanceof Date ? value : new Date(value)
  return Number.isNaN(date.getTime()) ? null : date
}

function startOfDay(date) {
  return new Date(date.getFullYear(), date.getMonth(), date.getDate())
}

/** 分组标题：今天 / 昨天 / 本年「10 月 29 日」/ 跨年「2025 年 10 月 29 日」 */
export function dayLabel(value, now = new Date()) {
  const date = toDate(value)
  if (!date) return '更早'
  const diff = Math.round((startOfDay(now) - startOfDay(date)) / 86400000)
  if (diff === 0) return '今天'
  if (diff === 1) return '昨天'
  const md = `${date.getMonth() + 1} 月 ${date.getDate()} 日`
  return date.getFullYear() === now.getFullYear() ? md : `${date.getFullYear()} 年 ${md}`
}

/** 按 created_at 的自然日分组，保持传入顺序（调用方已按时间倒序） */
export function groupRecordsByDay(records = [], now = new Date()) {
  const groups = []
  for (const record of records) {
    const label = dayLabel(record?.created_at, now)
    const last = groups[groups.length - 1]
    if (last && last.label === label) {
      last.records.push(record)
    } else {
      groups.push({ label, records: [record] })
    }
  }
  return groups
}

/** 行首时刻 HH:MM */
export function formatClock(value) {
  const date = toDate(value)
  if (!date) return '--:--'
  return `${String(date.getHours()).padStart(2, '0')}:${String(date.getMinutes()).padStart(2, '0')}`
}

/** 录音时长 m:ss */
export function formatRecordDuration(seconds) {
  const total = Math.max(0, Math.floor(Number(seconds) || 0))
  return `${Math.floor(total / 60)}:${String(total % 60).padStart(2, '0')}`
}

/**
 * 行内状态（小圆点 + 灰字）；常态（已转写 / 已优化）返回 null 不展示。
 * optimizing：该条正在流式优化（优化流 store 判定），优先于 record.status。
 */
export function recordStatusMeta(record, { optimizing = false } = {}) {
  if (optimizing || record?.status === 'optimizing') return { text: '优化中', dot: 'blue pulse' }
  if (record?.status === 'transcribing' || record?.status === 'pending') return { text: '转写中', dot: 'blue pulse' }
  if (record?.status === 'failed') return { text: '失败', dot: 'err' }
  if (typeof record?.error === 'string' && record.error.startsWith(NO_VOICE_PREFIX)) {
    return { text: '未听到人声', dot: 'warn', title: record.error }
  }
  if (record?.is_modified) return { text: '已编辑', dot: '' }
  return null
}

/** 记录集日期筛选的预设（自定义时用两个日期框） */
export const DATE_RANGE_OPTIONS = Object.freeze([
  { value: 'all', label: '全部时间' },
  { value: 'today', label: '今天' },
  { value: '7d', label: '近 7 天' },
  { value: '30d', label: '近 30 天' },
  { value: 'custom', label: '自定义' }
])

/** 记录集状态筛选（值与后端 status 一致；空 = 全部） */
export const STATUS_FILTER_OPTIONS = Object.freeze([
  { value: '', label: '全部状态' },
  { value: 'transcribed', label: '已识别' },
  { value: 'optimized', label: '已优化' },
  { value: 'failed', label: '失败' }
])

/** 本地日期 YYYY-MM-DD */
export function localDateString(date) {
  const value = toDate(date)
  if (!value) return ''
  const pad = (n) => String(n).padStart(2, '0')
  return `${value.getFullYear()}-${pad(value.getMonth() + 1)}-${pad(value.getDate())}`
}

/**
 * 日期预设 → { date_from, date_to }（本地日期，含两端；空串 = 不限）。
 * 近 7 天 = 今天往前共 7 个自然日；自定义时用 custom.from / custom.to（可只填一端，前后颠倒会自动对调）。
 */
export function dateRangeFor(preset, now = new Date(), custom = {}) {
  const today = startOfDay(toDate(now) || new Date())
  const daysAgo = (n) => localDateString(new Date(today.getFullYear(), today.getMonth(), today.getDate() - n))
  switch (preset) {
    case 'today':
      return { date_from: daysAgo(0), date_to: daysAgo(0) }
    case '7d':
      return { date_from: daysAgo(6), date_to: daysAgo(0) }
    case '30d':
      return { date_from: daysAgo(29), date_to: daysAgo(0) }
    case 'custom': {
      const valid = (value) => (/^\d{4}-\d{2}-\d{2}$/.test(String(value || '')) ? value : '')
      let from = valid(custom.from)
      let to = valid(custom.to)
      if (from && to && from > to) {
        [from, to] = [to, from]
      }
      return { date_from: from, date_to: to }
    }
    default:
      return { date_from: '', date_to: '' }
  }
}

