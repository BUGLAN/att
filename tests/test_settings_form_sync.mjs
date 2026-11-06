// 设置表单与服务端设置的同步判定（src/components/settings/settingsFormSync.mjs）单元测试。
//
// 锁死的真实踩坑：用户改完表单、500ms 防抖保存还没发出时，只要 store 的 settings
// 有任何变更（例如 loadPresets 写入 default_preset_id），深监听就会用服务端值重建
// 表单——改动被静默回滚，且待保存内容与快照一致后被判成「无变化」直接取消保存。
import test from 'node:test'
import assert from 'node:assert/strict'

import { SERVER_OWNED_FIELDS, hasPendingSettingsEdits, serverOwnedPatch } from '../src/components/settings/settingsFormSync.mjs'

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
  assert.deepEqual([...SERVER_OWNED_FIELDS].sort(), ['chatgpt_asr_api_key_set', 'doubao_asr_api_key_set'])
})

test('serverOwnedPatch 跟随服务端值（保存 / 清除 Token 两个方向）', () => {
  assert.deepEqual(
    serverOwnedPatch({ doubao_asr_api_key_set: true, chatgpt_asr_api_key_set: false, asr_provider: 'doubao_asr' }),
    { doubao_asr_api_key_set: true, chatgpt_asr_api_key_set: false }
  )
  assert.deepEqual(
    serverOwnedPatch({ doubao_asr_api_key_set: false, chatgpt_asr_api_key_set: true }),
    { doubao_asr_api_key_set: false, chatgpt_asr_api_key_set: true }
  )
})

test('serverOwnedPatch 缺字段 / 空值按「未保存」处理，且只返回这两个字段', () => {
  assert.deepEqual(serverOwnedPatch({}), { doubao_asr_api_key_set: false, chatgpt_asr_api_key_set: false })
  assert.deepEqual(serverOwnedPatch(null), { doubao_asr_api_key_set: false, chatgpt_asr_api_key_set: false })
  assert.deepEqual(Object.keys(serverOwnedPatch({ hotkey: {}, doubao_asr_api_key_set: true })).sort(), [...SERVER_OWNED_FIELDS].sort())
})
