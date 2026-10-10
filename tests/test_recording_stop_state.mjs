import test from 'node:test'
import assert from 'node:assert/strict'

import { derivePostStopState, deriveStartAbortState, shouldQueueStopDuringStart } from '../src/utils/recordingStopState.mjs'

test('single recording enters transcribing immediately after a valid stop', () => {
  assert.deepEqual(
    derivePostStopState({ mode: 'single', durationSeconds: 3 }),
    {
      recording: false,
      transcribing: true,
      trayIcon: 'transcribing'
    }
  )
})

test('single recording stays idle after a too-short stop', () => {
  assert.deepEqual(
    derivePostStopState({ mode: 'single', durationSeconds: 0 }),
    {
      recording: false,
      transcribing: false,
      trayIcon: 'idle'
    }
  )
})

test('multi recording enters transcribing immediately after stop', () => {
  assert.deepEqual(
    derivePostStopState({ mode: 'multi', durationSeconds: 0 }),
    {
      recording: false,
      transcribing: true,
      trayIcon: 'transcribing'
    }
  )
})

test('aborted start returns recorder state to idle', () => {
  assert.deepEqual(
    deriveStartAbortState(),
    {
      recording: false,
      transcribing: false,
      trayIcon: 'idle'
    }
  )
})

// 回归用例：历史缺陷把判据写成 `startPending && !isRecording`，而 prepareRecordingEnvironment
// 在开麦克风之前就把 isRecording 置真 → 整个启动窗口内该判据恒为假 → 松手被当成放弃/收尾，
// 最终开出一段无人停止的幽灵录音。这里显式传入 isRecording:true 锁死"它不得成为判据"。
test('启动窗口内收到停止请求必须排队（即使 isRecording 已被 prepare 置真）', () => {
  assert.equal(
    shouldQueueStopDuringStart({ startPending: true, isRecording: true }),
    true
  )
})

// 同理不要依赖 currentSessionId：意外断开路径不会清空它，残留 id 一旦被当成
// "会话已建立"就会重新绕开排队。显式传入一个残留 id 锁死这条约束。
test('启动窗口内存在残留 currentSessionId 也必须排队', () => {
  assert.equal(
    shouldQueueStopDuringStart({ startPending: true, currentSessionId: 'stale-session-id' }),
    true
  )
})

test('启动已结束（startPending 为假）时不排队', () => {
  assert.equal(
    shouldQueueStopDuringStart({ startPending: false }),
    false
  )
})

test('fromStart 的消费路径不得再入队，否则排队停止永远消费不掉', () => {
  assert.equal(
    shouldQueueStopDuringStart({ startPending: true, fromStart: true }),
    false
  )
})
