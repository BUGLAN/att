// 编辑记录弹窗的同步规则，不依赖 Vue，供 Node 测试锁定。

export function recordEditText(record) {
  return record?.optimized_text || record?.raw_text || ''
}

/**
 * 只有仍是打开弹窗时的那条记录，且用户没有改动文本，才接受服务端更新。
 * 返回应在弹窗内同步的记录、编辑文本和新的未修改基线；否则返回 null。
 */
export function resolveRecordEditSync({ recordId, editedText, baselineText, nextRecord }) {
  if (!recordId || !nextRecord || nextRecord.id !== recordId || editedText !== baselineText) {
    return null
  }

  const nextText = recordEditText(nextRecord)
  return {
    record: nextRecord,
    text: nextText,
    baselineText: nextText
  }
}
