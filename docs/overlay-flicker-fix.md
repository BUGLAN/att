# Electron 悬浮窗口闪烁问题修复文档

## 问题描述

在 Electron 应用中，划词翻译悬浮框和剪切板悬浮框在**第二次打开时会出现闪烁现象**。

### 症状表现

- **第一次打开**：正常显示，无任何问题
- **第二次及后续打开**：出现短暂的"缩小又弹大"的动画效果，视觉上表现为闪烁
- **闪烁时长**：非常短，约几毫秒
- **影响范围**：划词悬浮框和剪切板悬浮框均有此问题

## 问题分析

### 架构背景

项目使用 `UnifiedOverlayManager` 统一管理两种悬浮窗口：

```
electron/overlay/
├── index.js        # UnifiedOverlayManager 类
└── constants.js    # 模式和通道常量定义
```

两种模式共用同一个 `BrowserWindow` 实例：

| 模式 | 尺寸 | 说明 |
|------|------|------|
| `selection` (划词翻译) | 420 x 320 | 无焦点显示 (`showInactive`) |
| `clipboard` (剪切板) | 380 x 560 | 需要获取焦点 (`show` + `focus`) |

### 窗口创建配置

```javascript
// electron/overlay/index.js - createWindow()
this.window = new this.BrowserWindow({
  width: selectionConfig.width,    // 初始使用 selection 模式尺寸 (420x320)
  height: selectionConfig.height,
  show: false,                     // 创建时不显示
  frame: false,
  transparent: true,
  backgroundColor: '#00000000',    // 完全透明背景
  alwaysOnTop: true,
  resizable: false,
  maximizable: false,
  minimizable: false,
  fullscreenable: false,
  movable: true,
  skipTaskbar: true,
  focusable: true,
  hasShadow: true,
  // ...
})
```

### 窗口显示流程

```
openClipboard() / openSelection()
    ↓
createWindow()                      # 复用已存在的窗口或创建新窗口
    ↓
applyBounds(targetBounds)           # 设置窗口位置和尺寸
    ↓
dispatchActivation()                # 发送激活消息给渲染器
    ↓
[渲染器] 收到消息，更新内容
    ↓
[渲染器] notifyReady()              # 通知主进程准备就绪
    ↓
markRendererReady()
    ↓
showPreparedWindow()                # 显示窗口
    ↓
window.show() / window.showInactive()
```

### 问题根因

**窗口复用导致的尺寸变化闪烁**：

1. **第一次打开**（以剪切板模式为例）
   - 窗口创建时尺寸为 420x320（selection 模式的尺寸）
   - `applyBounds` 设置为 380x560（clipboard 模式的尺寸）
   - 窗口显示，用户看到正常尺寸的窗口

2. **窗口关闭**
   - 调用 `window.hide()` 隐藏窗口
   - 窗口对象仍然存在，尺寸保持为 380x560

3. **第二次打开**
   - 复用已存在的窗口（尺寸仍为 380x560）
   - Electron 在 `window.show()` 时可能触发窗口管理器的重新布局
   - **Windows 系统的窗口动画效果**在从隐藏状态显示窗口时会被触发
   - 如果窗口尺寸有微小变化或系统重新计算布局，会出现视觉上的"缩小弹大"效果

### 尝试过的无效方案

| 方案 | 说明 | 结果 |
|------|------|------|
| 延迟 `setBounds` 调用时机 | 在 `showPreparedWindow` 中设置尺寸而非 `openClipboard` | 无效 |
| 使用 `setOpacity(0)` 遮蔽 | 显示前设置透明度为 0，显示后恢复 | 无效 |
| 双 `requestAnimationFrame` | 渲染器端增加帧延迟确保渲染完成 | 无效 |
| 在 `hide` 时重置窗口尺寸 | 关闭时重置到初始尺寸 420x320 | 无效 |
| 移除 `focusWindow` 中的 `moveTop/focus` | 减少窗口显示后的操作 | 无效 |

## 解决方案

### 最终方案：关闭时销毁窗口

**核心思路**：不再复用窗口，每次关闭时销毁窗口，下次打开时重新创建。

### 代码修改

```javascript
// electron/overlay/index.js

hide(reason = 'manual') {
  void reason
  if (!this.window) {
    return
  }

  // 销毁窗口而不是隐藏，避免下次打开时闪烁
  this.window.destroy()
  this.window = null
  this.isWindowLoaded = false
  this.isVisible = false
  this.userMovedWindow = false
  this.ignoreBlurUntil = 0
  this.rendererReady = false
  this.pendingActivation = null
}
```

### 修改前后对比

**修改前**：
```javascript
hide(reason = 'manual') {
  void reason
  if (!this.window) {
    return
  }

  this.window.hide()
  this.isVisible = false
  this.userMovedWindow = false
  this.ignoreBlurUntil = 0
}
```

**修改后**：
```javascript
hide(reason = 'manual') {
  void reason
  if (!this.window) {
    return
  }

  // 销毁窗口而不是隐藏，避免下次打开时闪烁
  this.window.destroy()
  this.window = null
  this.isWindowLoaded = false
  this.isVisible = false
  this.userMovedWindow = false
  this.ignoreBlurUntil = 0
  this.rendererReady = false
  this.pendingActivation = null
}
```

## 方案权衡

### 优点

1. **彻底解决闪烁问题**：每次都是全新窗口，避免了窗口复用时的状态残留问题
2. **代码改动小**：只需修改 `hide` 方法，无需改动显示逻辑
3. **状态干净**：每次打开都是干净的初始状态，避免潜在的状态污染

### 缺点

1. **打开速度变慢**：每次打开都需要重新创建窗口、加载页面
2. **资源开销**：频繁创建销毁窗口有一定开销
3. **用户体验**：首次打开比窗口复用方式稍慢

## 相关文件

```
electron/overlay/
├── index.js           # UnifiedOverlayManager - 主要修改文件
└── constants.js       # OVERLAY_MODE, OVERLAY_CHANNELS 常量定义

src/components/overlay/
├── UnifiedOverlayWindow.vue   # 统一悬浮窗口入口组件
├── SelectionPanel.vue         # 划词翻译面板
└── ClipboardPanel.vue         # 剪切板面板

src/styles/
├── unified-overlay.css        # 统一悬浮窗口样式
└── clipboard-overlay.css      # 剪切板样式
```

## 提交记录

```
fix(overlay): 关闭时销毁窗口避免二次打开闪烁
```

## 后续优化方向

如果需要提升打开速度，可以考虑以下优化：

1. **预创建窗口池**：在后台预先创建窗口，使用时直接显示
2. **页面预加载**：使用 `BrowserWindow` 的预加载机制
3. **骨架屏**：在数据加载前显示骨架屏，视觉上减少等待感
4. **缓存机制**：对窗口内容进行缓存，减少重复渲染

## 参考资料

- [Electron BrowserWindow 文档](https://www.electronjs.org/docs/latest/api/browser-window)
- [Electron 窗口动画问题讨论](https://github.com/electron/electron/issues/2170)
