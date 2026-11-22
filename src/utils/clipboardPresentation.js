const typeLabels = {
  text: '文本',
  link: '链接',
  code: '长文本',
  file: '文件',
  image: '图片'
}

function looksLikeGeneratedImageName(name) {
  return /^[a-f0-9]{64}\.[a-z0-9]+$/i.test(String(name || '').trim())
}

export function isClipboardImageItem(item) {
  return item?.item_type === 'image'
}

export function getClipboardTypeLabel(type) {
  return typeLabels[type] || '记录'
}

export function getClipboardDisplayTitle(item) {
  const metadata = item?.metadata || {}
  const displayName = String(metadata.display_name || '').trim()
  if (displayName) {
    return displayName
  }

  const fileName = String(metadata.file_name || '').trim()
  if (fileName && !looksLikeGeneratedImageName(fileName)) {
    return fileName
  }

  const title = String(item?.title || '').trim()
  if (title) {
    return title
  }

  return isClipboardImageItem(item) ? '未命名图片' : '未命名记录'
}

export function getClipboardSummaryText(item) {
  const metadata = item?.metadata || {}
  if (isClipboardImageItem(item)) {
    const width = Number(metadata.width || 0)
    const height = Number(metadata.height || 0)
    if (width > 0 && height > 0) {
      return `${width} × ${height}`
    }
  }

  const summary = String(item?.summary || '').trim()
  if (summary) {
    return summary
  }

  return String(item?.content || '').trim()
}
