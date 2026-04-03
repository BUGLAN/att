import test from 'node:test'
import assert from 'node:assert/strict'

import { derivePostStopState, deriveStartAbortState } from '../src/utils/recordingStopState.mjs'

test('single recording enters transcribing immediately after a valid stop', () => {
  assert.deepEqual(
    derivePostStopState({ mode: 'single', durationSeconds: 3 }),
    {
      recording: false,
      transcribing: true,
      trayIcon: 'transcribing'
    }
  )
})

test('single recording stays idle after a too-short stop', () => {
  assert.deepEqual(
    derivePostStopState({ mode: 'single', durationSeconds: 0 }),
    {
      recording: false,
      transcribing: false,
      trayIcon: 'idle'
    }
  )
})

test('multi recording enters transcribing immediately after stop', () => {
  assert.deepEqual(
    derivePostStopState({ mode: 'multi', durationSeconds: 0 }),
    {
      recording: false,
      transcribing: true,
      trayIcon: 'transcribing'
    }
  )
})

test('aborted start returns recorder state to idle', () => {
  assert.deepEqual(
    deriveStartAbortState(),
    {
      recording: false,
      transcribing: false,
      trayIcon: 'idle'
    }
  )
})
