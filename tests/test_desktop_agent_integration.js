const test = require('node:test')
const assert = require('node:assert/strict')
const fs = require('node:fs')
const path = require('node:path')

const repoRoot = path.join(__dirname, '..')

test('electron main process wires desktop agent manager and local desktop settings IPC', () => {
  const mainProcessSource = fs.readFileSync(path.join(repoRoot, 'electron', 'main.js'), 'utf8')

  assert.match(
    mainProcessSource,
    /DesktopAgentManager/,
    '主进程必须接入 DesktopAgentManager'
  )

  assert.match(
    mainProcessSource,
    /desktop:settings:get/,
    '主进程必须暴露读取本地桌面设置的 IPC'
  )

  assert.match(
    mainProcessSource,
    /desktop:settings:save/,
    '主进程必须暴露保存本地桌面设置的 IPC'
  )

  assert.match(
    mainProcessSource,
    /hotkey:press/,
    '主进程必须转发桌面代理的热键按下事件'
  )

  assert.match(
    mainProcessSource,
    /hotkey:release/,
    '主进程必须转发桌面代理的热键松开事件'
  )
})

test('electron main process only uses desktop agent when it is ready and clears failed startup state', () => {
  const mainProcessSource = fs.readFileSync(path.join(repoRoot, 'electron', 'main.js'), 'utf8')

  assert.match(
    mainProcessSource,
    /desktopAgentManager\?\.isReady/,
    '主进程必须只在 desktop agent 就绪后才走代理链路'
  )

  assert.match(
    mainProcessSource,
    /desktopAgentManager = null/,
    'desktop agent 启动失败后主进程必须清空失效 manager，避免后续 IPC 持续报错'
  )
})

test('preload exposes desktop settings APIs and hotkey press/release listeners', () => {
  const preloadSource = fs.readFileSync(path.join(repoRoot, 'electron', 'preload.js'), 'utf8')

  assert.match(preloadSource, /getDesktopSettings/, 'preload 必须暴露本地桌面设置读取 API')
  assert.match(preloadSource, /saveDesktopSettings/, 'preload 必须暴露本地桌面设置保存 API')
  assert.match(preloadSource, /onHotkeyPress/, 'preload 必须暴露热键按下监听 API')
  assert.match(preloadSource, /onHotkeyRelease/, 'preload 必须暴露热键松开监听 API')
  assert.doesNotMatch(preloadSource, /onHotkeyToggle/, 'preload 不应再暴露旧的热键切换监听 API')
})

test('renderer recorder only uses press/release semantics', () => {
  const recorderSource = fs.readFileSync(path.join(repoRoot, 'src', 'composables', 'useRecorder.js'), 'utf8')

  assert.match(recorderSource, /onHotkeyPress/, '录音器必须监听热键按下事件')
  assert.match(recorderSource, /onHotkeyRelease/, '录音器必须监听热键松开事件')
  assert.doesNotMatch(recorderSource, /hotkey:toggle/, '录音器不应再保留旧的 toggle 语义')
})

test('desktop settings UI removes dead mouse-trigger configuration after desktop agent refactor', () => {
  const mainProcessSource = fs.readFileSync(path.join(repoRoot, 'electron', 'main.js'), 'utf8')
  const settingsPageSource = fs.readFileSync(path.join(repoRoot, 'src', 'components', 'settings', 'SettingsPage.vue'), 'utf8')

  assert.doesNotMatch(
    settingsPageSource,
    /form\.clipboard_overlay_trigger/,
    '设置页不应继续暴露已下线的桌面端剪贴板鼠标触发配置'
  )

  assert.doesNotMatch(
    settingsPageSource,
    /selection_toolbar_enabled|selection_toolbar_trigger_delay_ms|selection_toolbar_fallback_to_clipboard/,
    '设置页不应继续暴露已失效的全局划词触发配置'
  )

  assert.doesNotMatch(
    mainProcessSource,
    /clipboard_overlay_trigger|scheduleSelectionToolbar|handleClipboardOverlayTrigger/,
    '主进程不应继续保留桌面端鼠标触发的残留同步或调度逻辑'
  )
})
