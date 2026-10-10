// 录音链路的采样纯计算工具：音量计算、Float32 -> PCM16 转换、采样队列累积与切分。
//
// 本模块只做纯计算，不依赖 Vue / 浏览器全局对象（不触碰 AudioContext / WebSocket），
// 因此可被 Node 测试直接 import。
//
// 对照原 useRecorder.js 中的内联实现：
//   calculateAudioLevel / convertFloat32ToPcm16Buffer / takeStreamingSamples /
//   queueStreamingSamples / sendStreamingSamples 中的纯计算部分。
// 抽出的函数保持与原实现逐字一致的数值语义（含 count 超出队列长度时用 0 填充的旧行为）。

// 流式上行每累积满 3200 个采样点发送一次（16kHz 下约 200ms），与原实现常量一致
export const STREAM_FLUSH_SAMPLES = 3200

// PCM16 满量程系数：负半轴 -32768，正半轴 32767
const PCM16_NEGATIVE_SCALE = 0x8000
const PCM16_POSITIVE_SCALE = 0x7fff

/**
 * 判断采样帧是否含有效数据。
 * 等价于原实现里的 `!samples?.length` 取反，空数组 / undefined / null 都视为无数据。
 */
export function hasSamples(samples) {
  return Boolean(samples?.length)
}

/**
 * 计算归一化音量：RMS * 6 后钳制到 [0, 1]。空输入返回 0。
 * 与原 calculateAudioLevel 完全一致。
 */
export function calculateAudioLevel(samples) {
  if (!samples?.length) {
    return 0
  }

  let sumSquares = 0
  for (let index = 0; index < samples.length; index += 1) {
    sumSquares += samples[index] * samples[index]
  }

  const rms = Math.sqrt(sumSquares / samples.length)
  return Math.max(0, Math.min(1, rms * 6))
}

/**
 * Float32 采样 -> PCM16 ArrayBuffer。
 * 采样先钳制到 [-1, 1]，负值乘 0x8000、非负乘 0x7fff，与原实现一致。
 * 非 Float32 的类数组（如 Int8Array / 普通数组）同样可用。
 */
export function convertFloat32ToPcm16Buffer(samples) {
  const pcm16 = new Int16Array(samples.length)
  for (let index = 0; index < samples.length; index += 1) {
    const sample = Math.max(-1, Math.min(1, samples[index]))
    pcm16[index] = sample < 0 ? sample * PCM16_NEGATIVE_SCALE : sample * PCM16_POSITIVE_SCALE
  }
  return pcm16.buffer
}

/**
 * 把新的采样帧追加到待发送队列尾部，返回新队列（不修改入参）。
 * 对应原实现里的 `streamingPendingSamples.push(samples)`。
 */
export function appendSamples(pendingSamples, samples) {
  const queue = Array.isArray(pendingSamples) ? pendingSamples : []
  return [...queue, samples]
}

/**
 * 从待发送队列头部取出 count 个采样点，返回 { samples, remaining }。
 * - samples：长度恒为 count 的 Float32Array；队列不足时尾部保持 0（沿用原实现行为）
 * - remaining：取完之后剩余的分片队列（原实现是原地 shift / subarray，这里改为返回新数组，
 *   分片本身仍是 subarray 视图，不额外拷贝采样数据）
 *
 * 注意：调用方仍需按原逻辑自行扣减 `streamingPendingSampleCount -= count`，
 * 因为原实现在队列不足时计数器会变成负数，这个历史行为不在本次拆分中修改。
 */
export function takeSamplesFromQueue(pendingSamples, count) {
  const queue = Array.isArray(pendingSamples) ? pendingSamples.slice() : []
  const result = new Float32Array(count)
  let offset = 0

  while (offset < count && queue.length > 0) {
    const current = queue[0]
    const remaining = count - offset

    if (current.length <= remaining) {
      result.set(current, offset)
      offset += current.length
      queue.shift()
      continue
    }

    result.set(current.subarray(0, remaining), offset)
    queue[0] = current.subarray(remaining)
    offset += remaining
  }

  return { samples: result, remaining: queue }
}

/**
 * 规划非强制刷新的分片尺寸列表。
 * 等价于原实现里的 `while (streamingPendingSampleCount >= STREAM_FLUSH_SAMPLES)` 循环：
 * 每次取满一个 flushSamples 的分片，直到剩余不足一个分片为止。
 */
export function planFlushChunkSizes(sampleCount, flushSamples = STREAM_FLUSH_SAMPLES) {
  const step = Number.isFinite(flushSamples) ? Math.floor(flushSamples) : 0
  const total = Number.isFinite(sampleCount) ? Math.floor(sampleCount) : 0
  const sizes = []

  if (step <= 0) {
    return sizes
  }

  for (let remaining = total; remaining >= step; remaining -= step) {
    sizes.push(step)
  }

  return sizes
}
