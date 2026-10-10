// 设置表单与服务端设置的同步判定（src/components/settings/settingsFormSync.mjs）单元测试。
//
// 锁死的真实踩坑：用户改完表单、500ms 防抖保存还没发出时，只要 store 的 settings
// 有任何变更（例如 loadPresets 写入 default_preset_id），深监听就会用服务端值重建
// 表单——改动被静默回滚，且待保存内容与快照一致后被判成「无变化」直接取消保存。
import test from 'node:test'
import assert from 'node:assert/strict'

import { hasPendingSettingsEdits } from '../src/components/settings/settingsFormSync.mjs'

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
