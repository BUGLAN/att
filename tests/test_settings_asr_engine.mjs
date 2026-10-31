// 识别引擎页纯逻辑（src/components/settings/asrEngineStatus.mjs）单元测试。
// 覆盖：流式只对豆包可用、模式冲突提示、本地引擎加载状态的各个分支。
import test from 'node:test'
import assert from 'node:assert/strict'

import {
  localEngineView,
  recordingModeConflict,
  recordingModeOptions,
  splitHint
} from '../src/components/settings/asrEngineStatus.mjs'

test('录音模式：流式只有豆包可选，其余引擎置灰并说明原因', () => {
  const doubao = recordingModeOptions('doubao_asr')
  assert.deepEqual(doubao.map((mode) => mode.value), ['single', 'multi', 'streaming'])
  assert.ok(doubao.every((mode) => !mode.disabled))

  for (const provider of ['whisper', 'qwen_asr', 'chatgpt_asr']) {
    const streaming = recordingModeOptions(provider).find((mode) => mode.value === 'streaming')
    assert.equal(streaming.disabled, true, provider)
    assert.match(streaming.hint, /不支持流式/)
    assert.ok(recordingModeOptions(provider).filter((mode) => mode.value !== 'streaming').every((mode) => !mode.disabled))
  }
})

test('模式冲突：非豆包 + 流式给出提示，其它组合无提示', () => {
  assert.match(recordingModeConflict('whisper', 'streaming'), /faster-whisper不支持流式/)
  assert.equal(recordingModeConflict('doubao_asr', 'streaming'), '')
  assert.equal(recordingModeConflict('whisper', 'single'), '')
})

test('本地引擎状态：在线引擎不展示', () => {
  assert.equal(localEngineView('doubao_asr', null).state, 'none')
})

test('本地引擎状态：后端还没切过来或正在加载 → loading', () => {
  assert.equal(localEngineView('whisper', null).state, 'loading')
  assert.equal(localEngineView('whisper', { provider: 'qwen_asr', available: true, loaded: true }).state, 'loading')
  const view = localEngineView('whisper', { provider: 'whisper', available: true, loaded: true, loading: true })
  assert.equal(view.state, 'loading', '预热未完成时即便已加载也算加载中')
  assert.equal(view.chip, '加载中')
})

test('本地引擎状态：缺模型 / 加载失败 / 已就绪 / 未加载', () => {
  const missing = localEngineView('qwen_asr', { provider: 'qwen_asr', available: false, error: '未找到本地 Qwen3-ASR 模型' })
  assert.equal(missing.state, 'missing')
  assert.equal(missing.message, '未找到本地 Qwen3-ASR 模型')

  const failed = localEngineView('whisper', { provider: 'whisper', available: true, load_error: '加载本地 Whisper 模型失败' })
  assert.equal(failed.state, 'failed')
  assert.equal(failed.tone, 'err')
  assert.match(failed.tip, /点一下上方的引擎卡片/)

  const ready = localEngineView('whisper', { provider: 'whisper', available: true, loaded: true, device: 'cpu:int8', model_dir: '/m' })
  assert.equal(ready.state, 'ready')
  assert.equal(ready.message, 'cpu:int8 · /m')
  assert.equal(ready.title, '模型已就绪')

  const idle = localEngineView('whisper', { provider: 'whisper', available: true, loaded: false })
  assert.equal(idle.state, 'idle')
})

test('提示拆分：首行结论、缩进行是命令、其余是说明', () => {
  const hint = splitHint('未找到模型：请下载\n\twget https://x/a.tar.bz2\n\ttar xvf a.tar.bz2 -C /m\n目录内需要 a.onnx')
  assert.equal(hint.summary, '未找到模型：请下载')
  assert.deepEqual(hint.commands, ['wget https://x/a.tar.bz2', 'tar xvf a.tar.bz2 -C /m'])
  assert.deepEqual(hint.notes, ['目录内需要 a.onnx'])
  assert.deepEqual(splitHint(''), { summary: '', commands: [], notes: [] })
})
