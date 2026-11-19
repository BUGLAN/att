// 设置表单与服务端设置的同步判定（src/components/settings/settingsFormSync.mjs）单元测试。
//
// 锁死的真实踩坑：用户改完表单、500ms 防抖保存还没发出时，只要 store 的 settings
// 有任何变更（例如 loadPresets 写入 default_preset_id），深监听就会用服务端值重建
// 表单——改动被静默回滚，且待保存内容与快照一致后被判成「无变化」直接取消保存。
import test from 'node:test'
import assert from 'node:assert/strict'

import {
  SERVER_OWNED_FIELDS,
  TRANSIENT_CREDENTIAL_FIELDS,
  hasPendingSettingsEdits,
  serializeSavedSettingsPayload,
  serverOwnedPatch,
  submittedCredentialClearPatch
} from '../src/components/settings/settingsFormSync.mjs'

test('初始化完成前一律允许对齐（表单尚未与服务端对齐，快照无意义）', () => {
  assert.equal(
    hasPendingSettingsEdits({ initialized: false, draftSnapshot: 'draft', savedSnapshot: 'saved' }),
    false
  )
})

test('快照为空（基线未建立）时允许对齐', () => {
  assert.equal(
    hasPendingSettingsEdits({ initialized: true, draftSnapshot: 'draft', savedSnapshot: '' }),
    false
  )
})

test('表单与快照一致时允许对齐', () => {
  assert.equal(
    hasPendingSettingsEdits({ initialized: true, draftSnapshot: 'same', savedSnapshot: 'same' }),
    false
  )
})

test('存在未提交改动时必须禁止用服务端值覆盖表单', () => {
  assert.equal(
    hasPendingSettingsEdits({ initialized: true, draftSnapshot: 'dirty', savedSnapshot: 'saved' }),
    true
  )
})

// 锁死的真实踩坑：保存 Token 后后端已返回 *_api_key_set: true，但当时表单里还留着明文 Token，
// watch(settings) 判成「有未提交改动」跳过重建，卡片一直显示「未配置」。
// 这两个字段不进快照，必须无条件跟随服务端。
test('密钥「是否已保存」只由服务端维护：两个字段都要取到', () => {
  assert.deepEqual(
    [...SERVER_OWNED_FIELDS].sort(),
    ['chatgpt_asr_api_key_set', 'deepseek_asr_api_key_set', 'doubao_asr_api_key_set']
  )
})

test('serverOwnedPatch 跟随服务端值（保存 / 清除 Token 两个方向）', () => {
  assert.deepEqual(
    serverOwnedPatch({ doubao_asr_api_key_set: true, chatgpt_asr_api_key_set: false, deepseek_asr_api_key_set: false, asr_provider: 'doubao_asr' }),
    { doubao_asr_api_key_set: true, chatgpt_asr_api_key_set: false, deepseek_asr_api_key_set: false }
  )
  assert.deepEqual(
    serverOwnedPatch({ doubao_asr_api_key_set: false, chatgpt_asr_api_key_set: true, deepseek_asr_api_key_set: true }),
    { doubao_asr_api_key_set: false, chatgpt_asr_api_key_set: true, deepseek_asr_api_key_set: true }
  )
})

test('serverOwnedPatch 缺字段 / 空值按「未保存」处理，且只返回这几个字段', () => {
  assert.deepEqual(serverOwnedPatch({}), {
    doubao_asr_api_key_set: false,
    chatgpt_asr_api_key_set: false,
    deepseek_asr_api_key_set: false
  })
  assert.deepEqual(serverOwnedPatch(null), {
    doubao_asr_api_key_set: false,
    chatgpt_asr_api_key_set: false,
    deepseek_asr_api_key_set: false
  })
  assert.deepEqual(Object.keys(serverOwnedPatch({ hotkey: {}, doubao_asr_api_key_set: true })).sort(), [...SERVER_OWNED_FIELDS].sort())
})

test('保存快照剔除明文凭据，保留其它设置字段', () => {
  const payload = {
    asr_provider: 'deepseek_asr',
    deepseek_asr_api_key: 'sk-draft',
    deepseek_asr_api_key_clear: true,
    auto_optimize: true
  }
  assert.deepEqual(JSON.parse(serializeSavedSettingsPayload(payload)), {
    asr_provider: 'deepseek_asr',
    auto_optimize: true
  })
  assert.equal(TRANSIENT_CREDENTIAL_FIELDS.length, 6)
})

test('保存返回只清本次提交且未被用户改写的凭据字段', () => {
  const submitted = {
    doubao_asr_api_key: 'sk-old',
    doubao_asr_api_key_clear: false,
    chatgpt_asr_api_key: '',
    chatgpt_asr_api_key_clear: false,
    deepseek_asr_api_key: 'sk-ds',
    deepseek_asr_api_key_clear: false
  }
  const current = {
    ...submitted,
    doubao_asr_api_key: 'sk-new',
    deepseek_asr_api_key_clear: true
  }

  assert.deepEqual(submittedCredentialClearPatch(current, submitted), {
    doubao_asr_api_key_clear: false,
    chatgpt_asr_api_key: '',
    chatgpt_asr_api_key_clear: false,
    deepseek_asr_api_key: ''
  })
})
