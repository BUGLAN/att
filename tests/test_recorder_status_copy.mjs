// 状态 / 完成态文案模块（src/composables/recorder/statusCopy.mjs）单元测试。
// 覆盖正常路径 + 边界：未知阶段、真流式特有文案。
import test from 'node:test'
import assert from 'node:assert/strict'

import {
  deriveCompletionToast
} from '../src/composables/recorder/statusCopy.mjs'
// 注：deriveOptimizationDegradedToast 已随 LLM v2（移除 .env 内置 key 兜底）删除，
// 后端不再广播 optimization_degraded（降级改由 done 事件的 degraded 字段表达）。

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
  assert.equal(deriveCompletionToast('streaming', { pasted: true }), '流式转写完成，已复制并粘贴')
  assert.equal(deriveCompletionToast('streaming', { attempted: true }), '流式转写完成，复制或粘贴失败')
  assert.equal(deriveCompletionToast('streaming', {}), '流式转写完成')
})

test('deriveCompletionToast：已复制但粘贴失败时单独提示，不再说复制失败', () => {
  const copiedOnly = { attempted: true, copied: true, pasted: false, copyError: '已复制，辅助功能未授权' }
  assert.equal(deriveCompletionToast('transcription', copiedOnly), '已复制，粘贴失败（多为缺少「辅助功能」权限）')
  assert.equal(deriveCompletionToast('streaming', copiedOnly), '已复制，粘贴失败（多为缺少「辅助功能」权限）')
})

test('deriveCompletionToast：目标窗口已切换时只提示已复制，不说粘贴失败', () => {
  for (const stage of ['transcription', 'optimization', 'streaming']) {
    const focusChanged = {
      attempted: true,
      copied: true,
      pasted: false,
      copyError: '已复制，目标窗口已切换，文本已复制到剪贴板'
    }
    assert.equal(deriveCompletionToast(stage, focusChanged), '目标窗口已切换，文本已复制到剪贴板', stage)
  }
})

test('deriveCompletionToast：未知阶段抛错，避免静默拿到错误文案', () => {
  assert.throws(() => deriveCompletionToast('unknown', { pasted: true }), /未知的完成态阶段/)
  assert.throws(() => deriveCompletionToast(undefined, {}), /未知的完成态阶段/)
})

test('deriveCompletionToast：覆盖三个完成态阶段', () => {
  const stages = ['optimization', 'streaming', 'transcription']
  for (const stage of stages) {
    assert.equal(typeof deriveCompletionToast(stage, {}), 'string', stage)
  }
})
