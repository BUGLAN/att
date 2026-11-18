// 首页引擎标签状态（src/composables/recorder/engineStatus.mjs）单元测试。
//
// 锁死的真实问题：在线凭据还在验证时，首页抢先显示「识别引擎已就绪」。
import test from 'node:test'
import assert from 'node:assert/strict'

import { deriveRecorderEngineStatus } from '../src/composables/recorder/engineStatus.mjs'

const BASE = {
  engineText: '豆包语音 · 单段',
  serviceLoading: false,
  buttonDisabled: false,
  disabledTitle: '识别引擎未就绪'
}

test('凭据验证中：显示验证中，不显示已就绪', () => {
  assert.deepEqual(
    deriveRecorderEngineStatus({ ...BASE, credentialCheck: { status: 'checking' } }),
    { text: '豆包语音 · 单段 · 验证中', dot: 'gold pulse', title: '正在验证凭据' }
  )
})

test('验证结束后：恢复 ok，不阻塞录音', () => {
  assert.deepEqual(
    deriveRecorderEngineStatus({ ...BASE, credentialCheck: { status: 'ok' } }),
    { text: '豆包语音 · 单段', dot: 'ok', title: '识别引擎已就绪' }
  )
})

test('验证没能下结论：不改变后端允许录音时的就绪展示', () => {
  assert.deepEqual(
    deriveRecorderEngineStatus({ ...BASE, credentialCheck: { status: 'unknown' } }),
    { text: '豆包语音 · 单段', dot: 'ok', title: '识别引擎已就绪' }
  )
})

test('验证确定失败后引擎不可用：显示不可用原因', () => {
  assert.deepEqual(
    deriveRecorderEngineStatus({
      ...BASE,
      buttonDisabled: true,
      disabledTitle: '识别引擎未就绪',
      credentialCheck: { status: 'invalid' }
    }),
    { text: '豆包语音 · 单段', dot: 'warn', title: '识别引擎未就绪' }
  )
})

test('模型加载与不可用优先于凭据验证中', () => {
  assert.deepEqual(
    deriveRecorderEngineStatus({
      ...BASE,
      serviceLoading: true,
      credentialCheck: { status: 'checking' }
    }),
    { text: '豆包语音 · 单段 · 模型加载中', dot: 'gold pulse', title: '模型加载中' }
  )
  assert.deepEqual(
    deriveRecorderEngineStatus({
      ...BASE,
      buttonDisabled: true,
      credentialCheck: { status: 'checking' }
    }),
    { text: '豆包语音 · 单段', dot: 'warn', title: '识别引擎未就绪' }
  )
})
