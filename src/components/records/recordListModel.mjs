// 录音记录列表的纯展示模型（首页「最近」与记录集共用；不依赖 Vue，供 tests/test_record_list_model.mjs 锁定）。
//
// 列表按天分组（今天 / 昨天 / 10 月 29 日），行内只放时刻、正文与时长；
// 状态只在「进行中 / 失败 / 已编辑」时用小圆点 + 灰字标出，已优化是常态不再标。

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
  if (record?.is_modified) return { text: '已编辑', dot: '' }
  return null
}
