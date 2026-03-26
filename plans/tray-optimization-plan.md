# 托盘应用优化计划

## 问题分析

### 问题 1: 热键 (Win+Alt) 捕获失效

**根本原因**：
- Electron 的 `globalShortcut` 中 `Super+Alt` 格式可能不正确
- Windows 下 Win 键的正确格式是 `Super`，但组合键顺序和格式有要求

**Electron 快捷键格式要求**：
- 单独的 Win 键: `Super`
- Win+其他键: `Super+X` (Win 键在前)
- 但 `Super+Alt` 可能不被系统识别，因为这是系统保留组合键

**解决方案**：
1. 尝试 `Alt+Super` (交换顺序)
2. 或使用其他组合键如 `Alt+Shift+R`
3. 添加调试日志确认快捷键是否注册成功

### 问题 2: 右键菜单不显示历史记录

**根本原因**：
- `TrayManager.updateMenu(records)` 只在托盘创建时调用了一次，且没有传入任何记录
- 没有任何地方调用 `updateMenu` 来更新菜单

**调用链分析**：
```
main.js:setupTray() → TrayManager.create() → updateMenu() // records = []
// 之后没有任何地方调用 updateMenu
```

**解决方案**：
1. 在主进程添加 IPC 处理器，接收前端传来的记录数据
2. 前端在加载记录后，通过 IPC 更新托盘菜单
3. 或在托盘菜单弹出时动态获取记录

---

## Implementation

### Step 1: 修复热键注册并添加调试日志

**文件**: `electron/main.js`

```javascript
function registerHotkey(accelerator) {
  globalShortcut.unregisterAll()

  try {
    const success = globalShortcut.register(accelerator, triggerHotkey)
    if (success) {
      console.log(`[Hotkey] Registered: ${accelerator}`)
    } else {
      console.error(`[Hotkey] Failed to register: ${accelerator}`)
    }
  } catch (error) {
    console.error('[Hotkey] Registration error:', error)
  }
}

function setupHotkey() {
  // Windows: 尝试 Alt+Super (Alt+Win)
  // 如果仍然失败，考虑换用 Alt+Shift+R
  const defaultAccelerator = process.platform === 'darwin'
    ? 'CommandOrControl+Alt'
    : 'Alt+Super'  // 或 'Alt+Shift+R'

  registerHotkey(defaultAccelerator)
}
```

### Step 2: 添加托盘菜单更新 IPC

**文件**: `electron/main.js`

添加 IPC 处理器：

```javascript
function setupIPC() {
  // ... 现有代码 ...

  // 新增：更新托盘菜单
  ipcMain.on('tray:update-menu', (event, records) => {
    trayManager?.updateMenu(records)
  })
}
```

**文件**: `electron/preload.js`

暴露更新托盘菜单的 API：

```javascript
contextBridge.exposeInMainWorld('electronAPI', {
  // ... 现有代码 ...
  updateTrayMenu: (records) => ipcRenderer.send('tray:update-menu', records)
})
```

### Step 3: 前端调用托盘菜单更新

**文件**: `src/composables/useElectron.js`

添加托盘菜单更新方法：

```javascript
const tray = {
  setIcon: (iconType) => globalThis.window?.electronAPI?.setTrayIcon(iconType),
  updateMenu: (records) => globalThis.window?.electronAPI?.updateTrayMenu(records)
}
```

**文件**: `src/composables/useRecords.js`

在加载记录后更新托盘菜单：

```javascript
async function loadRecords() {
  await recordsStore.loadRecords()
  // 更新托盘菜单
  const { tray } = useElectron()
  tray.updateMenu(recordsStore.sortedRecords)
}
```

### Step 4: 在记录变化时更新托盘菜单

在以下时机调用 `tray.updateMenu`:
- 应用启动后加载记录
- 新增记录后
- 删除记录后
- 记录状态变化后

---

## Critical Files

| 文件 | 改动内容 |
|------|----------|
| `electron/main.js` | 修复热键注册，添加 IPC 处理器 |
| `electron/preload.js` | 暴露 `updateTrayMenu` API |
| `src/composables/useElectron.js` | 添加 `tray.updateMenu` 方法 |
| `src/composables/useRecords.js` | 在记录变化时更新托盘菜单 |

---

## 关于 Win+Alt 快捷键的说明

Windows 系统对 `Win+Alt` 组合键有特殊处理：
- 这是某些应用的系统菜单快捷键
- 可能被系统拦截

如果 `Alt+Super` 仍然不工作，建议更换为：
- `Alt+Shift+R` (R for Record)
- `Ctrl+Alt+R`
- 或在设置中让用户自定义快捷键

---

## Verification

1. 启动 Electron 应用，查看控制台日志确认快捷键注册成功
2. 按下设置的快捷键，验证录音功能
3. 右键点击托盘图标，验证历史记录是否显示
4. 点击历史记录项，验证复制功能
