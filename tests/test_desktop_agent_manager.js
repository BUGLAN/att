const test = require('node:test')
const assert = require('node:assert/strict')
const EventEmitter = require('node:events')

const { DesktopAgentManager } = require('../electron/desktopAgent/manager')

class FakeStream extends EventEmitter {
  constructor() {
    super()
    this.writes = []
  }

  write(chunk) {
    this.writes.push(chunk)
    return true
  }

  setEncoding() {}
}

class FakeChildProcess extends EventEmitter {
  constructor() {
    super()
    this.stdout = new FakeStream()
    this.stderr = new FakeStream()
    this.stdin = new FakeStream()
    this.pid = 1234
    this.killed = false
  }

  kill() {
    this.killed = true
  }
}

test('DesktopAgentManager sends agent.init and resolves start after response', async () => {
  const child = new FakeChildProcess()
  const manager = new DesktopAgentManager({
    pythonPath: 'python',
    scriptPath: 'desktop_agent.py',
    runtimeRoot: 'E:/repo/att',
    spawnProcess: () => child
  })

  const startPromise = manager.start({
    hotkey: {
      modifiers: ['win', 'alt'],
      key: null,
      display: 'Win+Alt'
    },
    pause_audio_on_recording: true
  })

  assert.equal(child.stdin.writes.length, 1)
  const request = JSON.parse(child.stdin.writes[0].trim())
  assert.equal(request.method, 'agent.init')

  child.stdout.emit('data', `${JSON.stringify({
    jsonrpc: '2.0',
    id: request.id,
    result: {
      ready: true
    }
  })}\n`)

  await startPromise
})

test('DesktopAgentManager emits hotkey press and release notifications', async () => {
  const child = new FakeChildProcess()
  const manager = new DesktopAgentManager({
    pythonPath: 'python',
    scriptPath: 'desktop_agent.py',
    runtimeRoot: 'E:/repo/att',
    spawnProcess: () => child
  })

  const events = []
  manager.on('hotkey:press', (payload) => events.push(['press', payload]))
  manager.on('hotkey:release', (payload) => events.push(['release', payload]))

  const startPromise = manager.start()
  const initRequest = JSON.parse(child.stdin.writes[0].trim())
  child.stdout.emit('data', `${JSON.stringify({
    jsonrpc: '2.0',
    id: initRequest.id,
    result: { ready: true }
  })}\n`)
  await startPromise

  child.stdout.emit('data', `${JSON.stringify({
    jsonrpc: '2.0',
    method: 'hotkey.press',
    params: {
      hotkey: { modifiers: ['win', 'alt'], key: null, display: 'Win+Alt' }
    }
  })}\n`)
  child.stdout.emit('data', `${JSON.stringify({
    jsonrpc: '2.0',
    method: 'hotkey.release',
    params: {
      hotkey: { modifiers: ['win', 'alt'], key: null, display: 'Win+Alt' }
    }
  })}\n`)

  assert.deepEqual(events, [
    ['press', { hotkey: { modifiers: ['win', 'alt'], key: null, display: 'Win+Alt' } }],
    ['release', { hotkey: { modifiers: ['win', 'alt'], key: null, display: 'Win+Alt' } }]
  ])
})

test('DesktopAgentManager sends recording and mute commands over JSON-RPC', async () => {
  const child = new FakeChildProcess()
  const manager = new DesktopAgentManager({
    pythonPath: 'python',
    scriptPath: 'desktop_agent.py',
    runtimeRoot: 'E:/repo/att',
    spawnProcess: () => child
  })

  const startPromise = manager.start()
  const initRequest = JSON.parse(child.stdin.writes[0].trim())
  child.stdout.emit('data', `${JSON.stringify({
    jsonrpc: '2.0',
    id: initRequest.id,
    result: { ready: true }
  })}\n`)
  await startPromise

  const recordingPromise = manager.setRecordingActive(true)
  const recordingRequest = JSON.parse(child.stdin.writes[1].trim())
  assert.equal(recordingRequest.method, 'recording.set_active')
  child.stdout.emit('data', `${JSON.stringify({
    jsonrpc: '2.0',
    id: recordingRequest.id,
    result: { success: true }
  })}\n`)
  await recordingPromise

  const mutePromise = manager.updateMutePolicy({ pause_audio_on_recording: false })
  const muteRequest = JSON.parse(child.stdin.writes[2].trim())
  assert.equal(muteRequest.method, 'mute.set_policy')
  child.stdout.emit('data', `${JSON.stringify({
    jsonrpc: '2.0',
    id: muteRequest.id,
    result: { success: true }
  })}\n`)
  await mutePromise
})

test('DesktopAgentManager marks shutdown exit as expected', async () => {
  const child = new FakeChildProcess()
  const manager = new DesktopAgentManager({
    pythonPath: 'python',
    scriptPath: 'desktop_agent.py',
    runtimeRoot: 'E:/repo/att',
    spawnProcess: () => child
  })

  const startPromise = manager.start()
  const initRequest = JSON.parse(child.stdin.writes[0].trim())
  child.stdout.emit('data', `${JSON.stringify({
    jsonrpc: '2.0',
    id: initRequest.id,
    result: { ready: true }
  })}\n`)
  await startPromise

  const exits = []
  manager.on('exit', (payload) => exits.push(payload))

  const stopPromise = manager.stop()
  const shutdownRequest = JSON.parse(child.stdin.writes[1].trim())
  child.stdout.emit('data', `${JSON.stringify({
    jsonrpc: '2.0',
    id: shutdownRequest.id,
    result: { success: true }
  })}\n`)
  await stopPromise

  child.emit('close', 0)

  assert.deepEqual(exits, [{ code: 0, expected: true }])
})
