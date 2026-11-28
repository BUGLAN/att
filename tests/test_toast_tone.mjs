// Toast 色调纯逻辑：warn 必须保留为独立色调，未知值继续回退 info。
import test from 'node:test'
import assert from 'node:assert/strict'

import { normalizeToastTone } from '../src/stores/toastTone.mjs'

test('Toast 支持 success、error、warn、info', () => {
  assert.equal(normalizeToastTone('success'), 'success')
  assert.equal(normalizeToastTone('error'), 'error')
  assert.equal(normalizeToastTone('warn'), 'warn')
  assert.equal(normalizeToastTone('info'), 'info')
})

test('Toast 未知色调回退 info', () => {
  assert.equal(normalizeToastTone('unknown'), 'info')
  assert.equal(normalizeToastTone(undefined), 'info')
})
