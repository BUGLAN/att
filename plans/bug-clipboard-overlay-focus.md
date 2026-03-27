# BUG 报告：剪切板悬浮框焦点问题

## 问题描述

**症状**：鼠标中键点击触发剪切板悬浮框后，悬浮框没有获得焦点。导致点击悬浮框外部区域无法关闭悬浮框。只有手动点击悬浮框让它获得焦点后，再点击外部区域才能正常关闭。

**影响**：用户体验受影响，悬浮框无法通过点击外部关闭，需要额外的手动操作。

## 复现步骤

1. 启动应用
2. 在任意应用中使用鼠标中键触发剪切板悬浮框
3. 悬浮框显示后，点击悬浮框外部区域
4. 预期：悬浮框关闭；实际：悬浮框不关闭
5. 手动点击悬浮框，使其获得焦点
6. 再次点击悬浮框外部区域
7. 悬浮框正常关闭

## 根本原因分析

### 1. 触发链路延迟

中键点击触发流程：
```
Python pynput 监听 → WebSocket 广播 → 前端渲染进程 → Electron 主进程
```

当 Electron 收到消息显示悬浮框时，鼠标中键可能尚未完全释放，系统焦点仍在触发中键的窗口上。

### 2. Windows 前台窗口限制

Windows 系统限制后台应用抢占前台焦点。当 Electron 应用不是当前前台应用时，`window.focus()` 可能被系统忽略。

### 3. blur 事件触发机制

当前的关闭逻辑依赖 `blur` 事件：
- `electron/clipboardOverlay/index.js:90-114`

```javascript
this.window.on('blur', () => {
  // 焦点保护期检查
  if (Date.now() < this.ignoreBlurUntil) {
    // 重新获取焦点
    return
  }
  // 关闭窗口
  this.hide()
})
```

**问题**：如果悬浮框从未获得过焦点，点击其他地方不会触发 `blur` 事件，因为焦点从未在悬浮框上。

## 已尝试的修复

### 修改 1：使用 `showInactive()` + 延迟焦点获取

**文件**：`electron/clipboardOverlay/index.js`

```javascript
// 修改前
window.show()
this.focusWindow('show')

// 修改后
window.showInactive()
setTimeout(() => {
  this.focusWindow('show-delayed')
}, 60)
```

**效果**：问题未解决

### 修改 2：增强 `focusWindow()` 方法

```javascript
focusWindow(reason) {
  // 先确保窗口可聚焦
  this.window.setFocusable(true)
  this.window.setAlwaysOnTop(true, 'screen-saver')
  this.window.moveTop()

  // 尝试让应用获取前台焦点
  app.focus({ steal: true })

  // 聚焦窗口
  this.window.focus()
  this.window.webContents.focus()
}
```

**效果**：问题未解决

## 相关代码

| 文件 | 行号 | 功能 |
|------|------|------|
| `electron/clipboardOverlay/index.js` | 90-114 | blur 事件处理 |
| `electron/clipboardOverlay/index.js` | 160-192 | `show()` 方法 |
| `electron/clipboardOverlay/index.js` | 346-367 | `focusWindow()` 方法 |
| `electron/clipboardOverlay/index.js` | 363-392 | `scheduleFocusGuard()` |
| `app/services/hotkey_listener.py` | 219-323 | 中键点击监听 |

## 可能的解决方向

### 方向 1：使用 Windows API 强制获取焦点

在 Windows 上，可以使用 `user32.dll` 的 `SetForegroundWindow` API 强制将窗口置于前台。Electron 可以通过 `ffi-napi` 或类似库调用原生 API。

### 方向 2：改用全局鼠标钩子检测点击外部

不依赖 blur 事件，而是在 Electron 主进程中使用全局鼠标钩子监听点击事件，当检测到点击在悬浮框外部时关闭窗口。

### 方向 3：使用 Electron 的 `win.setSkipTaskbar(false)` 临时显示任务栏

临时将窗口显示在任务栏，让用户点击任务栏图标激活应用，但这不是理想的解决方案。

### 方向 4：增加更长的焦点重试间隔

当前焦点守卫在 100ms 和 200ms 后重试。可以尝试增加重试次数和间隔，或在检测到焦点获取失败时使用不同的策略。

### 方向 5：使用 `webContents.executeJavaScript` 在渲染进程中处理

在悬浮框显示后，通过渲染进程的 JavaScript 强制聚焦到页面元素，可能绕过 Windows 的焦点限制。

## 调试信息

日志文件位置：`electron/logs/electron-overlay.log`

关键日志点：
- `Overlay window shown`：显示后 `isFocused` 状态
- `Overlay window focus event`：focus 事件是否触发
- `Overlay blur received`：blur 事件是否触发
- `Applied overlay focus request`：焦点请求后的状态

## 环境信息

- 操作系统：Windows 11 Pro 10.0.26200
- Electron 版本：待确认
- Node.js 版本：待确认

## 状态

**未解决** - 需要进一步调查和尝试其他方案
