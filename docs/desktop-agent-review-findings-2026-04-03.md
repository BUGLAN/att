# Desktop Agent 改造遗留问题记录

记录时间：2026-04-03

本文档记录本次 `desktop agent` 重构中已确认、但暂未修复的 3 个问题，便于后续继续处理。

## 1. 剪贴板浮层的全局触发失效

- 优先级：`P1`
- 位置：`electron/main.js:561-566`
- 现象：
  - 设置页里的 `clipboard_overlay_trigger` 仍然可配置。
  - 但在 Electron 桌面环境里，鼠标中键、左右键同时按下这两种全局触发方式已经不再生效。
- 原因：
  - `desktop:sync-input-settings` 现在只会持久化 `payload.hotkey`。
  - 新的 `DesktopAgentManager` 只上报 `hotkey.press` / `hotkey.release`。
  - 旧的 `DesktopInputManager` 已被删除，原来负责调用 `handleClipboardOverlayTrigger()` 的全局鼠标监听链路也一起消失了。
- 影响：
  - 用户虽然还能在设置页看到相关选项，但这些选项在桌面端已经变成无效配置。
- 修复方向：
  - 要么在新的 desktop agent 中补回全局鼠标触发能力，并重新接入 `handleClipboardOverlayTrigger()`。
  - 要么明确下线这项能力，同时移除对应设置项和同步逻辑，避免出现“可配置但无效果”的状态。

## 2. 全局划词工具栏不再弹出

- 优先级：`P1`
- 位置：`electron/main.js:254-260`
- 现象：
  - 系统范围内拖拽选中文本后，划词工具栏不再弹出。
  - 设置页中的 `selection_toolbar_enabled`、`selection_toolbar_trigger_delay_ms` 等配置仍然存在。
- 原因：
  - 当前 desktop agent 只转发热键按下/松开事件。
  - 旧实现里由全局鼠标监听触发的 `scheduleSelectionToolbar()` 已经失去调用入口。
  - 现在主进程中虽然还保留了 `scheduleSelectionToolbar()`，但没有任何全局输入事件能触发它。
- 影响：
  - 划词工具栏在桌面端的核心交互已经回归失效。
  - 相关设置项同样变成“UI 仍在，但功能已断开”的状态。
- 修复方向：
  - 在 desktop agent 中补充全局鼠标拖拽/选区检测能力，并重新接回 `scheduleSelectionToolbar()`。
  - 如果短期内不打算恢复该能力，需要同步下线设置项和相关主进程逻辑。

## 3. desktop agent 启动失败时可能直接导致主进程崩溃

- 优先级：`P1`
- 位置：`electron/desktopAgent/manager.js:52-55`
- 现象：
  - 当辅助进程无法启动时，例如 Python 缺失、`EPERM`、被安全软件拦截等，Electron 可能直接崩溃，而不是向前端展示可恢复的错误提示。
- 原因：
  - `DesktopAgentManager` 在子进程 `error` 回调里直接 `emit('error', error)`。
  - 但 `electron/main.js` 没有为这个对象注册 `'error'` 监听器。
  - 在 Node 里，未监听的 `'error'` 事件会直接抛出异常，导致进程退出。
- 影响：
  - desktop agent 失败本应只是“功能不可用并提示用户”，但现在会升级成应用启动稳定性问题。
- 修复方向：
  - 不要直接抛裸 `'error'` 事件。
  - 改为内部消化后转换成普通事件，例如统一走已有的 `agent:error` / `desktop:agent-error` 上报链路。
  - 同时补一条测试，覆盖子进程启动失败时“不会崩溃、会正常上报”的场景。

## 后续修复建议

建议按下面顺序处理：

1. 先修复第 3 个问题，避免 desktop agent 启动异常时把整个 Electron 主进程带崩。
2. 再决定第 1、2 个问题的产品方向：
   - 如果继续保留桌面端全局鼠标能力，就在 desktop agent 中补回对应监听与事件转发。
   - 如果不再保留，就同步删除设置项、同步接口和残留逻辑，避免伪配置。
3. 为第 1、2 个问题分别补集成测试，覆盖：
   - 剪贴板浮层触发链路。
   - 划词工具栏激活链路。
