// 状态 / 完成态文案模块（src/composables/recorder/statusCopy.mjs）单元测试。
// 覆盖正常路径 + 边界：未知阶段、非法 copyResult、真流式特有文案。
import test from 'node:test'
import assert from 'node:assert/strict'

import {
  IDLE_SESSION_STATUS_TEXT,
  deriveCompletionToast,
  deriveRealtimePhaseText,
  listCompletionStages
} from '../src/composables/recorder/statusCopy.mjs'
// 注：deriveOptimizationDegradedToast 已随 LLM v2（移除 .env 内置 key 兜底）删除，
// 后端不再广播 optimization_degraded（降级改由 done 事件的 degraded 字段表达）。

test('deriveRealtimePhaseText：多段录音各阶段文案与原实现一致', () => {
  assert.equal(deriveRealtimePhaseText('listening'), '静音切段已启用，停顿后会自动出字')
  assert.equal(deriveRealtimePhaseText('speaking'), '正在监听语音片段...')
  assert.equal(deriveRealtimePhaseText('segment-processing'), '上一段正在转写，继续说话即可')
  assert.equal(deriveRealtimePhaseText('finalizing'), '正在合并整场录音结果')
  assert.equal(deriveRealtimePhaseText('completed'), '多段录音处理完成')
  assert.equal(deriveRealtimePhaseText('error'), '本次录音未形成有效结果')
})

test('deriveRealtimePhaseText：未知阶段 / 空阶段回退空闲文案', () => {
  assert.equal(deriveRealtimePhaseText('idle'), IDLE_SESSION_STATUS_TEXT)
  assert.equal(deriveRealtimePhaseText('unknown-phase'), IDLE_SESSION_STATUS_TEXT)
  assert.equal(deriveRealtimePhaseText(undefined), IDLE_SESSION_STATUS_TEXT)
  assert.equal(deriveRealtimePhaseText(''), IDLE_SESSION_STATUS_TEXT)
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

test('deriveCompletionToast：多段录音阶段三态文案', () => {
  assert.equal(deriveCompletionToast('multi', { pasted: true }), '多段录音完成，已复制并粘贴')
  assert.equal(deriveCompletionToast('multi', { attempted: true }), '多段录音完成，复制并粘贴失败')
  assert.equal(deriveCompletionToast('multi', {}), '多段录音完成')
})

test('deriveCompletionToast：未知阶段抛错，避免静默拿到错误文案', () => {
  assert.throws(() => deriveCompletionToast('unknown', { pasted: true }), /未知的完成态阶段/)
  assert.throws(() => deriveCompletionToast(undefined, {}), /未知的完成态阶段/)
})

test('listCompletionStages：覆盖四个完成态阶段', () => {
  assert.deepEqual(listCompletionStages().sort(), ['multi', 'optimization', 'streaming', 'transcription'])
})
