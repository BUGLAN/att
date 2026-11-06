// 桌面端原生录音会话映射（src/composables/recorder/nativeSession.mjs）单元测试。
// 覆盖：各相位的互斥标志与计时器动作、完成态提示文案、空文本与降级路径、未知相位。
import test from 'node:test'
import assert from 'node:assert/strict'

import { NATIVE_PHASES, deriveNativeSessionPatch } from '../src/composables/recorder/nativeSession.mjs'

test('录音相位：connecting 起表并清空上一轮文本，流式 recording 不重复起表', () => {
  const connecting = deriveNativeSessionPatch({ phase: 'connecting', mode: 'streaming' })
  assert.equal(connecting.recording, true)
  assert.equal(connecting.clock, 'start')
  assert.equal(connecting.resetTranscript, true)

  const streaming = deriveNativeSessionPatch({ phase: 'recording', mode: 'streaming' })
  assert.equal(streaming.livePhase, 'streaming')
  assert.equal(streaming.clock, null)

  const single = deriveNativeSessionPatch({ phase: 'recording', mode: 'single' })
  assert.equal(single.livePhase, 'recording')
  assert.equal(single.clock, 'start')
})

test('收尾相位：录音与转写标志互斥', () => {
  const patch = deriveNativeSessionPatch({ phase: 'finalizing', mode: 'streaming' })
  assert.equal(patch.recording, false)
  assert.equal(patch.transcribing, true)
  assert.equal(patch.clock, 'stop')
})

test('完成态：按复制粘贴结果选择提示文案（真流式用「复制或粘贴失败」）', () => {
  const pasted = deriveNativeSessionPatch({
    phase: 'completed', mode: 'streaming', text: '你好', copied: true, pasted: true, record: { id: 'r1' }
  })
  assert.equal(pasted.toast, '流式转写完成，已复制并粘贴')
  assert.equal(pasted.transcript, '你好')
  assert.deepEqual(pasted.record, { id: 'r1' })
  assert.equal(pasted.transcribing, false)

  const failed = deriveNativeSessionPatch({
    phase: 'completed', mode: 'streaming', text: '你好', copied: true, pasted: false, copy_error: '已复制，需要权限'
  })
  assert.equal(failed.toast, '流式转写完成，复制或粘贴失败')

  const plain = deriveNativeSessionPatch({ phase: 'completed', mode: 'single', text: '你好' })
  assert.equal(plain.toast, '转写完成')
})

test('完成态：空文本与降级', () => {
  const empty = deriveNativeSessionPatch({ phase: 'completed', mode: 'streaming', text: '', message: '录音时间过短' })
  assert.equal(empty.toast, '录音时间过短')
  assert.equal(empty.record, null)

  const degraded = deriveNativeSessionPatch({
    phase: 'completed', mode: 'streaming', text: '部分', degraded: true, message: '识别中断，已保存已识别内容：x'
  })
  assert.equal(degraded.toast, '识别中断，已保存已识别内容：x')
})

test('错误与取消：复位标志并提示', () => {
  const error = deriveNativeSessionPatch({ phase: 'error', message: '未配置 Token' })
  assert.equal(error.recording, false)
  assert.equal(error.transcribing, false)
  assert.equal(error.toast, '未配置 Token')

  const cancelled = deriveNativeSessionPatch({ phase: 'cancelled' })
  assert.equal(cancelled.livePhase, 'idle')
  assert.equal(cancelled.toast, '已取消本次录音')
})

test('相位全集都有映射，未知相位忽略', () => {
  for (const phase of NATIVE_PHASES) {
    assert.ok(deriveNativeSessionPatch({ phase }), phase)
  }
  assert.equal(deriveNativeSessionPatch({ phase: 'weird' }), null)
})

test('自动优化：optimizing 开优化流面板；单段保持处理中、流式不锁热键', () => {
  const single = deriveNativeSessionPatch({ phase: 'optimizing', mode: 'single', text: '原文', record: { id: 'r1' } })
  assert.equal(single.livePhase, 'optimizing')
  assert.equal(single.optimization, 'begin')
  assert.equal(single.transcribing, true)
  assert.equal(single.recording, false)
  assert.equal(single.transcript, '原文')
  assert.deepEqual(single.record, { id: 'r1' })

  const streaming = deriveNativeSessionPatch({ phase: 'optimizing', mode: 'streaming' })
  assert.equal(streaming.transcribing, false)
})

test('自动优化完成态：成功输入优化文，失败兜底原文并用失败色，取消不粘贴', () => {
  const done = deriveNativeSessionPatch({
    phase: 'completed', mode: 'streaming', text: '优化文', optimized: true, copied: true, pasted: true
  })
  assert.equal(done.optimization, 'finish')
  assert.equal(done.transcript, '优化文')
  assert.equal(done.toast, '优化完成，已输入并复制')

  const failed = deriveNativeSessionPatch({
    phase: 'completed', mode: 'single', text: '原文', optimize_error: '鉴权失败', copied: true, pasted: true
  })
  assert.equal(failed.optimization, 'fail')
  assert.equal(failed.optimizeError, '鉴权失败')
  assert.equal(failed.toast, '优化失败，已粘贴转写文本')
  assert.equal(failed.toastTone, 'error')

  // 已流式输入部分优化文后失败：不再粘贴原文，提示原文在剪贴板
  const partial = deriveNativeSessionPatch({
    phase: 'completed', mode: 'streaming', text: '原文', optimize_error: '超时', copied: true, pasted: false,
    copy_error: '优化中断：输入框里是部分优化结果，原文已复制到剪贴板'
  })
  assert.equal(partial.toast, '优化中断：输入框里是部分优化结果，原文已复制到剪贴板')
  assert.equal(partial.toastTone, 'error')

  const cancelled = deriveNativeSessionPatch({
    phase: 'completed', mode: 'single', text: '原文', optimization_cancelled: true
  })
  assert.equal(cancelled.optimization, 'cancel')
  assert.equal(cancelled.toast, '已取消优化，保留原始文本')

  const plain = deriveNativeSessionPatch({ phase: 'completed', mode: 'single', text: '原文', copied: true, pasted: true })
  assert.equal(plain.optimization, null)
})
