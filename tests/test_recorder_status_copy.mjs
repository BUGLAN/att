// 状态 / 完成态文案模块（src/composables/recorder/statusCopy.mjs）单元测试。
// 覆盖正常路径 + 边界：未知阶段、真流式特有文案。
import test from 'node:test'
import assert from 'node:assert/strict'

import {
  IDLE_SESSION_STATUS_TEXT,
  deriveCompletionToast,
  listCompletionStages
} from '../src/composables/recorder/statusCopy.mjs'
// 注：deriveOptimizationDegradedToast 已随 LLM v2（移除 .env 内置 key 兜底）删除，
// 后端不再广播 optimization_degraded（降级改由 done 事件的 degraded 字段表达）。

test('IDLE_SESSION_STATUS_TEXT：空闲态文案', () => {
  assert.equal(IDLE_SESSION_STATUS_TEXT, '按快捷键开始录音')
})

test('deriveCompletionToast：转写阶段三态文案', () => {
  assert.equal(
    deriveCompletionToast('transcription', { attempted: true, copied: true, pasted: true }),
    '转写完成，已复制并粘贴'
  )
  assert.equal(
    deriveCompletionToast('transcription', { attempted: true, copied: false, pasted: false }),
    '转写完成，复制并粘贴失败'
  )
  assert.equal(
    deriveCompletionToast('transcription', { attempted: false, copied: false, pasted: false }),
    '转写完成'
  )
})

test('deriveCompletionToast：优化阶段三态文案', () => {
  assert.equal(deriveCompletionToast('optimization', { pasted: true }), '优化完成，已输入并复制')
  assert.equal(deriveCompletionToast('optimization', { attempted: true }), '优化完成，自动输入失败')
  assert.equal(deriveCompletionToast('optimization', {}), '优化完成')
})

test('deriveCompletionToast：真流式阶段用的是"复制或粘贴失败"（与其他阶段不同）', () => {
  assert.equal(deriveCompletionToast('streaming', { pasted: true }), '真流式转写完成，已复制并粘贴')
  assert.equal(deriveCompletionToast('streaming', { attempted: true }), '真流式转写完成，复制或粘贴失败')
  assert.equal(deriveCompletionToast('streaming', {}), '真流式转写完成')
})

test('deriveCompletionToast：未知阶段抛错，避免静默拿到错误文案', () => {
  assert.throws(() => deriveCompletionToast('unknown', { pasted: true }), /未知的完成态阶段/)
  assert.throws(() => deriveCompletionToast(undefined, {}), /未知的完成态阶段/)
})

test('listCompletionStages：覆盖三个完成态阶段', () => {
  assert.deepEqual(listCompletionStages().sort(), ['optimization', 'streaming', 'transcription'])
})
