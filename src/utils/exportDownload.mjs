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
