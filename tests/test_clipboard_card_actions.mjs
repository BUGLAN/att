import test from 'node:test'
import assert from 'node:assert/strict'

import {
  buildClipboardTogglePatch,
  patchClipboardItems
} from '../src/utils/clipboardCardActions.mjs'

test('buildClipboardTogglePatch flips favorite state', () => {
  const patch = buildClipboardTogglePatch({
    id: 'favorite-item',
    is_favorite: false
  }, 'is_favorite')

  assert.deepEqual(patch, {
    id: 'favorite-item',
    is_favorite: true
  })
})

test('patchClipboardItems only updates the matching item', () => {
  const items = [
    { id: 'one', is_favorite: false, is_pinned: false },
    { id: 'two', is_favorite: true, is_pinned: false }
  ]

  const nextItems = patchClipboardItems(items, {
    id: 'two',
    is_pinned: true
  })

  assert.deepEqual(nextItems, [
    { id: 'one', is_favorite: false, is_pinned: false },
    { id: 'two', is_favorite: true, is_pinned: true }
  ])
  assert.notStrictEqual(nextItems, items)
})
