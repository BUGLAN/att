// 主窗口文字图标表（src/components/common/glyphs.mjs）单元测试。
// 覆盖：录音印章与悬浮窗同源、各类单字齐备且不为空、印章状态优先级。
import test from 'node:test'
import assert from 'node:assert/strict'

import {
  ACTION_GLYPHS,
  CLIP_TYPE_GLYPHS,
  NAV_GLYPHS,
  SEALS,
  clipTypeGlyph,
  recorderSealState
} from '../src/components/common/glyphs.mjs'
import { SEALS as FLOATING_SEALS } from '../src/components/floating/floatingView.mjs'

test('录音印章与悬浮窗同源', () => {
  assert.equal(SEALS, FLOATING_SEALS)
  assert.equal(SEALS.idle, '錄')
  assert.equal(NAV_GLYPHS.home, SEALS.idle, '首页导航与待命印章同字')
})

test('单字表：每项恰好一个字', () => {
  for (const table of [NAV_GLYPHS, ACTION_GLYPHS, CLIP_TYPE_GLYPHS]) {
    for (const [key, glyph] of Object.entries(table)) {
      assert.equal([...glyph].length, 1, `${key} 应为单字：${glyph}`)
    }
  }
})

test('剪贴板类型：未知类型回落「文」', () => {
  assert.equal(clipTypeGlyph('image'), '圖')
  assert.equal(clipTypeGlyph('code'), '長')
  assert.equal(clipTypeGlyph('whatever'), '文')
})

test('印章状态：录音中 > 处理中 > 成/敗 > 待命', () => {
  assert.equal(recorderSealState(), 'idle')
  assert.equal(recorderSealState({ recording: true, processing: true, flash: 'success' }), 'listening')
  assert.equal(recorderSealState({ processing: true, flash: 'failure' }), 'converting')
  assert.equal(recorderSealState({ flash: 'success' }), 'success')
  assert.equal(recorderSealState({ flash: 'failure' }), 'failure')
  assert.equal(recorderSealState({ flash: 'bogus' }), 'idle')
})
