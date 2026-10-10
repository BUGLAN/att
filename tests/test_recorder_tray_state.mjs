// 托盘图标状态推导模块（src/composables/recorder/trayState.mjs）单元测试。
// 覆盖优先级（录音 > 转写 > 未连接 > 就绪）与缺省值边界。
import test from 'node:test'
import assert from 'node:assert/strict'

import {
  TRAY_ICON_STATES,
  deriveTrayIconState
} from '../src/composables/recorder/trayState.mjs'

test('deriveTrayIconState：四个输入组合的完整真值表', () => {
  const cases = [
    // [recording, transcribing, connected, expected]
    [false, false, true, 'idle'],
    [false, false, false, 'disconnected'],
    [true, false, true, 'recording'],
    [true, false, false, 'recording'],
    [false, true, true, 'transcribing'],
    [false, true, false, 'transcribing'],
    [true, true, true, 'recording'],
    [true, true, false, 'recording']
  ]

  for (const [recording, transcribing, connected, expected] of cases) {
    assert.equal(
      deriveTrayIconState({ recording, transcribing, connected }),
      expected,
      `recording=${recording} transcribing=${transcribing} connected=${connected}`
    )
  }
})

test('deriveTrayIconState：正在监听时后端断连不抢图标（录音优先）', () => {
  assert.equal(
    deriveTrayIconState({ recording: true, transcribing: false, connected: false }),
    'recording'
  )
})

test('deriveTrayIconState：不传参 / 传空对象时按"就绪"处理', () => {
  assert.equal(deriveTrayIconState(), 'idle')
  assert.equal(deriveTrayIconState({}), 'idle')
  assert.equal(deriveTrayIconState(undefined), 'idle')
})

test('deriveTrayIconState：connected 只认显式 false', () => {
  assert.equal(deriveTrayIconState({ connected: undefined }), 'idle')
  assert.equal(deriveTrayIconState({ connected: null }), 'idle')
  assert.equal(deriveTrayIconState({ connected: 0 }), 'idle')
})

test('TRAY_ICON_STATES：导出四个状态且与推导结果同源', () => {
  assert.deepEqual([...TRAY_ICON_STATES], ['idle', 'recording', 'transcribing', 'disconnected'])

  const derived = new Set([
    deriveTrayIconState({ recording: true }),
    deriveTrayIconState({ transcribing: true }),
    deriveTrayIconState({ connected: false }),
    deriveTrayIconState({})
  ])

  for (const state of derived) {
    assert.ok(TRAY_ICON_STATES.includes(state), `推导出了未登记的状态：${state}`)
  }
})
