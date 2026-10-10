const assert = require('node:assert/strict')

const { UnifiedOverlayManager } = require('../electron/overlay')

class FakeBrowserWindow {
  constructor(options = {}) {
    this.options = options
    this.bounds = {
      x: 0,
      y: 0,
      width: options.width,
      height: options.height
    }
    this.visible = false
    // 记录窗口是否已被销毁，供断言「hide() 等价于销毁窗口」这一语义
    this.destroyed = false
    this.handlers = {}
    this.webContents = {
      sent: [],
      once: (event, handler) => {
        this.handlers[event] = handler
      },
      send: (channel, payload) => {
        this.webContents.sent.push({ channel, payload })
      },
      focus: () => {}
    }
  }

  setAlwaysOnTop() {}
  setVisibleOnAllWorkspaces() {}
  loadURL() {}
  loadFile() {}
  on(event, handler) {
    this.handlers[event] = handler
  }
  setBounds(bounds) {
    this.bounds = bounds
  }
  getBounds() {
    return this.bounds
  }
  show() {
    this.visible = true
  }
  showInactive() {
    this.visible = true
  }
  moveTop() {}
  setFocusable() {}
  focus() {}
  hide() {
    this.visible = false
  }
  isDestroyed() {
    return this.destroyed === true
  }
  destroy() {
    this.visible = false
    this.destroyed = true
    // 与 Electron 真实行为保持一致：destroy() 会触发 closed 事件
    if (typeof this.handlers.closed === 'function') {
      this.handlers.closed()
    }
  }
}

function createManager() {
  return new UnifiedOverlayManager({
    BrowserWindow: FakeBrowserWindow,
    screen: {
      getCursorScreenPoint: () => ({ x: 320, y: 240 }),
      getDisplayNearestPoint: () => ({
        workArea: { x: 0, y: 0, width: 1440, height: 900 }
      }),
      screenToDipPoint: (point) => point
    },
    isDevelopmentRuntime: () => false
  })
}

// electron/overlay/index.js 的 hide() 已从「隐藏窗口」改为「销毁窗口并把 this.window 置空」
// （提交 585f3a9，避免下次打开时闪烁），因此这里断言语义等价的结果：
// 旧窗口被销毁、manager.window 变成 null、isVisible 为 false。
function assertWindowDiscarded(manager, previousWindow) {
  assert.notEqual(previousWindow, null, 'hide() 之前应当存在窗口实例')
  assert.equal(previousWindow.visible, false, '被销毁的窗口不应处于可见状态')
  assert.equal(previousWindow.isDestroyed(), true, 'hide() 应当销毁窗口而不是仅隐藏')
  assert.equal(manager.window, null, 'hide() 之后 manager.window 应当被置空')
  assert.equal(manager.isVisible, false, 'hide() 之后 manager.isVisible 应当为 false')
}

async function testSelectionWaitsForReadyBeforeShow() {
  const manager = createManager()
  manager.openSelection({
    text: 'hello',
    point: { x: 200, y: 180 },
    trigger: 'mouse_selection'
  })

  manager.window.handlers['did-finish-load']()

  assert.equal(manager.window.visible, false)
  assert.equal(manager.window.webContents.sent[0].channel, 'overlay:activate')
  assert.equal(manager.window.webContents.sent[0].payload.mode, 'selection')

  manager.markRendererReady()
  assert.equal(manager.window.visible, true)

  const bounds = manager.window.getBounds()
  assert.equal(bounds.x, 996)
  assert.equal(bounds.y, 24)
}

async function testClipboardModeShowsAfterReady() {
  const manager = createManager()
  manager.openClipboard({ focusSearch: true })
  manager.window.handlers['did-finish-load']()
  manager.markRendererReady()

  assert.equal(manager.currentMode, 'clipboard')
  assert.equal(manager.window.visible, true)
  assert.equal(manager.pendingActivation.mode, 'clipboard')
}

async function testClipboardIgnoresSelectionWhileVisible() {
  const manager = createManager()
  manager.openClipboard({ focusSearch: true })
  manager.window.handlers['did-finish-load']()
  manager.markRendererReady()

  const result = manager.openSelection({
    text: 'ignored',
    point: { x: 120, y: 120 }
  })

  assert.equal(result, false)
  assert.equal(manager.currentMode, 'clipboard')
}

async function testClipboardBlurDuringGraceDoesNotHideWindow() {
  const manager = createManager()
  manager.openClipboard({ focusSearch: true })
  manager.window.handlers['did-finish-load']()
  manager.markRendererReady()

  manager.window.handlers.blur()

  assert.equal(manager.window.visible, true)
  assert.equal(manager.isVisible, true)
}

async function testClipboardOutsidePointerHidesWindow() {
  const manager = createManager()
  manager.openClipboard({ focusSearch: true })
  manager.window.handlers['did-finish-load']()
  manager.markRendererReady()

  const previousWindow = manager.window

  manager.handleGlobalPointerDown({
    point: { x: 40, y: 40 }
  })

  assertWindowDiscarded(manager, previousWindow)
}

async function testClipboardInsidePointerKeepsWindowVisible() {
  const manager = createManager()
  manager.openClipboard({ focusSearch: true })
  manager.window.handlers['did-finish-load']()
  manager.markRendererReady()

  const bounds = manager.window.getBounds()
  manager.handleGlobalPointerDown({
    point: {
      x: bounds.x + 20,
      y: bounds.y + 20
    }
  })

  assert.equal(manager.window.visible, true)
  assert.equal(manager.isVisible, true)
}

async function testClipboardPointerUsesDipCoordinates() {
  const manager = new UnifiedOverlayManager({
    BrowserWindow: FakeBrowserWindow,
    screen: {
      getCursorScreenPoint: () => ({ x: 320, y: 240 }),
      getDisplayNearestPoint: () => ({
        workArea: { x: 0, y: 0, width: 1440, height: 900 }
      }),
      screenToDipPoint: (point) => ({
        x: Math.round(point.x / 1.5),
        y: Math.round(point.y / 1.5)
      })
    },
    isDevelopmentRuntime: () => false
  })

  manager.openClipboard({ focusSearch: true })
  manager.window.handlers['did-finish-load']()
  manager.markRendererReady()

  const bounds = manager.window.getBounds()
  manager.handleGlobalPointerDown({
    point: {
      x: Math.round((bounds.x + 24) * 1.5),
      y: Math.round((bounds.y + 24) * 1.5)
    }
  })

  assert.equal(manager.window.visible, true)
  assert.equal(manager.isVisible, true)
}

async function testSelectionOutsidePointerHidesWindow() {
  const manager = createManager()
  manager.openSelection({
    text: 'hello',
    point: { x: 200, y: 180 }
  })
  manager.window.handlers['did-finish-load']()
  manager.markRendererReady()

  const previousWindow = manager.window

  manager.handleGlobalPointerDown({
    point: { x: 40, y: 40 }
  })

  assertWindowDiscarded(manager, previousWindow)
}

async function testSelectionInsidePointerKeepsWindowVisible() {
  const manager = createManager()
  manager.openSelection({
    text: 'hello',
    point: { x: 200, y: 180 }
  })
  manager.window.handlers['did-finish-load']()
  manager.markRendererReady()

  const bounds = manager.window.getBounds()
  manager.handleGlobalPointerDown({
    point: {
      x: bounds.x + 20,
      y: bounds.y + 20
    }
  })

  assert.equal(manager.window.visible, true)
  assert.equal(manager.isVisible, true)
}

async function testSelectionReusesLastMovedPosition() {
  const manager = createManager()
  manager.openSelection({
    text: 'hello',
    point: { x: 200, y: 180 }
  })
  manager.window.handlers['did-finish-load']()
  manager.markRendererReady()

  manager.window.setBounds({
    x: 720,
    y: 180,
    width: 420,
    height: 320
  })
  manager.programmaticMoveLock = 0
  manager.window.handlers.move()
  manager.hide('manual')

  manager.openSelection({
    text: 'world',
    point: { x: 120, y: 120 }
  })

  const bounds = manager.window.getBounds()
  assert.equal(bounds.x, 720)
  assert.equal(bounds.y, 180)
}

async function testSelectionActionCompleteKeepsWindowVisible() {
  const manager = createManager()
  manager.openSelection({
    text: 'hello',
    point: { x: 200, y: 180 }
  })
  manager.window.handlers['did-finish-load']()
  manager.markRendererReady()

  manager.handleActionComplete({
    mode: 'selection',
    actionType: 'translate'
  })

  assert.equal(manager.window.visible, true)
  assert.equal(manager.isVisible, true)
}

async function testClipboardWindowCreatedAtTargetSize() {
  const manager = createManager()
  manager.openClipboard()

  // 回归：窗口必须按**目标模式**尺寸创建。
  // 旧实现固定用 selection 尺寸(420x320)创建，再在隐藏状态下 setBounds 改成
  // clipboard 尺寸(380x560)；Windows 上这类透明无边框窗口会在首次 show() 时
  // 播放一次尺寸过渡动画，视觉上就是「缩小又弹大」的闪烁。
  assert.equal(manager.window.options.width, 380)
  assert.equal(manager.window.options.height, 560)
  // 透明窗口关闭系统投影，避免 DWM 阴影带来的显示动画与边缘残影
  assert.equal(manager.window.options.hasShadow, false)
}

async function testSelectionWindowCreatedAtTargetSize() {
  const manager = createManager()
  manager.openSelection({
    text: 'hello',
    point: { x: 200, y: 180 }
  })

  assert.equal(manager.window.options.width, 420)
  assert.equal(manager.window.options.height, 320)
}

async function testRecreatedWindowUsesCurrentModeSize() {
  const manager = createManager()
  manager.openSelection({
    text: 'hello',
    point: { x: 200, y: 180 }
  })
  manager.window.handlers['did-finish-load']()
  manager.markRendererReady()
  manager.hide('manual')

  assert.equal(manager.window, null)

  // 关闭后再以剪切板模式打开：新窗口必须是剪切板尺寸，而不是 selection 尺寸
  manager.openClipboard()
  assert.equal(manager.window.options.width, 380)
  assert.equal(manager.window.options.height, 560)
}

async function testApplyBoundsSkipsRedundantResize() {
  const manager = createManager()
  manager.openSelection({
    text: 'hello',
    point: { x: 200, y: 180 }
  })

  const before = { ...manager.window.getBounds() }
  let setBoundsCalls = 0
  const originalSetBounds = manager.window.setBounds.bind(manager.window)
  manager.window.setBounds = (bounds) => {
    setBoundsCalls += 1
    originalSetBounds(bounds)
  }

  // 尺寸与位置完全一致时不应触发窗口管理调用（避免多余的系统动画）
  manager.applyBounds({ ...before })
  assert.equal(setBoundsCalls, 0)

  // 仅有位置变化时仍需生效
  manager.applyBounds({ ...before, x: before.x - 40 })
  assert.equal(setBoundsCalls, 1)
  assert.equal(manager.window.getBounds().x, before.x - 40)
}

async function main() {
  // 用例清单集中登记：逐个 await 执行，任一用例失败都会中断并落到下面的 catch
  const cases = [
    ['选择模式等待渲染完成后再显示', testSelectionWaitsForReadyBeforeShow],
    ['剪贴板模式就绪后显示', testClipboardModeShowsAfterReady],
    ['剪贴板可见时忽略选择请求', testClipboardIgnoresSelectionWhileVisible],
    ['宽限期内失焦不关闭窗口', testClipboardBlurDuringGraceDoesNotHideWindow],
    ['剪贴板外部点击销毁窗口', testClipboardOutsidePointerHidesWindow],
    ['剪贴板内部点击保持窗口可见', testClipboardInsidePointerKeepsWindowVisible],
    ['剪贴板命中判断使用 DIP 坐标', testClipboardPointerUsesDipCoordinates],
    ['选择模式外部点击销毁窗口', testSelectionOutsidePointerHidesWindow],
    ['选择模式内部点击保持窗口可见', testSelectionInsidePointerKeepsWindowVisible],
    ['选择模式复用上次拖拽位置', testSelectionReusesLastMovedPosition],
    ['选择模式动作完成后窗口保持可见', testSelectionActionCompleteKeepsWindowVisible],
    ['剪切板窗口按目标尺寸创建（防尺寸闪烁）', testClipboardWindowCreatedAtTargetSize],
    ['选择窗口按目标尺寸创建', testSelectionWindowCreatedAtTargetSize],
    ['销毁后重开按当前模式尺寸创建', testRecreatedWindowUsesCurrentModeSize],
    ['边界完全一致时跳过多余 setBounds', testApplyBoundsSkipsRedundantResize]
  ]

  for (const [name, run] of cases) {
    await run()
    console.log(`  ✔ ${name}`)
  }

  console.log(`test_unified_overlay_manager.js: ${cases.length} 个用例全部通过`)
  process.exit(0)
}

main().catch((error) => {
  console.error(error)
  process.exit(1)
})
