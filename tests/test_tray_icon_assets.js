const test = require('node:test')
const assert = require('node:assert/strict')
const fs = require('node:fs')
const path = require('node:path')

const { TrayManager, pickWindowsTraySize } = require('../electron/tray')

// 托盘状态必须与渲染层实际发送的字符串一致（见 src/composables/useRecorder.js）
const STATES = ['idle', 'recording', 'transcribing', 'disconnected']

// 每个平台一套多倍图：Windows 16px 基准（100%~300% 缩放全档），macOS/Linux 菜单栏 22px 基准
const PLATFORM_SIZES = {
  win: { '': 16, '@1.25x': 20, '@1.5x': 24, '@1.75x': 28, '@2x': 32, '@2.5x': 40, '@3x': 48 },
  macos: { '': 22, '@2x': 44 }
}

const TRAY_ICON_ROOT = path.join(__dirname, '..', 'electron', 'tray', 'icons')
const SRC_ROOT = path.join(__dirname, '..', 'src')
const TRAY_SOURCE = path.join(__dirname, '..', 'electron', 'tray', 'index.js')

/** 递归列出 src/ 下的 js / mjs / vue 源文件，供状态字面量扫描。 */
function listSourceFiles(root) {
  const files = []

  for (const entry of fs.readdirSync(root, { withFileTypes: true })) {
    const fullPath = path.join(root, entry.name)
    if (entry.isDirectory()) {
      files.push(...listSourceFiles(fullPath))
      continue
    }
    if (/\.(js|mjs|vue)$/.test(entry.name)) {
      files.push(fullPath)
    }
  }

  return files
}

/** 从 PNG 头里读出宽高，顺带校验魔数。 */
function readPngSize(filePath) {
  const buffer = fs.readFileSync(filePath)
  assert.equal(
    buffer.subarray(0, 8).toString('hex'),
    '89504e470d0a1a0a',
    `${filePath} 不是合法 PNG`
  )
  return {
    width: buffer.readUInt32BE(16),
    height: buffer.readUInt32BE(20),
    bytes: buffer.length
  }
}

test('托盘图标资产：每个状态在每个平台都有正确尺寸的多倍图', () => {
  for (const [platform, sizes] of Object.entries(PLATFORM_SIZES)) {
    for (const state of STATES) {
      for (const [suffix, expected] of Object.entries(sizes)) {
        const filePath = path.join(TRAY_ICON_ROOT, platform, `${state}${suffix}.png`)
        assert.ok(fs.existsSync(filePath), `缺少图标文件：${filePath}`)

        const { width, height, bytes } = readPngSize(filePath)
        assert.equal(width, expected, `${filePath} 宽度应为 ${expected}`)
        assert.equal(height, expected, `${filePath} 高度应为 ${expected}`)
        // 全透明的空壳 PNG 只有几十字节，几百字节以上说明确实画了内容
        assert.ok(bytes > 300, `${filePath} 只有 ${bytes} 字节，疑似空图`)
      }
    }
  }
})

test('托盘图标路径：TrayManager.getIconPath 落到真实存在的文件', () => {
  const manager = new TrayManager(null, null)
  const expectedDir = process.platform === 'win32' ? 'win' : 'macos'

  for (const state of STATES) {
    const iconPath = manager.getIconPath(state)
    assert.equal(path.basename(path.dirname(iconPath)), expectedDir)
    assert.ok(fs.existsSync(iconPath), `getIconPath('${state}') 指向了不存在的文件：${iconPath}`)
  }
})

test('托盘图标路径：未知 state 回落到 idle', () => {
  const manager = new TrayManager(null, null)
  assert.equal(
    manager.getIconPath('not-a-state'),
    manager.getIconPath('idle')
  )
})

test('Windows 托盘：按显示缩放挑够用的位图，绝不小于需要的物理像素', () => {
  // 200% 缩放时托盘槽位是 16 * 2 = 32 物理像素：
  // 之前给 16px 位图会被放大成糊块，现在必须挑到 32px 那一档。
  assert.equal(pickWindowsTraySize(2), 32)
  assert.equal(pickWindowsTraySize(1), 16)
  assert.equal(pickWindowsTraySize(1.25), 20)
  assert.equal(pickWindowsTraySize(1.5), 24)
  assert.equal(pickWindowsTraySize(1.75), 28)
  assert.equal(pickWindowsTraySize(2.5), 40)
  assert.equal(pickWindowsTraySize(3), 48)
})

test('Windows 托盘：异常缩放值不放大、不报错', () => {
  // 超出档位（400%）时给最大档，让系统去缩小
  assert.equal(pickWindowsTraySize(4), 48)
  // 读不到缩放（单测 / 异常）时同样给最大档：宁可缩小，不要放大
  assert.equal(pickWindowsTraySize(undefined), 48)
  assert.equal(pickWindowsTraySize(null), 48)
  assert.equal(pickWindowsTraySize(NaN), 48)
  assert.equal(pickWindowsTraySize(0), 48)
  assert.equal(pickWindowsTraySize(-1), 48)
  // 低于 100% 的缩放仍按 16px 基准
  assert.equal(pickWindowsTraySize(0.5), 16)
})

test('Windows 托盘：挑出的档位对应文件真实存在且尺寸一致', () => {
  const dir = path.join(TRAY_ICON_ROOT, 'win')
  const manager = new TrayManager(null, null)

  const cases = [
    [1, 16, ''],
    [1.5, 24, '@1.5x'],
    [2, 32, '@2x'],
    [2.5, 40, '@2.5x'],
    [3, 48, '@3x']
  ]

  for (const state of STATES) {
    for (const [scale, expectedSize, suffix] of cases) {
      const iconPath = manager.getIconPath(state, scale)

      assert.equal(
        path.basename(iconPath),
        `${state}${suffix}.png`,
        `缩放 ${scale} 时应挑 ${expectedSize}px 档位`
      )
      assert.ok(fs.existsSync(iconPath), `缺少 ${iconPath}`)

      const { width } = readPngSize(iconPath)
      assert.equal(width, expectedSize)
    }
  }
})

test('托盘图标：不再使用手绘兜底（裸 RGBA 缓冲会被当 BGRA 解析成青蓝小球）', () => {
  const source = fs.readFileSync(TRAY_SOURCE, 'utf8')
  // 只看调用，注释里提到这段历史不算复现
  assert.ok(
    !/nativeImage\.createFromBuffer\s*\(/.test(source),
    '托盘里不应再出现 createFromBuffer 兜底'
  )
  assert.ok(
    !/createDefaultIcon\s*\(/.test(source),
    '托盘里不应再出现手绘兜底图标'
  )
})

test('托盘状态：渲染层写死的 state 字面量都在托盘状态表内', () => {
  const files = listSourceFiles(SRC_ROOT)
  const sent = new Set()

  for (const file of files) {
    const source = fs.readFileSync(file, 'utf8')
    for (const match of source.matchAll(/tray\.setIcon\(\s*'([^']+)'\s*\)/g)) {
      sent.add(match[1])
    }
  }

  assert.ok(sent.size > 0, 'src/ 里没有解析到任何 tray.setIcon 字面量调用')
  for (const state of sent) {
    assert.ok(STATES.includes(state), `渲染层发送了托盘状态表里没有的 state：${state}`)
  }
})

test('托盘状态：渲染层与图标资产的状态集合一致', async () => {
  // 渲染层能推导出的状态（ESM，这里用动态 import 拿）
  const { TRAY_ICON_STATES } = await import('../src/composables/recorder/trayState.mjs')
  assert.deepEqual([...TRAY_ICON_STATES].sort(), [...STATES].sort())
})
