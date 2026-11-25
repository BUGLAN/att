// 本地模型「运行设备」选项纯逻辑（src/components/settings/deviceOptions.mjs）单测。
import test from 'node:test'
import assert from 'node:assert/strict'

import {
  deviceHintFor,
  deviceOptionsFor,
  isMacPlatform,
  normalizeDeviceValue
} from '../src/components/settings/deviceOptions.mjs'

test('isMacPlatform 只认 darwin', () => {
  assert.equal(isMacPlatform('darwin'), true)
  assert.equal(isMacPlatform('win32'), false)
  assert.equal(isMacPlatform('linux'), false)
  assert.equal(isMacPlatform(undefined), false)
})

test('faster-whisper：Windows/Linux 有 GPU(CUDA)，macOS 只有自动/CPU', () => {
  assert.deepEqual(deviceOptionsFor('whisper', 'win32').map((o) => o.value), ['auto', 'cpu', 'cuda'])
  assert.deepEqual(deviceOptionsFor('whisper', 'linux').map((o) => o.value), ['auto', 'cpu', 'cuda'])
  assert.deepEqual(deviceOptionsFor('whisper', 'darwin').map((o) => o.value), ['auto', 'cpu'])
})

test('Qwen3-ASR / SenseVoice：macOS 用 CoreML，Windows/Linux 用 CUDA', () => {
  for (const kind of ['qwen_asr', 'sense_voice']) {
    assert.deepEqual(deviceOptionsFor(kind, 'darwin').map((o) => o.value), ['auto', 'cpu', 'coreml'])
    assert.deepEqual(deviceOptionsFor(kind, 'win32').map((o) => o.value), ['auto', 'cpu', 'cuda'])
    assert.equal(deviceOptionsFor(kind, 'darwin').find((o) => o.value === 'coreml').label, 'CoreML（Apple GPU）')
  }
})

test('提示文案区分平台与引擎', () => {
  assert.match(deviceHintFor('whisper', 'darwin'), /Accelerate/)
  assert.match(deviceHintFor('whisper', 'win32'), /CUDA 版构建/)
  assert.match(deviceHintFor('qwen_asr', 'darwin'), /CoreML/)
  assert.match(deviceHintFor('sense_voice', 'win32'), /GPU/)
})

test('归一设备值：别名映射 + 平台不支持回落 auto', () => {
  assert.equal(normalizeDeviceValue('whisper', 'win32', 'GPU'), 'cuda')
  assert.equal(normalizeDeviceValue('qwen_asr', 'darwin', 'metal'), 'coreml')
  assert.equal(normalizeDeviceValue('qwen_asr', 'darwin', 'apple'), 'coreml')
  assert.equal(normalizeDeviceValue('whisper', 'darwin', 'cuda'), 'auto')
  assert.equal(normalizeDeviceValue('qwen_asr', 'win32', 'coreml'), 'auto')
  assert.equal(normalizeDeviceValue('whisper', 'win32', ''), 'auto')
  assert.equal(normalizeDeviceValue('whisper', 'win32', 'cpu'), 'cpu')
})
