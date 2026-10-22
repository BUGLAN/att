// 悬浮窗收尾判定（src/composables/recorder/floatingSession.mjs）单元测试。
// 覆盖“后台优化期间不能提前收尾”和“旧记录事件不能误收新会话”两类回归。
import test from 'node:test'
import assert from 'node:assert/strict'

import {
  ownsFloatingRecord,
  shouldAwaitBackgroundOptimization
} from '../src/composables/recorder/floatingSession.mjs'

test('shouldAwaitBackgroundOptimization：开启自动优化且记录尚未优化时要等待', () => {
  assert.equal(
    shouldAwaitBackgroundOptimization({
      autoOptimize: true,
      recordId: 'r1',
      recordStatus: 'transcribed',
      optimizationCompleted: false
    }),
    true
  )
})

test('shouldAwaitBackgroundOptimization：关闭自动优化时立即收尾', () => {
  assert.equal(
    shouldAwaitBackgroundOptimization({
      autoOptimize: false,
      recordId: 'r1',
      recordStatus: 'transcribed'
    }),
    false
  )
})

test('shouldAwaitBackgroundOptimization：没有 record id 时不等待', () => {
  assert.equal(
    shouldAwaitBackgroundOptimization({
      autoOptimize: true,
      recordId: '',
      recordStatus: 'transcribed'
    }),
    false
  )
})

test('shouldAwaitBackgroundOptimization：优化事件已先到或记录已优化时不再等待', () => {
  assert.equal(
    shouldAwaitBackgroundOptimization({
      autoOptimize: true,
      recordId: 'r1',
      recordStatus: 'transcribed',
      optimizationCompleted: true
    }),
    false
  )
  assert.equal(
    shouldAwaitBackgroundOptimization({
      autoOptimize: true,
      recordId: 'r1',
      recordStatus: 'optimized',
      optimizationCompleted: false
    }),
    false
  )
})

test('ownsFloatingRecord：只有当前会话绑定的同一个 record id 才算归属', () => {
  assert.equal(
    ownsFloatingRecord({ active: true, currentRecordId: 'r1', eventRecordId: 'r1' }),
    true
  )
  assert.equal(
    ownsFloatingRecord({ active: true, currentRecordId: 'r1', eventRecordId: 'r2' }),
    false
  )
  assert.equal(
    ownsFloatingRecord({ active: false, currentRecordId: 'r1', eventRecordId: 'r1' }),
    false
  )
})

test('ownsFloatingRecord：当前会话未绑定 record id 时拒绝任何事件', () => {
  assert.equal(
    ownsFloatingRecord({ active: true, currentRecordId: '', eventRecordId: 'r1' }),
    false
  )
  assert.equal(
    ownsFloatingRecord({ active: true, currentRecordId: undefined, eventRecordId: 'r1' }),
    false
  )
  assert.equal(
    ownsFloatingRecord({ active: true, currentRecordId: 'r1', eventRecordId: '' }),
    false
  )
})
