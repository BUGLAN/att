// 导出下载工具：把后端导出 URL 触发为浏览器下载
// 被 useRecords.exportRecord 与记录集页面的导出按钮共用

/**
 * 触发一个 URL 的文件下载。
 * @param {string} url 后端导出端点地址
 * @param {string} filename 保存文件名（含扩展名）
 */
export function downloadFromUrl(url, filename) {
  const link = document.createElement('a')
  link.href = url
  link.target = '_blank'
  link.rel = 'noopener'
  link.download = filename
  document.body.appendChild(link)
  link.click()
  link.remove()
}

/**
 * 由记录构造导出文件名：att-{日期时间}-{短 id}.{扩展名}
 * @param {{ id: string, created_at?: string }} record 记录对象
 * @param {string} format 导出格式（扩展名），如 txt / json
 */
export function buildExportFilename(record, format) {
  let stamp = 'export'
  if (record?.created_at) {
    const date = new Date(record.created_at)
    if (!Number.isNaN(date.getTime())) {
      const pad = (value) => String(value).padStart(2, '0')
      stamp = `${date.getFullYear()}${pad(date.getMonth() + 1)}${pad(date.getDate())}-${pad(date.getHours())}${pad(date.getMinutes())}`
    }
  }

  const shortId = String(record?.id || '').replace(/-/g, '').slice(0, 8) || 'record'
  return `att-${stamp}-${shortId}.${format}`
}

/** 批量导出可选的格式（记录集「导出当前结果」「导出选中」的弹出菜单） */
export const EXPORT_FORMATS = Object.freeze([
  { value: 'md', label: 'Markdown（.md）' },
  { value: 'txt', label: '纯文本（.txt）' },
  { value: 'json', label: 'JSON（.json）' },
  { value: 'csv', label: '表格（.csv）' }
])

/**
 * 批量导出的查询串（GET /api/records/export）：选中的 id（逗号分隔）或筛选条件二选一；不含令牌。
 * filter 与 /records/search 的参数同名：q、search_type、favorite_only、status、date_from、date_to。
 */
export function buildRecordsExportQuery({ ids = null, filter = {}, format = 'md', useOptimized = true } = {}) {
  const params = new URLSearchParams()
  params.set('format', format)
  params.set('use_optimized', useOptimized ? 'true' : 'false')
  if (Array.isArray(ids)) {
    params.set('ids', ids.join(','))
    return params.toString()
  }
  const value = filter || {}
  if (value.q) params.set('q', value.q)
  if (value.search_type && value.search_type !== 'all') params.set('search_type', value.search_type)
  if (value.favorite_only) params.set('favorite_only', 'true')
  if (value.status) params.set('status', value.status)
  if (value.date_from) params.set('date_from', value.date_from)
  if (value.date_to) params.set('date_to', value.date_to)
  return params.toString()
}

/** 批量导出的保存文件名：att-records-{日期时间}-{条数}.{扩展名}（与后端 Content-Disposition 同形） */
export function buildBatchExportFilename(format, count, now = new Date()) {
  const pad = (value) => String(value).padStart(2, '0')
  const stamp = `${now.getFullYear()}${pad(now.getMonth() + 1)}${pad(now.getDate())}-${pad(now.getHours())}${pad(now.getMinutes())}`
  return `att-records-${stamp}-${Math.max(0, Number(count) || 0)}.${format}`
}

