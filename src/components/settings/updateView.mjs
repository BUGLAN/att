// 设置页「关于」的检查更新展示逻辑（纯函数，tests/test_update_view.mjs 覆盖）。
//
// 数据来自 GET/POST /api/update*（att-server/src/update.rs）：
//   check:    { status: 'available' | 'latest' | 'error', current_version, latest_version, notes, published_at, asset, error, checked_at }
//   download: { status: 'idle' | 'downloading' | 'done' | 'error', version, file_name, path, received, total, error }

/** 字节数 → 人类可读（KB / MB），用于安装包大小与下载进度 */
export function formatBytes(bytes) {
  const value = Number(bytes) || 0
  if (value < 1024) {
    return `${value} B`
  }
  if (value < 1024 * 1024) {
    return `${(value / 1024).toFixed(1)} KB`
  }
  return `${(value / 1024 / 1024).toFixed(1)} MB`
}

/** ISO 时间 → `YYYY-MM-DD HH:mm`（本地时区）；无法解析时返回空串 */
export function formatDateTime(iso) {
  if (!iso) {
    return ''
  }
  const date = new Date(iso)
  if (Number.isNaN(date.getTime())) {
    return ''
  }
  const pad = (n) => String(n).padStart(2, '0')
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())} ${pad(date.getHours())}:${pad(date.getMinutes())}`
}

/**
 * 检查结果的展示：{ tone: 'ok' | 'info' | 'err', title, detail }；还没检查过返回 null。
 * 三种结果都要有明确反馈，失败时 detail 就是后端给的原因。
 */
export function describeCheck(check) {
  if (!check) {
    return null
  }
  const current = check.current_version ? `v${check.current_version}` : '当前版本'
  if (check.status === 'available') {
    const published = formatDateTime(check.published_at)
    const parts = [`当前 ${current}`]
    if (published) {
      parts.push(`发布于 ${published}`)
    }
    if (!check.asset) {
      parts.push('发布里没有适合本机的安装包，请到发布页手动下载')
    } else if (check.asset.size) {
      parts.push(`安装包 ${formatBytes(check.asset.size)}`)
    }
    return { tone: 'info', title: `发现新版本 v${check.latest_version}`, detail: parts.join(' · ') }
  }
  if (check.status === 'latest') {
    return { tone: 'ok', title: `已是最新版本（${current}）`, detail: '' }
  }
  return { tone: 'err', title: '检查更新失败', detail: check.error || '未知原因' }
}

/**
 * 版本号旁的小标记：{ tone: 'ok' | 'info', text }；没检查过或检查失败返回 null。
 * 「已是最新」只用这个标记提示，不再另起提示条。
 */
export function versionBadge(check) {
  if (check?.status === 'latest') {
    return { tone: 'ok', text: '已是最新' }
  }
  if (check?.status === 'available' && check.latest_version) {
    return { tone: 'info', text: `可更新至 v${check.latest_version}` }
  }
  return null
}

/**
 * 「检查更新」那一行的说明：{ error, meta }。
 * 检查失败的原因写在这一行（红字），不另起提示条；meta 是上次检查时间或默认说明。
 */
export function describeCheckLine(check) {
  if (!check) {
    return { error: '', meta: '从发布渠道获取最新正式版本，与当前版本比较。' }
  }
  const time = formatDateTime(check.checked_at)
  const meta = time ? `上次检查：${time}` : ''
  const error = check.status === 'error' ? `检查失败：${check.error || '未知原因'}` : ''
  return { error, meta }
}

/** 下载进度百分比（0–100 的整数）；总大小未知时返回 null */
export function downloadPercent(download) {
  if (!download?.total) {
    return null
  }
  return Math.min(100, Math.floor((download.received / download.total) * 100))
}

/** 下载状态的展示文案；空闲返回空串 */
export function describeDownload(download) {
  if (!download || download.status === 'idle') {
    return ''
  }
  if (download.status === 'downloading') {
    const percent = downloadPercent(download)
    const size = download.total
      ? `${formatBytes(download.received)} / ${formatBytes(download.total)}`
      : formatBytes(download.received)
    return percent === null ? `正在下载… ${size}` : `正在下载… ${percent}%（${size}）`
  }
  if (download.status === 'done') {
    return `已下载 v${download.version}：${download.file_name}，打开安装包即可完成更新`
  }
  return `下载失败：${download.error || '未知原因'}`
}

/** 是否可以手动开始下载：有新版本、有本机安装包，且没在下载 / 这个版本还没下好 */
export function canStartDownload(check, download) {
  if (check?.status !== 'available' || !check.asset) {
    return false
  }
  if (download?.status === 'downloading') {
    return false
  }
  return !(download?.status === 'done' && download.version === check.latest_version)
}

/** 发布说明摘要：去掉空行，最多 maxLines 行 */
export function summarizeNotes(notes, maxLines = 6) {
  const lines = String(notes || '').split(/\r?\n/).map((line) => line.trimEnd()).filter((line) => line.trim())
  if (lines.length <= maxLines) {
    return lines.join('\n')
  }
  return `${lines.slice(0, maxLines).join('\n')}\n…`
}
