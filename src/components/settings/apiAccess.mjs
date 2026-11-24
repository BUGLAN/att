// 设置 → 诊断 →「外部 API」纯逻辑：目录文本规整、绝对路径判断与令牌显示。
// 与 Vue 无关，便于 tests/test_settings_api_access.mjs 锁定行为。

/** 去掉首尾空白与「复制为路径」带的成对引号。 */
export function normalizeDirectoryEntry(value) {
  const text = String(value ?? '').trim()
  return text.startsWith('"') && text.endsWith('"') ? text.slice(1, -1).trim() : text
}

/**
 * 是否是绝对路径：Windows 盘符、UNC 共享，或 POSIX 根路径。
 * 允许目录只接受绝对路径（后端同样校验），相对路径无法可靠判断用户指的是哪里。
 */
export function isAbsoluteDirectoryPath(value) {
  const text = normalizeDirectoryEntry(value)
  if (!text) {
    return false
  }
  return /^[a-zA-Z]:[\\/]/.test(text) || /^\\\\[^\\/]+[\\/][^\\/]+/.test(text) || text.startsWith('/')
}

/** 去重比较键：统一分隔符、去掉末尾分隔符（Windows 大小写不敏感）。 */
function directoryKey(value) {
  return normalizeDirectoryEntry(value)
    .replace(/\\/g, '/')
    .replace(/\/+$/, '')
    .toLowerCase()
}

/**
 * 把一个目录加入列表：规整、校验绝对路径、去重。
 * 返回新数组（不原地改），出错时列表原样返回并带中文 error。
 */
export function addAllowedDirectory(list, value) {
  const entry = normalizeDirectoryEntry(value)
  const current = Array.isArray(list) ? list : []
  if (!entry) {
    return { list: [...current], error: '请输入目录路径' }
  }
  if (!isAbsoluteDirectoryPath(entry)) {
    return { list: [...current], error: '请填写绝对路径（例如 D:\\audio 或 /Users/me/audio）' }
  }
  const key = directoryKey(entry)
  if (current.some((dir) => directoryKey(dir) === key)) {
    return { list: [...current], error: '该目录已在列表中' }
  }
  return { list: [...current, entry], error: '' }
}

/** 令牌显示：未开启 / 正在生成 / 已脱敏 / 用户点了「显示」后的明文。 */
export function apiTokenDisplay({ enabled, hasToken, revealedToken }) {
  if (!enabled) {
    return '未开启'
  }
  if (!hasToken) {
    return '正在生成…'
  }
  return revealedToken || 'att_••••••••••••••••'
}
