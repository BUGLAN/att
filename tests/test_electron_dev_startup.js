const test = require('node:test')
const assert = require('node:assert/strict')
const fs = require('node:fs')
const path = require('node:path')

const repoRoot = path.join(__dirname, '..')

test('electron existing-backend dev script keeps vite on strict port', () => {
  const packageJsonPath = path.join(repoRoot, 'package.json')
  const packageJson = JSON.parse(fs.readFileSync(packageJsonPath, 'utf8'))
  const script = packageJson.scripts['electron:dev:existing-backend']

  assert.match(
    script,
    /\bvite --strictPort\b/,
    'electron:dev:existing-backend 必须使用 vite --strictPort，避免自动漂移到其他端口'
  )

  assert.match(
    script,
    /wait-on http:\/\/127\.0\.0\.1:8090\b/,
    'electron:dev:existing-backend 必须等待 8090 端口就绪（显式 IPv4 回环，避免 localhost 双栈解析歧义）'
  )
})

test('electron main process enables single instance lock', () => {
  const mainProcessPath = path.join(repoRoot, 'electron', 'main.js')
  const mainProcessSource = fs.readFileSync(mainProcessPath, 'utf8')

  assert.match(
    mainProcessSource,
    /requestSingleInstanceLock/,
    '主进程必须申请单实例锁，避免重复注册全局输入监听'
  )

  assert.match(
    mainProcessSource,
    /second-instance/,
    '主进程必须处理 second-instance 事件，将焦点切回现有窗口'
  )
})

test('electron main process hides macOS native window buttons', () => {
  const mainProcessPath = path.join(repoRoot, 'electron', 'main.js')
  const mainProcessSource = fs.readFileSync(mainProcessPath, 'utf8')

  assert.match(
    mainProcessSource,
    /process\.platform === 'darwin'[\s\S]{0,200}setWindowButtonVisibility\(false\)/,
    "macOS 必须隐藏原生红黄绿按钮，标题栏只保留 TitleBar.vue 的自绘按钮（与 Windows 一致）"
  )
})
