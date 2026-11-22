// 识别引擎页纯逻辑（src/components/settings/asrEngineStatus.mjs）单元测试。
// 覆盖：流式对豆包与 Windows 微信输入法可用、模式冲突提示、本地引擎加载状态的各个分支。
import test from 'node:test'
import assert from 'node:assert/strict'

import {
  hintBlocks,
  hintSummary,
  localEngineView,
  normalizeDoubaoWebTimeoutSeconds,
  normalizeLocalModelIdleMinutes,
  recordingModeConflict,
  recordingModeOptions,
  providerLabel,
  splitHint,
  supportsStreaming
} from '../src/components/settings/asrEngineStatus.mjs'

test('本地模型空闲释放：缺省 15 分钟，0 关闭，无效值兜底', () => {
  assert.equal(normalizeLocalModelIdleMinutes(undefined), 15)
  assert.equal(normalizeLocalModelIdleMinutes(''), 15)
  assert.equal(normalizeLocalModelIdleMinutes('not-a-number'), 15)
  assert.equal(normalizeLocalModelIdleMinutes(0), 0)
  assert.equal(normalizeLocalModelIdleMinutes('2.6'), 3)
  assert.equal(normalizeLocalModelIdleMinutes(-3), 0)
})

test('豆包网页语音：展示名独立，超时缺省 30 秒并限制在 10–300', () => {
  assert.equal(providerLabel('doubao_asr'), '豆包语音')
  assert.equal(providerLabel('doubao_web_asr'), '豆包网页语音')
  assert.equal(normalizeDoubaoWebTimeoutSeconds(undefined), 30)
  assert.equal(normalizeDoubaoWebTimeoutSeconds(''), 30)
  assert.equal(normalizeDoubaoWebTimeoutSeconds('not-a-number'), 30)
  assert.equal(normalizeDoubaoWebTimeoutSeconds(30), 30)
  assert.equal(normalizeDoubaoWebTimeoutSeconds('45.4'), 45)
  assert.equal(normalizeDoubaoWebTimeoutSeconds(1), 10)
  assert.equal(normalizeDoubaoWebTimeoutSeconds(999), 300)
})

test('录音模式：流式支持豆包与 Windows 微信输入法，其余引擎置灰并说明原因', () => {
  const doubao = recordingModeOptions('doubao_asr', 'darwin')
  assert.deepEqual(doubao.map((mode) => mode.value), ['single', 'multi', 'streaming'])
  assert.ok(doubao.every((mode) => !mode.disabled))

  const wetypeWindows = recordingModeOptions('wetype_asr', 'win32')
  assert.equal(wetypeWindows.find((mode) => mode.value === 'streaming').disabled, false)
  assert.equal(supportsStreaming('wetype_asr', 'win32'), true)
  const wetypeMac = recordingModeOptions('wetype_asr', 'darwin')
  const streaming = wetypeMac.find((mode) => mode.value === 'streaming')
  assert.equal(streaming.disabled, true)
  assert.match(streaming.hint, /仅支持 Windows/)
  assert.equal(supportsStreaming('wetype_asr', 'darwin'), false)
  // health/runtime 的平台字段是权威结论：前端 UA 推断不准时仍能放开流式
  assert.equal(supportsStreaming('wetype_asr', 'darwin', true), true)
  assert.equal(recordingModeOptions('wetype_asr', 'darwin', true).find((mode) => mode.value === 'streaming').disabled, false)

  for (const provider of ['whisper', 'qwen_asr', 'chatgpt_asr', 'deepseek_asr', 'doubao_web_asr']) {
    const streaming = recordingModeOptions(provider, 'win32').find((mode) => mode.value === 'streaming')
    assert.equal(streaming.disabled, true, provider)
    assert.match(streaming.hint, /不支持流式/)
    assert.ok(recordingModeOptions(provider, 'win32').filter((mode) => mode.value !== 'streaming').every((mode) => !mode.disabled))
  }
})

test('模式冲突：不支持流式的引擎与平台给出提示，其它组合无提示', () => {
  assert.match(recordingModeConflict('whisper', 'streaming', 'win32'), /faster-whisper不支持流式/)
  assert.match(recordingModeConflict('doubao_web_asr', 'streaming', 'win32'), /豆包网页语音不支持流式/)
  assert.match(recordingModeConflict('wetype_asr', 'streaming', 'darwin'), /仅支持 Windows/)
  assert.equal(recordingModeConflict('wetype_asr', 'streaming', 'darwin', true), '')
  assert.equal(recordingModeConflict('wetype_asr', 'streaming', 'win32'), '')
  assert.equal(recordingModeConflict('doubao_asr', 'streaming', 'darwin'), '')
  assert.equal(recordingModeConflict('whisper', 'single', 'win32'), '')
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
  const missing = localEngineView('qwen_asr', {
    provider: 'qwen_asr',
    available: false,
    error: '本地 Qwen3-ASR 模型未安装',
    error_detail: '用下面的命令下载：\n\twget https://x/a.tar.bz2'
  })
  assert.equal(missing.state, 'missing')
  assert.equal(missing.message, '本地 Qwen3-ASR 模型未安装', '结论只有一句')
  assert.equal(missing.detail, '用下面的命令下载：\n\twget https://x/a.tar.bz2', '命令进折叠详情')
  assert.equal(missing.detailLabel, '安装位置')

  const failed = localEngineView('whisper', { provider: 'whisper', available: true, load_error: '加载本地 Whisper 模型失败：x' })
  assert.equal(failed.state, 'failed')
  assert.equal(failed.tone, 'err')
  assert.equal(failed.detail, '加载本地 Whisper 模型失败：x', '完整原因折叠展示')
  assert.ok(!failed.message.includes('\n'))
  assert.match(failed.tip, /点一下上方的引擎卡片/)

  const ready = localEngineView('whisper', { provider: 'whisper', available: true, loaded: true, device: 'cpu:int8', model_dir: '/m' })
  assert.equal(ready.state, 'ready')
  assert.equal(ready.message, 'cpu:int8 · /m')
  assert.equal(ready.title, '模型已就绪')
  assert.equal(ready.detail, '')

  const idle = localEngineView('whisper', { provider: 'whisper', available: true, loaded: false })
  assert.equal(idle.state, 'idle')
})

test('本地引擎状态：旧格式多行 error 拆成结论 + 详情，缺省时给通用结论', () => {
  const legacy = localEngineView('qwen_asr', {
    provider: 'qwen_asr',
    available: false,
    error: '未找到模型：请下载\n\twget https://x/a.tar.bz2'
  })
  assert.equal(legacy.message, '未找到模型：请下载')
  assert.equal(legacy.detail, '\twget https://x/a.tar.bz2')

  const bare = localEngineView('whisper', { provider: 'whisper', available: false })
  assert.equal(bare.message, 'faster-whisper 模型未安装')
  assert.equal(bare.detail, '')
})

test('本地引擎状态：Whisper 缺 VAD 时提示幻觉风险，Qwen 不提示', () => {
  const idle = localEngineView('whisper', { provider: 'whisper', available: true, loaded: false, vad_ready: false })
  assert.match(idle.tip, /未安装 VAD/)
  assert.match(idle.tip, /幻觉/)
  const ready = localEngineView('whisper', { provider: 'whisper', available: true, loaded: true, vad_ready: true })
  assert.equal(ready.tip.includes('未安装 VAD'), false)
  const qwen = localEngineView('qwen_asr', { provider: 'qwen_asr', available: true, loaded: false, vad_ready: false })
  assert.equal(qwen.tip.includes('未安装 VAD'), false)
})

test('提示拆分：首行结论、缩进行是命令、其余是说明', () => {
  const hint = splitHint('未找到模型：请下载\n\twget https://x/a.tar.bz2\n\ttar xvf a.tar.bz2 -C /m\n目录内需要 a.onnx')
  assert.equal(hint.summary, '未找到模型：请下载')
  assert.deepEqual(hint.commands, ['wget https://x/a.tar.bz2', 'tar xvf a.tar.bz2 -C /m'])
  assert.deepEqual(hint.notes, ['目录内需要 a.onnx'])
  assert.deepEqual(splitHint(''), { summary: '', commands: [], notes: [] })
})

test('详情块：保持说明与命令的先后顺序，连续命令合成一块', () => {
  assert.deepEqual(
    hintBlocks('下载后放到这个目录：\n\t/m/faster_whisper\n目录内至少要有 model.bin\n\n\ta\n\tb'),
    [
      { type: 'note', text: '下载后放到这个目录：' },
      { type: 'code', text: '/m/faster_whisper' },
      { type: 'note', text: '目录内至少要有 model.bin' },
      { type: 'code', text: 'a\nb' }
    ]
  )
  assert.deepEqual(hintBlocks(''), [])
  assert.deepEqual(hintBlocks(undefined), [])
})

test('一句结论：多行文案只留首行', () => {
  assert.equal(hintSummary('本地 Whisper 模型未安装\n下载……\n\t/m'), '本地 Whisper 模型未安装')
  assert.equal(hintSummary('  只有一句  '), '只有一句')
  assert.equal(hintSummary(null), '')
})
