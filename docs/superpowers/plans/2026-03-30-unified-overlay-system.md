# Unified Overlay System Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the separate selection toolbar and clipboard overlay implementations with one shared overlay window system that supports `selection` and `clipboard` modes.

**Architecture:** Introduce a single Electron overlay manager and a single renderer entrypoint. The manager owns one `BrowserWindow`, computes mode-specific bounds and focus rules, and activates a unified Vue shell that renders `SelectionPanel` or `ClipboardPanel` without extra backdrop windows.

**Tech Stack:** Electron, Vue 3, Vite, Node IPC, existing FastAPI-backed renderer APIs

---

### Task 1: Establish Unified Overlay Contracts

**Files:**
- Create: `electron/overlay/constants.js`
- Create: `electron/overlay/index.js`
- Modify: `electron/main.js`
- Modify: `electron/preload.js`
- Test: `tests/test_unified_overlay_manager.js`

- [ ] **Step 1: Write the failing manager contract tests**

```js
it('creates one window and emits overlay:activate for selection mode', () => {
  const manager = new UnifiedOverlayManager({ BrowserWindow: FakeWindow, screen: fakeScreen })
  manager.openSelection({ text: 'hello', point: { x: 10, y: 10 } })
  assert.equal(manager.window.webContents.sent[0].channel, 'overlay:activate')
  assert.equal(manager.window.webContents.sent[0].payload.mode, 'selection')
})

it('switches an existing selection overlay into clipboard mode', () => {
  const manager = new UnifiedOverlayManager({ BrowserWindow: FakeWindow, screen: fakeScreen })
  manager.openSelection({ text: 'hello' })
  manager.markRendererReady()
  manager.openClipboard({ focusSearch: true })
  assert.equal(manager.window.webContents.sent.at(-1).payload.mode, 'clipboard')
})
```

- [ ] **Step 2: Run the focused test to verify it fails**

Run: `uv run python -m pytest tests -q`
Expected: FAIL because `UnifiedOverlayManager` and its tests do not exist yet.

- [ ] **Step 3: Add shared mode constants and the new manager skeleton**

```js
// electron/overlay/constants.js
const OVERLAY_MODE = {
  selection: 'selection',
  clipboard: 'clipboard'
}

module.exports = { OVERLAY_MODE }
```

```js
// electron/overlay/index.js
class UnifiedOverlayManager {
  openSelection(payload = {}) {}
  openClipboard(payload = {}) {}
  hide() {}
  setPinned(active) {}
  markRendererReady() {}
}

module.exports = { UnifiedOverlayManager }
```

- [ ] **Step 4: Rewire `electron/main.js` and `electron/preload.js` onto unified IPC names**

```js
ipcMain.on('overlay:open-selection', (event, payload = {}) => {
  overlayManager?.openSelection(payload)
})

ipcMain.on('overlay:open-clipboard', (event, payload = {}) => {
  overlayManager?.openClipboard(payload)
})
```

```js
openSelectionOverlay: (payload) => ipcRenderer.send('overlay:open-selection', payload),
openClipboardOverlay: (payload) => ipcRenderer.send('overlay:open-clipboard', payload),
hideOverlay: () => ipcRenderer.send('overlay:hide'),
toggleOverlayPinned: (payload) => ipcRenderer.send('overlay:pin', payload),
onOverlayActivate: (callback) => {
  ipcRenderer.on('overlay:activate', (event, payload) => callback(payload))
}
```

- [ ] **Step 5: Run tests again and confirm the contract passes**

Run: `uv run python -m pytest tests -q`
Expected: PASS with the new manager test included.


### Task 2: Build One Renderer Shell With Two Panels

**Files:**
- Create: `src/unified-overlay.html`
- Create: `src/unified-overlay.js`
- Create: `src/components/overlay/UnifiedOverlayWindow.vue`
- Create: `src/components/overlay/SelectionPanel.vue`
- Create: `src/components/overlay/ClipboardPanel.vue`
- Create: `src/styles/unified-overlay.css`
- Modify: `vite.config.js`
- Modify: `src/composables/useElectron.js`

- [ ] **Step 1: Create the new Vite entry and shell component**

```js
// src/unified-overlay.js
import { createApp } from 'vue'
import UnifiedOverlayWindow from './components/overlay/UnifiedOverlayWindow.vue'
import './styles/unified-overlay.css'

createApp(UnifiedOverlayWindow).mount('#app')
```

```html
<!-- src/unified-overlay.html -->
<!DOCTYPE html>
<html lang="zh-CN">
<head>
  <meta charset="UTF-8">
  <meta name="viewport" content="width=device-width, initial-scale=1.0">
  <title>ATT Unified Overlay</title>
</head>
<body>
  <div id="app"></div>
  <script type="module" src="./unified-overlay.js"></script>
</body>
</html>
```

- [ ] **Step 2: Add shell-level mode switching and ready handshake**

```vue
<script setup>
const mode = ref('selection')
const activation = ref(null)

onMounted(() => {
  onOverlayActivate((payload) => {
    mode.value = payload.mode
    activation.value = payload
    nextTick(() => window.electronAPI?.notifyOverlayReady?.())
  })
})
</script>
```

- [ ] **Step 3: Move selection and clipboard business UI into dedicated panels**

```vue
<SelectionPanel
  v-if="mode === 'selection'"
  :activation="activation"
  :pinned="isPinned"
  @close="closeOverlay"
  @pin="togglePin"
  @action-complete="handleSelectionActionComplete"
/>
<ClipboardPanel
  v-else
  :activation="activation"
  :pinned="isPinned"
  @close="closeOverlay"
  @pin="togglePin"
/>
```

- [ ] **Step 4: Register the new entry in Vite**

```js
unifiedOverlay: resolve(__dirname, 'src/unified-overlay.html')
```

- [ ] **Step 5: Run the frontend build to confirm the new entry compiles**

Run: `npm run build`
Expected: PASS and emit `dist/unified-overlay.html`.


### Task 3: Implement Stable Window Timing, Focus Rules, and Bounds

**Files:**
- Modify: `electron/overlay/index.js`
- Modify: `electron/main.js`
- Test: `tests/test_unified_overlay_manager.js`

- [ ] **Step 1: Add mode-specific bounds and focus policy**

```js
const MODE_CONFIG = {
  selection: { width: 420, height: 320, focusOnShow: false, closeOnBlur: false },
  clipboard: { width: 380, height: 560, focusOnShow: true, closeOnBlur: true }
}
```

- [ ] **Step 2: Gate visibility on renderer readiness**

```js
activate(payload) {
  this.pendingActivation = payload
  this.rendererReady = false
  this.window.webContents.send('overlay:activate', payload)
}

markRendererReady() {
  this.rendererReady = true
  if (!this.window.isVisible()) {
    this.showPreparedWindow()
  }
}
```

- [ ] **Step 3: Enforce per-mode focus and blur handling**

```js
this.window.on('blur', () => {
  if (this.pinned) return
  if (this.mode !== OVERLAY_MODE.clipboard) return
  this.hide('blur')
})
```

- [ ] **Step 4: Remove all backdrop/dismiss-window behavior from the active path**

```js
// No createDismissWindows(), scheduleDismissWindows(), or backdrop pointer routing.
// Close behavior comes only from blur policy or explicit renderer commands.
```

- [ ] **Step 5: Re-run tests and verify the new focus model**

Run: `uv run python -m pytest tests -q`
Expected: PASS with blur and ready-timing assertions.


### Task 4: Port Selection Behavior Into the Unified Panel

**Files:**
- Modify: `src/components/overlay/SelectionPanel.vue`
- Modify: `src/composables/useElectron.js`
- Modify: `electron/main.js`

- [ ] **Step 1: Move the four existing actions into `SelectionPanel`**

```js
const actions = [
  { value: 'translate', label: '翻译' },
  { value: 'explain', label: '解释' },
  { value: 'summarize', label: '总结' },
  { value: 'rewrite', label: '改写' }
]
```

- [ ] **Step 2: Preserve current API integration but emit a unified completion signal**

```js
async function runAction(actionType) {
  // 调用现有 api.runAITranslate / Explain / Summarize / Rewrite
  emit('action-complete', { actionType, pinned })
}
```

- [ ] **Step 3: Implement the confirmed close rule**

```js
function handleSelectionActionComplete() {
  if (isPinned.value) {
    return
  }
  closeOverlay()
}
```

- [ ] **Step 4: Keep `selection` non-focus-invasive**

```js
// 不在 selection panel 内主动调用 search focus 或 window focus 相关逻辑
```

- [ ] **Step 5: Run build and tests**

Run: `npm run build`
Expected: PASS

Run: `uv run python -m pytest tests -q`
Expected: PASS


### Task 5: Port Clipboard Behavior Into the Unified Panel

**Files:**
- Modify: `src/components/overlay/ClipboardPanel.vue`
- Modify: `src/composables/useElectron.js`
- Modify: `electron/main.js`

- [ ] **Step 1: Move clipboard search/list logic into `ClipboardPanel`**

```js
onOverlayActivate((payload) => {
  if (payload.mode !== 'clipboard') return
  refreshOverlay(payload)
})
```

- [ ] **Step 2: Remove double-refresh on initial mount**

```js
onMounted(() => {
  onOverlayActivate(handleActivation)
  onClipboardChanged(() => {
    if (mode.value === 'clipboard') {
      refreshOverlay({ focusSearch: false })
    }
  })
})
```

- [ ] **Step 3: Preserve keyboard navigation and copy / paste behavior**

```js
if (event.key === 'Enter') {
  event.preventDefault()
  applyItem(activeItem.value)
}
```

- [ ] **Step 4: Keep blur-close entirely in the main process**

```js
// renderer 不自行实现 backdrop 关闭，只发送显式 close/pin/drag 事件
```

- [ ] **Step 5: Run build and tests**

Run: `npm run build`
Expected: PASS

Run: `uv run python -m pytest tests -q`
Expected: PASS


### Task 6: Remove Legacy Entrypoints and Old Managers

**Files:**
- Delete: `electron/clipboardOverlay/backdropPreload.js`
- Delete: `electron/clipboardOverlay/index.js`
- Delete: `electron/selectionToolbar/backdropPreload.js`
- Delete: `electron/selectionToolbar/index.js`
- Delete: `src/clipboard-overlay.html`
- Delete: `src/clipboard-overlay.js`
- Delete: `src/selection-toolbar.html`
- Delete: `src/selection-toolbar.js`
- Modify: `vite.config.js`
- Modify: `electron/main.js`
- Modify: `electron/preload.js`
- Test: `tests/test_selection_toolbar_manager.js`

- [ ] **Step 1: Replace old imports and window manager wiring with the new unified manager**

```js
const { UnifiedOverlayManager } = require('./overlay')
let overlayManager = null
```

- [ ] **Step 2: Delete obsolete tests and add unified replacements**

```bash
git rm tests/test_selection_toolbar_manager.js
```

- [ ] **Step 3: Remove the old Vite entries**

```js
// 删除 clipboardOverlay / selectionToolbar entries，只保留 unifiedOverlay
```

- [ ] **Step 4: Run the full validation suite**

Run: `uv run python -m pytest tests -q`
Expected: PASS

Run: `npm run build`
Expected: PASS

- [ ] **Step 5: Manual smoke-check the Electron dev flow**

Run: `npm run electron:dev`
Expected: app launches, `selection` and `clipboard` both open through the unified overlay path without extra dismiss windows.
