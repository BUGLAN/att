// 设置表单往返（服务端设置 → 表单 → 提交载荷，src/components/settings/settingsFormSync.mjs）单元测试。
//
// 锁死的真实踩坑：
// 1. normalizeSettings 漏写 sense_voice_device：表单里是 undefined，识别引擎页挂载时归一成 auto 并自动保存，
//    用户选的 GPU / CoreML 被悄悄改回「自动」；
// 2. 清空数字框得到 ''，原样提交被后端判成类型非法整单拒绝，之后在任何分页改任何设置都保存失败。
import test from 'node:test'
import assert from 'node:assert/strict'

import {
  NUMERIC_SETTINGS_DEFAULTS,
  TRANSIENT_CREDENTIAL_FIELDS,
  buildSettingsPayload,
  normalizeNumericSetting,
  normalizeSettingsForm
} from '../src/components/settings/settingsFormSync.mjs'

// 载荷字段由表单的另一个字段派生（热词文本框 → 词表数组）
const DERIVED_PAYLOAD_FIELDS = { asr_hotwords: 'asr_hotwords_text' }

const SERVER_SETTINGS = {
  asr_provider: 'sense_voice',
  asr_language: 'zh',
  asr_hotwords: ['落墨', 'Rust'],
  whisper_device: 'cuda',
  qwen_asr_device: 'cpu',
  sense_voice_device: 'cuda',
  sense_voice_num_threads: 4,
  qwen_asr_max_new_tokens: 512,
  qwen_asr_max_audio_seconds: 45,
  doubao_asr_end_window_size: 600,
  chatgpt_asr_timeout_seconds: 90,
  deepseek_asr_timeout_seconds: 75,
  wetype_asr_timeout_seconds: 30,
  wetype_asr_pid: 1234,
  silence_threshold: 2.5,
  silence_level: -35
}

test('往返：载荷的每个字段都由表单写出，数字字段都是有限数', () => {
  const form = normalizeSettingsForm(SERVER_SETTINGS)
  const payload = buildSettingsPayload(form, { saved: SERVER_SETTINGS })

  for (const field of Object.keys(payload)) {
    const source = DERIVED_PAYLOAD_FIELDS[field] || field
    assert.ok(Object.hasOwn(form, source), `载荷字段 ${field} 没有由 normalizeSettingsForm 写出`)
    assert.notEqual(form[source], undefined, `表单字段 ${source} 是 undefined`)
  }
  for (const field of Object.keys(NUMERIC_SETTINGS_DEFAULTS)) {
    assert.ok(Number.isFinite(payload[field]), `${field} 应是有限数：${payload[field]}`)
    assert.equal(payload[field], SERVER_SETTINGS[field], `${field} 往返后不变`)
  }
  assert.deepEqual(payload.asr_hotwords, ['落墨', 'Rust'])
  for (const field of TRANSIENT_CREDENTIAL_FIELDS) {
    assert.equal(Object.hasOwn(payload, field), false, `没输入时不提交 ${field}`)
  }
})

test('SenseVoice 运行设备回显并原样提交，缺字段时为 auto', () => {
  const form = normalizeSettingsForm(SERVER_SETTINGS)
  assert.equal(form.sense_voice_device, 'cuda')
  assert.equal(buildSettingsPayload(form).sense_voice_device, 'cuda')

  assert.equal(normalizeSettingsForm({}).sense_voice_device, 'auto')
})

test('数字框清空：回落到上次保存的值，再没有就用默认值', () => {
  const form = normalizeSettingsForm(SERVER_SETTINGS)
  form.deepseek_asr_timeout_seconds = ''
  form.silence_threshold = '   '
  form.silence_level = 'abc'
  form.qwen_asr_max_new_tokens = Number.NaN
  form.chatgpt_asr_timeout_seconds = null
  const payload = buildSettingsPayload(form, { saved: SERVER_SETTINGS })
  assert.equal(payload.deepseek_asr_timeout_seconds, 75)
  assert.equal(payload.silence_threshold, 2.5)
  assert.equal(payload.silence_level, -35)
  assert.equal(payload.qwen_asr_max_new_tokens, 512)
  assert.equal(payload.chatgpt_asr_timeout_seconds, 90)

  const withoutSaved = buildSettingsPayload(form)
  assert.equal(withoutSaved.deepseek_asr_timeout_seconds, NUMERIC_SETTINGS_DEFAULTS.deepseek_asr_timeout_seconds)
  assert.equal(withoutSaved.silence_level, NUMERIC_SETTINGS_DEFAULTS.silence_level)
})

test('数字框清空后载荷与已保存的一致：不会单独触发保存，改别的设置时照常保存', () => {
  const form = normalizeSettingsForm(SERVER_SETTINGS)
  const saved = JSON.stringify(buildSettingsPayload(form, { saved: SERVER_SETTINGS }))
  form.deepseek_asr_timeout_seconds = ''
  assert.equal(JSON.stringify(buildSettingsPayload(form, { saved: SERVER_SETTINGS })), saved)

  form.auto_optimize = true
  const next = buildSettingsPayload(form, { saved: SERVER_SETTINGS })
  assert.equal(next.auto_optimize, true)
  assert.equal(next.deepseek_asr_timeout_seconds, 75, '被清空的字段保留上次的值')
})

test('数字字段：数字字符串照常解析，0 与负数是合法值', () => {
  assert.equal(normalizeNumericSetting('silence_threshold', '3.5'), 3.5)
  assert.equal(normalizeNumericSetting('silence_level', -50), -50)
  assert.equal(normalizeNumericSetting('wetype_asr_pid', 0, { wetype_asr_pid: 99 }), 0)
  assert.equal(normalizeNumericSetting('silence_level', Infinity, { silence_level: -20 }), -20)
})

test('微信输入法超时为 0 时沿用旧行为回落默认值', () => {
  const form = normalizeSettingsForm({ wetype_asr_timeout_seconds: 0 })
  assert.equal(buildSettingsPayload(form).wetype_asr_timeout_seconds, 60)
})

test('明文凭据只在填写时提交，清除标记只在勾选时提交', () => {
  const form = normalizeSettingsForm({})
  form.deepseek_asr_api_key = 'sk-test'
  form.chatgpt_asr_api_key_clear = true
  const payload = buildSettingsPayload(form)
  assert.equal(payload.deepseek_asr_api_key, 'sk-test')
  assert.equal(payload.chatgpt_asr_api_key_clear, true)
  assert.equal(Object.hasOwn(payload, 'doubao_asr_api_key'), false)
  assert.equal(Object.hasOwn(payload, 'deepseek_asr_api_key_clear'), false)
})

test('快捷键按字段交给调用方归一', () => {
  const seen = []
  const normalizeHotkey = (field, hotkey) => {
    seen.push(field)
    return hotkey ?? { field }
  }
  const form = normalizeSettingsForm({}, { normalizeHotkey })
  assert.deepEqual(form.rewrite_hotkey, { field: 'rewrite_hotkey' })
  buildSettingsPayload(form, { normalizeHotkey })
  for (const field of ['hotkey', 'record_toggle_hotkey', 'show_main_window_hotkey', 'clipboard_panel_hotkey',
    'rewrite_hotkey', 'voice_rewrite_hotkey', 'cycle_preset_hotkey']) {
    assert.equal(seen.filter((item) => item === field).length, 2, `${field} 表单、载荷各归一一次`)
  }
})
