// 设置页「关于」的检查更新展示逻辑（纯函数，tests/test_update_view.mjs 覆盖）。
//
// 数据来自 GET/POST /api/update*（att-server/src/update.rs）：
//   check:    { status: 'available' | 'latest' | 'error', current_version, latest_version, notes, published_at, asset, error, checked_at }
//   download: { status: 'idle' | 'downloading' | 'done' | 'installing' | 'error', mode, version, file_name, path, received, total, error }
//   in_app:   桌面版有应用内更新（下载签名更新包 → 「重启并更新」）；mode = 'in_app' | 'installer' 标明这次下的是哪种

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
 * 检查结果的展示：{ tone: 'ok' | 'accent' | 'err', title, detail }；还没检查过返回 null。
 * 三种结果都要有明确反馈，失败时 detail 就是后端给的原因。
 */
export function describeCheck(check, { inApp = false } = {}) {
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
    if (inApp) {
      // 应用内更新包的大小以更新清单为准，下载时显示进度
    } else if (!check.asset) {
      parts.push('发布里没有适合本机的安装包，请到发布页手动下载')
    } else if (check.asset.size) {
      parts.push(`安装包 ${formatBytes(check.asset.size)}`)
    }
    return { tone: 'accent', title: `发现新版本 v${check.latest_version}`, detail: parts.join(' · ') }
  }
  if (check.status === 'latest') {
    return { tone: 'ok', title: `已是最新版本（${current}）`, detail: '' }
  }
  return { tone: 'err', title: '检查更新失败', detail: check.error || '未知原因' }
}

/**
 * 版本号旁的小标记：{ tone: 'ok' | 'accent', text }；没检查过或检查失败返回 null。
 * 「已是最新」只用这个标记提示，不再另起提示条。
 */
export function versionBadge(check) {
  if (check?.status === 'latest') {
    return { tone: 'ok', text: '已是最新' }
  }
  if (check?.status === 'available' && check.latest_version) {
    return { tone: 'accent', text: `可更新至 v${check.latest_version}` }
  }
  return null
}

/**
 * 「检查更新」那一行的说明：{ error, meta }。
 * 检查失败的原因写在这一行（红字），不另起提示条；meta 是上次检查时间或默认说明。
 */
export function describeCheckLine(check) {
  if (!check) {
    return { error: '', meta: '' }
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
  const inApp = download.mode === 'in_app'
  if (download.status === 'installing') {
    return '正在安装更新，应用会自动重启…'
  }
  if (download.status === 'done') {
    return inApp
      ? `v${download.version} 已下载，点「重启并更新」即可完成更新`
      : `已下载 v${download.version}：${download.file_name}，打开安装包即可完成更新`
  }
  return `${inApp ? '更新失败' : '下载失败'}：${download.error || '未知原因'}`
}

/**
 * 是否可以手动开始下载：有新版本、能下载（有应用内更新，或发布里有本机安装包），
 * 且没在下载 / 安装，这个版本也还没下好。
 */
export function canStartDownload(check, download, { inApp = false } = {}) {
  if (check?.status !== 'available' || !(inApp || check.asset)) {
    return false
  }
  if (download?.status === 'downloading' || download?.status === 'installing') {
    return false
  }
  return !(download?.status === 'done' && download.version === check.latest_version)
}

/** 应用内更新包已下好：可以「重启并更新」 */
export function canInstall(download) {
  return download?.mode === 'in_app' && download.status === 'done'
}

/** 应用内更新失败时的退路：改为下载完整安装包手动安装（发布里要有本机安装包） */
export function canFallbackToInstaller(check, download) {
  return check?.status === 'available' && Boolean(check.asset) &&
    download?.mode === 'in_app' && download.status === 'error'
}

/** 发布说明摘要：去掉空行，最多 maxLines 行 */
export function summarizeNotes(notes, maxLines = 6) {
  const lines = String(notes || '').split(/\r?\n/).map((line) => line.trimEnd()).filter((line) => line.trim())
  if (lines.length <= maxLines) {
    return lines.join('\n')
  }
  return `${lines.slice(0, maxLines).join('\n')}\n…`
}
