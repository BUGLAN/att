// 记录集批量操作的纯逻辑：选择只保留当前可见项，删除按 settled 结果分别更新。

export function retainVisibleRecordIds(selectedIds, records) {
  const visible = new Set((records || []).map((record) => record.id))
  return new Set([...selectedIds].filter((id) => visible.has(id)))
}

export function summarizeRecordDeleteResults(ids, results) {
  const deletedIds = []
  const failedIds = []
  ids.forEach((id, index) => {
    if (results[index]?.status === 'fulfilled') {
      deletedIds.push(id)
    } else {
      failedIds.push(id)
    }
  })
  return { deletedIds, failedIds }
}
