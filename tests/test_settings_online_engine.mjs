// 在线识别引擎卡片状态（src/components/settings/onlineEngineStatus.mjs）单元测试。
//
// 锁死的真实问题：填一个假 Token，卡片和首页都显示「已配置 / 已就绪」，要到录音时才失败。
import test from 'node:test'
import assert from 'node:assert/strict'

import {
  credentialNote,
  isOnlineProvider,
  needsCredentialCheck,
  onlineEngineChip
} from '../src/components/settings/onlineEngineStatus.mjs'

test('没保存凭据：未配置（警示色）', () => {
  assert.deepEqual(onlineEngineChip({ keySet: false }), { chip: '在线 · 未配置', tone: 'warn', loading: false })
  // 没凭据时旧结论不起作用
  assert.equal(onlineEngineChip({ keySet: false, check: { status: 'ok' } }).chip, '在线 · 未配置')
})

test('有凭据：按验证结论区分五种状态', () => {
  assert.equal(onlineEngineChip({ keySet: true }).chip, '在线 · 已配置')
  assert.deepEqual(onlineEngineChip({ keySet: true, check: { status: 'checking' } }), { chip: '在线 · 验证中', tone: 'accent', loading: true })
  assert.equal(onlineEngineChip({ keySet: true, check: { status: 'ok' } }).tone, 'ok')
  assert.deepEqual(onlineEngineChip({ keySet: true, check: { status: 'invalid' } }), { chip: '在线 · 验证失败', tone: 'err', loading: false })
  assert.equal(onlineEngineChip({ keySet: true, check: { status: 'unknown' } }).chip, '在线 · 未能验证')
})

test('提示条只在验证失败 / 没验成时出现', () => {
  assert.equal(credentialNote(null), null)
  assert.equal(credentialNote({ status: 'ok' }), null)
  assert.equal(credentialNote({ status: 'checking' }), null)
  assert.deepEqual(credentialNote({ status: 'invalid', message: '鉴权失败（401）' }), { tone: 'err', text: '鉴权失败（401）' })
  const unknown = credentialNote({ status: 'unknown', message: '连接豆包语音服务超时（10 秒）' })
  assert.equal(unknown.tone, 'warn')
  assert.match(unknown.text, /超时/)
  assert.match(unknown.text, /重新验证/)
})

test('保存后何时重新验证：换引擎、填新 Token、改 Resource ID / 接入点', () => {
  const saved = {
    asr_provider: 'doubao_asr',
    doubao_asr_resource_id: 'volc.seedasr.sauc.duration',
    doubao_asr_endpoint: 'wss://a',
    chatgpt_asr_endpoint: 'https://c'
  }
  const same = { ...saved }
  assert.equal(needsCredentialCheck(same, saved), false)
  assert.equal(needsCredentialCheck({ ...same, doubao_asr_api_key: 'new' }, saved), true)
  assert.equal(needsCredentialCheck({ ...same, doubao_asr_resource_id: 'volc.bigasr.sauc.duration' }, saved), true)
  assert.equal(needsCredentialCheck({ ...same, doubao_asr_endpoint: 'wss://b' }, saved), true)
  assert.equal(needsCredentialCheck({ ...same, asr_provider: 'chatgpt_asr' }, saved), true)
  assert.equal(needsCredentialCheck({ ...same, asr_provider: 'chatgpt_asr', chatgpt_asr_api_key: 't' }, { ...saved, asr_provider: 'chatgpt_asr' }), true)
  // 本地引擎不需要验证凭据
  assert.equal(needsCredentialCheck({ ...same, asr_provider: 'whisper' }, saved), false)
  assert.equal(isOnlineProvider('qwen_asr'), false)
})
