// 录音采样纯计算模块（src/composables/recorder/pcm.mjs）单元测试。
// 覆盖正常路径 + 边界：空输入、超长输入、负值、队列不足、非法分片步长。
import test from 'node:test'
import assert from 'node:assert/strict'

import {
  STREAM_FLUSH_SAMPLES,
  appendSamples,
  calculateAudioLevel,
  convertFloat32ToPcm16Buffer,
  hasSamples,
  planFlushChunkSizes,
  takeSamplesFromQueue
} from '../src/composables/recorder/pcm.mjs'

// 把 ArrayBuffer 还原成 Int16Array，便于断言采样值
function toInt16(buffer) {
  return new Int16Array(buffer)
}

test('STREAM_FLUSH_SAMPLES 与流式上行分片大小保持一致（3200）', () => {
  assert.equal(STREAM_FLUSH_SAMPLES, 3200)
})

test('convertFloat32ToPcm16Buffer：常规输入按 ±满量程缩放', () => {
  const buffer = convertFloat32ToPcm16Buffer(Float32Array.from([0, 0.5, -0.5, 1, -1]))
  assert.ok(buffer instanceof ArrayBuffer)
  assert.equal(buffer.byteLength, 10)
  assert.deepEqual([...toInt16(buffer)], [0, 16383, -16384, 32767, -32768])
})

test('convertFloat32ToPcm16Buffer：越界采样被钳制到 ±1（正负两侧对称）', () => {
  const buffer = convertFloat32ToPcm16Buffer(Float32Array.from([2, -2, 100, -100]))
  assert.deepEqual([...toInt16(buffer)], [32767, -32768, 32767, -32768])
})

test('convertFloat32ToPcm16Buffer：空输入返回零长度 buffer', () => {
  const buffer = convertFloat32ToPcm16Buffer(Float32Array.from([]))
  assert.equal(buffer.byteLength, 0)
})

test('convertFloat32ToPcm16Buffer：超长输入（1 分钟 16kHz）长度正确且不抛错', () => {
  const samples = new Float32Array(960000)
  samples[0] = 0.25
  samples[samples.length - 1] = -0.25
  const view = toInt16(convertFloat32ToPcm16Buffer(samples))
  assert.equal(view.length, 960000)
  assert.equal(view[0], 8191)
  assert.equal(view[view.length - 1], -8192)
})

test('calculateAudioLevel：空输入与全零输入返回 0', () => {
  assert.equal(calculateAudioLevel(undefined), 0)
  assert.equal(calculateAudioLevel(Float32Array.from([])), 0)
  assert.equal(calculateAudioLevel(Float32Array.from([0, 0, 0])), 0)
})

test('calculateAudioLevel：RMS * 6 后钳制到 1', () => {
  // 0.125 在 Float32 中可精确表示，避免浮点误差影响断言
  assert.equal(calculateAudioLevel(Float32Array.from([0.125])), 0.75)
  // 满幅方波 RMS = 1，乘 6 后被钳制到 1，不会超过归一化上限
  assert.equal(calculateAudioLevel(Float32Array.from([1, 1, 1, 1])), 1)
  assert.equal(calculateAudioLevel(Float32Array.from([2, -2])), 1)
})

test('hasSamples：空 / 未定义视为无数据，非空帧视为有数据', () => {
  assert.equal(hasSamples(undefined), false)
  assert.equal(hasSamples(null), false)
  assert.equal(hasSamples([]), false)
  assert.equal(hasSamples(Float32Array.from([])), false)
  assert.equal(hasSamples(Float32Array.from([0])), true)
})

test('appendSamples：追加到队尾且不修改原队列', () => {
  const frameA = Float32Array.from([1])
  const frameB = Float32Array.from([2])
  const base = [frameA]
  const next = appendSamples(base, frameB)
  assert.equal(next.length, 2)
  assert.equal(next[0], frameA)
  assert.equal(next[1], frameB)
  assert.equal(base.length, 1)
})

test('appendSamples：队列缺省时也能从空队列开始', () => {
  const frame = Float32Array.from([1, 2])
  assert.deepEqual(appendSamples(undefined, frame), [frame])
})

test('takeSamplesFromQueue：单个分片刚好取完', () => {
  const queue = [Float32Array.from([1, 2, 3, 4])]
  const { samples, remaining } = takeSamplesFromQueue(queue, 4)
  assert.deepEqual([...samples], [1, 2, 3, 4])
  assert.equal(remaining.length, 0)
})

test('takeSamplesFromQueue：部分取出后剩余分片是 subarray 视图（保证切分正确）', () => {
  const queue = [Float32Array.from([1, 2, 3, 4])]
  const { samples, remaining } = takeSamplesFromQueue(queue, 2)
  assert.deepEqual([...samples], [1, 2])
  assert.equal(remaining.length, 1)
  assert.deepEqual([...remaining[0]], [3, 4])
  // 原队列不被修改
  assert.deepEqual([...queue[0]], [1, 2, 3, 4])
})

test('takeSamplesFromQueue：跨多个分片拼接', () => {
  const queue = [Float32Array.from([1, 2]), Float32Array.from([3, 4, 5])]
  const { samples, remaining } = takeSamplesFromQueue(queue, 4)
  assert.deepEqual([...samples], [1, 2, 3, 4])
  assert.equal(remaining.length, 1)
  assert.deepEqual([...remaining[0]], [5])
})

test('takeSamplesFromQueue：队列不足时尾部补 0（沿用原实现行为）', () => {
  const queue = [Float32Array.from([1, 2])]
  const { samples, remaining } = takeSamplesFromQueue(queue, 5)
  assert.deepEqual([...samples], [1, 2, 0, 0, 0])
  assert.equal(remaining.length, 0)
})

test('takeSamplesFromQueue：空队列返回全 0 帧', () => {
  const { samples, remaining } = takeSamplesFromQueue([], 3)
  assert.deepEqual([...samples], [0, 0, 0])
  assert.equal(remaining.length, 0)
})

test('takeSamplesFromQueue：队列中存在空分片不会死循环', () => {
  const queue = [Float32Array.from([]), Float32Array.from([7, 8])]
  const { samples, remaining } = takeSamplesFromQueue(queue, 2)
  assert.deepEqual([...samples], [7, 8])
  assert.equal(remaining.length, 0)
})

test('takeSamplesFromQueue：负数长度仍抛 RangeError（不静默吞掉非法入参）', () => {
  assert.throws(() => takeSamplesFromQueue([], -1), RangeError)
})

test('planFlushChunkSizes：不足一个分片时不发送', () => {
  assert.deepEqual(planFlushChunkSizes(0), [])
  assert.deepEqual(planFlushChunkSizes(3199), [])
  assert.deepEqual(planFlushChunkSizes(-3200), [])
})

test('planFlushChunkSizes：整数倍与零头场景', () => {
  assert.deepEqual(planFlushChunkSizes(3200), [3200])
  assert.deepEqual(planFlushChunkSizes(6400), [3200, 3200])
  assert.deepEqual(planFlushChunkSizes(5000), [3200])
  assert.deepEqual(planFlushChunkSizes(9600), [3200, 3200, 3200])
})

test('planFlushChunkSizes：支持自定义步长与非法步长', () => {
  assert.deepEqual(planFlushChunkSizes(25, 10), [10, 10])
  assert.deepEqual(planFlushChunkSizes(25, 0), [])
  assert.deepEqual(planFlushChunkSizes(25, Number.NaN), [])
  assert.deepEqual(planFlushChunkSizes(Number.NaN, 10), [])
})

test('planFlushChunkSizes：超长输入能规划出完整分片序列', () => {
  const sizes = planFlushChunkSizes(3200 * 50)
  assert.equal(sizes.length, 50)
  assert.equal(sizes.reduce((sum, size) => sum + size, 0), 160000)
})
