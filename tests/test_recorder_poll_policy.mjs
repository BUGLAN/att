// 轮询策略模块（src/composables/recorder/pollPolicy.mjs）单元测试。
// 覆盖正常路径 + 边界：非法计数、达到失败上限、固定节拍与 tick 守卫。
import test from 'node:test'
import assert from 'node:assert/strict'

import {
  MAX_POLL_FAILURES,
  POLL_INTERVAL_MS,
  hasReachedPollFailureLimit,
  nextPollFailureCount,
  shouldSkipPollTick,
  shouldStopPollingAfterTick
} from '../src/composables/recorder/pollPolicy.mjs'

test('轮询常量与原实现一致（上限 30 次、固定 1s 节拍）', () => {
  assert.equal(MAX_POLL_FAILURES, 30)
  assert.equal(POLL_INTERVAL_MS, 1000)
})

test('nextPollFailureCount：正常累加', () => {
  assert.equal(nextPollFailureCount(0), 1)
  assert.equal(nextPollFailureCount(1), 2)
  assert.equal(nextPollFailureCount(29), 30)
})

test('nextPollFailureCount：负数 / NaN / undefined 等非法计数按 0 起算', () => {
  assert.equal(nextPollFailureCount(-5), 1)
  assert.equal(nextPollFailureCount(Number.NaN), 1)
  assert.equal(nextPollFailureCount(undefined), 1)
  assert.equal(nextPollFailureCount(0.5), 1)
})

test('hasReachedPollFailureLimit：默认上限 30', () => {
  assert.equal(hasReachedPollFailureLimit(0), false)
  assert.equal(hasReachedPollFailureLimit(29), false)
  assert.equal(hasReachedPollFailureLimit(30), true)
  assert.equal(hasReachedPollFailureLimit(31), true)
})

test('hasReachedPollFailureLimit：自定义上限与非法入参', () => {
  assert.equal(hasReachedPollFailureLimit(2, 3), false)
  assert.equal(hasReachedPollFailureLimit(3, 3), true)
  // 非法上限回退到 MAX_POLL_FAILURES
  assert.equal(hasReachedPollFailureLimit(5, 0), false)
  assert.equal(hasReachedPollFailureLimit(30, Number.NaN), true)
  // 非法计数按 0 处理，不会误判为达到上限
  assert.equal(hasReachedPollFailureLimit(Number.NaN), false)
})

test('累计失败到上限的仿真：恰好第 30 次失败才停止轮询', () => {
  let failureCount = 0
  let stoppedAt = null

  for (let attempt = 1; attempt <= 60; attempt += 1) {
    failureCount = nextPollFailureCount(failureCount)
    if (hasReachedPollFailureLimit(failureCount)) {
      stoppedAt = attempt
      break
    }
  }

  assert.equal(stoppedAt, MAX_POLL_FAILURES)
  assert.equal(failureCount, MAX_POLL_FAILURES)
})

test('shouldSkipPollTick：上一次请求未返回时跳过本 tick', () => {
  assert.equal(shouldSkipPollTick(true), true)
  assert.equal(shouldSkipPollTick(false), false)
  assert.equal(shouldSkipPollTick(undefined), false)
})

test('shouldStopPollingAfterTick：仅当本轮已完成才停止', () => {
  assert.equal(shouldStopPollingAfterTick(true), true)
  assert.equal(shouldStopPollingAfterTick(false), false)
  assert.equal(shouldStopPollingAfterTick(undefined), false)
})
