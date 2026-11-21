// 本地模型管理纯逻辑（src/components/settings/modelManager.mjs）单元测试。
// 覆盖：体积文案、安装状态、按引擎过滤、下载进度与取消、VAD 提示。
import test from 'node:test'
import assert from 'node:assert/strict'

import {
  downloadLabel,
  formatBytes,
  installedModelSelection,
  isJobActive,
  jobView,
  modelStatusText,
  modelsForProvider,
  normalizeDownloadSource,
  vadWarning,
  whisperOptionLabel
} from '../src/components/settings/modelManager.mjs'

const MODELS = [
  {
    id: 'whisper-small',
    engine: 'whisper',
    title: 'Whisper small',
    select_value: 'small',
    status: 'installed',
    installed_bytes: 483_546_902,
    download_size: 486_000_000
  },
  {
    id: 'whisper-large-v3-turbo',
    engine: 'whisper',
    title: 'Whisper large-v3-turbo',
    select_value: 'large-v3-turbo',
    status: 'missing',
    installed_bytes: 0,
    download_size: 1_617_884_929
  },
  {
    id: 'qwen3-asr-0.6b-int8',
    engine: 'qwen_asr',
    title: 'Qwen3-ASR 0.6B int8',
    select_value: 'qwen3-asr-0.6B-int8',
    status: 'missing',
    installed_bytes: 0,
    download_size: 878_702_423
  },
  {
    id: 'silero-vad',
    engine: 'vad',
    title: 'Silero VAD',
    status: 'missing',
    installed_bytes: 0,
    download_size: 643_854
  }
]

test('体积文案：二进制单位、0 与小数位', () => {
  assert.equal(formatBytes(0), '0 B')
  assert.equal(formatBytes(512), '512 B')
  assert.equal(formatBytes(1024), '1.0 KB')
  assert.equal(formatBytes(483_546_902), '461.1 MB')
  assert.equal(formatBytes(1_617_884_929), '1.5 GB')
  assert.equal(formatBytes(-1), '0 B')
  assert.equal(formatBytes('nope'), '0 B')
})

test('模型状态与下载按钮：已安装 / 部分 / 未安装都带体积', () => {
  assert.equal(modelStatusText(MODELS[0]), '已安装 · 461.1 MB')
  assert.equal(modelStatusText({ ...MODELS[1], status: 'partial', installed_bytes: 100_000_000 }), '安装不完整 · 95.4 MB / 1.5 GB')
  assert.equal(modelStatusText(MODELS[1]), '未安装 · 需下载 1.5 GB')
  assert.equal(downloadLabel(MODELS[1]), '下载 1.5 GB')
  assert.equal(downloadLabel({ ...MODELS[1], status: 'partial' }), '继续下载（共 1.5 GB）')
})

test('按引擎过滤：Whisper 带 VAD，Qwen 不带', () => {
  assert.deepEqual(modelsForProvider(MODELS, 'whisper').map((model) => model.id), [
    'whisper-small',
    'whisper-large-v3-turbo',
    'silero-vad'
  ])
  assert.deepEqual(modelsForProvider(MODELS, 'qwen_asr').map((model) => model.id), ['qwen3-asr-0.6b-int8'])
  assert.deepEqual(modelsForProvider(MODELS, 'doubao_asr'), [])
  assert.deepEqual(modelsForProvider(null, 'whisper'), [])
})

test('Whisper 下拉文案标出已安装与下载体积', () => {
  assert.equal(whisperOptionLabel('small', MODELS), 'small · 已安装')
  assert.equal(whisperOptionLabel('large-v3-turbo', MODELS), 'large-v3-turbo · 未安装（1.5 GB）')
  assert.equal(whisperOptionLabel('unknown', MODELS), 'unknown')
})

test('下载任务：进度、速度、当前文件、取消与错误', () => {
  const downloading = {
    id: 'job-1',
    model_id: 'whisper-small',
    status: 'downloading',
    received: 50,
    total: 100,
    speed_bps: 1024 * 1024,
    current_file: 'faster-whisper-small/model.bin',
    error: null
  }
  assert.equal(isJobActive(downloading), true)
  const view = jobView(downloading)
  assert.equal(view.active, true)
  assert.equal(view.canCancel, true)
  assert.equal(view.percent, 50)
  assert.match(view.label, /正在下载 · 50 B \/ 100 B/)
  assert.equal(view.speed, '1.0 MB/s')
  assert.equal(view.file, 'model.bin')

  const cancelling = jobView({ ...downloading, status: 'cancelling' })
  assert.equal(cancelling.canCancel, false, '已经在取消时不再重复发请求')

  const extracting = jobView({ ...downloading, status: 'extracting', total: 100 })
  assert.equal(extracting.label, '正在解压')
  assert.equal(extracting.percent, 50)
  assert.equal(extracting.canCancel, false, '进入解压后不再提供取消，避免安装到一半')

  const failed = jobView({ ...downloading, status: 'error', error: '网络中断' })
  assert.equal(failed.active, false)
  assert.match(failed.label, /下载失败：网络中断/)

  assert.deepEqual(jobView(null), { active: false, canCancel: false, percent: 0, label: '', speed: '', file: '', error: '' })
})

test('VAD 提示：只对 Whisper 且未安装时出现', () => {
  assert.match(vadWarning('whisper', MODELS), /未安装 VAD/)
  assert.equal(vadWarning('qwen_asr', MODELS), '')
  assert.equal(vadWarning('whisper', [{ ...MODELS[3], status: 'installed' }]), '')
  assert.equal(vadWarning('whisper', []), '')
})

test('下载源归一化：只接受镜像，其余回落官方源', () => {
  assert.equal(normalizeDownloadSource('mirror'), 'mirror')
  assert.equal(normalizeDownloadSource('official'), 'official')
  assert.equal(normalizeDownloadSource('ftp'), 'official')
  assert.equal(normalizeDownloadSource(undefined), 'official')
})

test('下载完成：仅自动选择已安装且带配置键的模型', () => {
  const job = { status: 'done', model_id: 'whisper-small' }
  assert.equal(installedModelSelection(job, MODELS), null)
  const configured = MODELS.map((model) => model.id === 'whisper-small'
    ? { ...model, config_key: 'whisper_model', select_value: 'small' }
    : model)
  assert.deepEqual(installedModelSelection(job, configured), { key: 'whisper_model', value: 'small' })
  assert.equal(installedModelSelection({ ...job, status: 'error' }, configured), null)
  assert.equal(installedModelSelection(job, [{ ...configured[0], status: 'missing' }]), null)
})
