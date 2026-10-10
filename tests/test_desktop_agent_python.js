const test = require('node:test')
const assert = require('node:assert/strict')
const fs = require('node:fs')
const path = require('node:path')

const repoRoot = path.join(__dirname, '..')

test('desktop agent python script exists and uses pynput keyboard listener', () => {
  const scriptPath = path.join(repoRoot, 'electron', 'python', 'desktop_agent.py')
  const source = fs.readFileSync(scriptPath, 'utf8')

  assert.match(source, /pynput/, 'desktop agent 必须使用 pynput')
  assert.match(source, /recording\.set_active/, 'desktop agent 必须处理录音状态同步命令')
  assert.match(source, /hotkey\.press/, 'desktop agent 必须上报热键按下事件')
  assert.match(source, /hotkey\.release/, 'desktop agent 必须上报热键松开事件')
})

test('desktop agent python script detects macOS accessibility permission before listening', () => {
  const scriptPath = path.join(repoRoot, 'electron', 'python', 'desktop_agent.py')
  const source = fs.readFileSync(scriptPath, 'utf8')

  // pynput 在 macOS 无辅助功能权限时静默失效（不抛异常），
  // 因此 agent 必须在启动监听器前主动检测并上报 permission.required
  assert.match(source, /AXIsProcessTrusted/, 'desktop agent 必须主动检测 macOS 辅助功能权限')
  assert.match(source, /check_macos_keyboard_trust/, 'desktop agent 必须封装权限检测函数')
  assert.match(
    source,
    /permission\.required/,
    'desktop agent 必须在无权限时上报 permission.required 事件'
  )
})

test('pyproject includes pynput dependency for the desktop agent', () => {
  const pyprojectSource = fs.readFileSync(path.join(repoRoot, 'pyproject.toml'), 'utf8')

  assert.match(pyprojectSource, /pynput/, 'Python 依赖中必须包含 pynput')
})
