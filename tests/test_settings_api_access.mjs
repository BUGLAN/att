// 设置 → 诊断 →「外部 API」纯逻辑单测（src/components/settings/apiAccess.mjs）。
import test from 'node:test'
import assert from 'node:assert/strict'

import { apiTokenDisplay } from '../src/components/settings/apiAccess.mjs'

test('令牌显示：按开关 / 是否生成 / 是否点开切换', () => {
  assert.equal(apiTokenDisplay({ enabled: false, hasToken: false, revealedToken: '' }), '未开启')
  assert.equal(apiTokenDisplay({ enabled: true, hasToken: false, revealedToken: '' }), '正在生成…')
  assert.equal(apiTokenDisplay({ enabled: true, hasToken: true, revealedToken: '' }), 'att_••••••••••••••••')
  assert.equal(
    apiTokenDisplay({ enabled: true, hasToken: true, revealedToken: 'att_secret' }),
    'att_secret'
  )
})
