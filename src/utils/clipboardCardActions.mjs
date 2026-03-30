export function buildClipboardTogglePatch(item, field) {
  if (!item || !item.id) {
    throw new Error('Clipboard item is required')
  }

  if (field !== 'is_favorite' && field !== 'is_pinned') {
    throw new Error(`Unsupported clipboard field: ${field}`)
  }

  return {
    id: item.id,
    [field]: !Boolean(item[field])
  }
}

export function patchClipboardItems(items, patch) {
  const nextItems = Array.isArray(items) ? items : []
  if (!patch || !patch.id) {
    return nextItems
  }

  return nextItems.map((item) => (
    item.id === patch.id
      ? { ...item, ...patch }
      : item
  ))
}
