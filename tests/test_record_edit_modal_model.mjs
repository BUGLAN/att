// 编辑记录弹窗同步规则（src/components/modals/recordEditModel.mjs）单元测试。
// 覆盖：同一条记录同步、不同记录隔离、用户改动后保留草稿。
import test from 'node:test'
import assert from 'node:assert/strict'

import { recordEditText, resolveRecordEditSync } from '../src/components/modals/recordEditModel.mjs'

test('编辑文本优先使用优化文本，没有时回退原文', () => {
  assert.equal(recordEditText({ raw_text: '原文', optimized_text: '优化文' }), '优化文')
  assert.equal(recordEditText({ raw_text: '原文', optimized_text: '' }), '原文')
  assert.equal(recordEditText(null), '')
})

test('同一条记录未改动时，接受服务端更新并刷新基线', () => {
  const nextRecord = { id: 'r1', raw_text: '原文', optimized_text: '更新后的优化文' }
  assert.deepEqual(
    resolveRecordEditSync({
      recordId: 'r1',
      editedText: '旧优化文',
      baselineText: '旧优化文',
      nextRecord
    }),
    {
      record: nextRecord,
      text: '更新后的优化文',
      baselineText: '更新后的优化文'
    }
  )
})

test('当前记录切换为另一条时，不修改编辑目标和草稿', () => {
  assert.equal(
    resolveRecordEditSync({
      recordId: 'r1',
      editedText: '正在编辑',
      baselineText: '正在编辑',
      nextRecord: { id: 'r2', raw_text: '另一条记录' }
    }),
    null
  )
})

test('用户改动后，同一条记录的服务端更新也不覆盖草稿', () => {
  assert.equal(
    resolveRecordEditSync({
      recordId: 'r1',
      editedText: '用户新输入',
      baselineText: '打开时文本',
      nextRecord: { id: 'r1', raw_text: '服务端更新' }
    }),
    null
  )
})

test('打开弹窗时没有目标记录，不接收后续更新', () => {
  assert.equal(
    resolveRecordEditSync({
      recordId: null,
      editedText: '',
      baselineText: '',
      nextRecord: { id: 'r1', raw_text: '新记录' }
    }),
    null
  )
})
