import { api } from '@/api/client'

export const OVERLAY_CLIPBOARD_FILTERS = [
  { value: 'all', label: '全部' },
  { value: 'favorite', label: '收藏' },
  { value: 'text', label: '文本' },
  { value: 'image', label: '图片' },
  { value: 'file', label: '文件' },
  { value: 'link', label: '链接' }
]

export const PAGE_CLIPBOARD_FILTERS = [
  { value: 'all', label: '全部内容' },
  { value: 'text', label: '纯文本' },
  { value: 'image', label: '图片' },
  { value: 'file', label: '文件' },
  { value: 'link', label: '链接' }
]

const typeLabels = {
  text: '文本',
  link: '链接',
  code: '长文本',
  file: '文件',
  image: '图片'
}

const typeIcons = {
  text: 'T',
  link: 'L',
  code: '</>',
  file: 'F',
  image: 'I'
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

export function getClipboardTypeIcon(type) {
  return typeIcons[type] || '.'
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

export function buildClipboardQueryParams(filterValue, query, limit = 30) {
  const params = { limit }
  const normalizedQuery = String(query || '').trim()

  if (normalizedQuery) {
    params.query = normalizedQuery
  }

  if (filterValue === 'favorite') {
    params.favorite_only = true
    return params
  }

  if (['text', 'image', 'file', 'link', 'code'].includes(filterValue)) {
    params.item_type = filterValue
  }

  return params
}

async function buildClipboardPreviewUrl(item) {
  const sourceUrl = await api.getClipboardItemImageUrl(
    item.id,
    item.updated_at || item.created_at || ''
  )
  const response = await fetch(sourceUrl)
  if (!response.ok) {
    throw new Error(`Failed to load clipboard image: HTTP ${response.status}`)
  }

  const blob = await response.blob()
  return URL.createObjectURL(blob)
}

export async function enrichClipboardItems(items = []) {
  return Promise.all((items || []).map(async (item) => {
    if (!isClipboardImageItem(item)) {
      return item
    }

    let previewUrl = ''
    try {
      previewUrl = await buildClipboardPreviewUrl(item)
    } catch (error) {
      console.error('[ClipboardPresentation] Failed to build preview URL:', error)
    }

    return {
      ...item,
      preview_url: previewUrl
    }
  }))
}

export function revokeClipboardPreviewUrls(items = []) {
  for (const item of items || []) {
    const previewUrl = String(item?.preview_url || '')
    if (previewUrl.startsWith('blob:')) {
      URL.revokeObjectURL(previewUrl)
    }
  }
}
