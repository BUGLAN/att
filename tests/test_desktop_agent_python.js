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

test('pyproject includes pynput dependency for the desktop agent', () => {
  const pyprojectSource = fs.readFileSync(path.join(repoRoot, 'pyproject.toml'), 'utf8')

  assert.match(pyprojectSource, /pynput/, 'Python 依赖中必须包含 pynput')
})
