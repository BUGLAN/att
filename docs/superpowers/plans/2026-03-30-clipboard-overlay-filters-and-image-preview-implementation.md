# Clipboard Overlay Filters And Image Preview Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 为统一剪切板悬浮框补齐顶部快速筛选、右键收藏、图片悬停预览，并修复主界面图片条目显示与图片复制回剪切板能力。

**Architecture:** 后端新增图片读取接口，前端统一通过后端 URL 访问图片资源。共享的剪切板展示辅助逻辑收敛到独立工具文件，悬浮框与主界面卡片复用同一套名称、类型和图片预览规则。

**Tech Stack:** FastAPI、Electron、Vue 3、Vite、Node `assert` 测试、Pytest

---

### Task 1: 补齐后端图片读取能力

**Files:**
- Modify: `app/clipboard/manager.py`
- Modify: `app/clipboard/routes.py`
- Test: `tests/test_clipboard_manager.py`

- [ ] **Step 1: 先写失败测试，覆盖图片路径解析与图片筛选**

```python
item = await manager.create_item(
    content="image:test",
    source="test",
    item_type=ClipboardItemType.IMAGE,
    title="测试图片",
    summary="32 x 16",
    metadata={"image_path": str(image_path), "file_name": "demo.png"},
)

resolved_path = await manager.get_item_image_path(item.id)
assert resolved_path == image_path

image_items = await manager.list_items(item_type=ClipboardItemType.IMAGE)
assert [value.id for value in image_items] == [item.id]
```

- [ ] **Step 2: 运行测试，确认当前失败**

Run: `uv run python -m pytest tests/test_clipboard_manager.py -q`

Expected: 因 `get_item_image_path` 不存在或断言失败而报红。

- [ ] **Step 3: 在管理器和路由中补齐最小实现**

```python
async def get_item_image_path(self, item_id: str) -> Optional[Path]:
    item = await self.get_item(item_id)
    if not item or item.item_type != ClipboardItemType.IMAGE:
        return None

    image_path = Path((item.metadata or {}).get("image_path", ""))
    if not image_path or not image_path.exists():
        return None
    return image_path
```

```python
@router.get("/items/{item_id}/image")
async def get_clipboard_item_image(item_id: str):
    image_path = await clipboard_manager.get_item_image_path(item_id)
    if not image_path:
        raise HTTPException(status_code=404, detail="Clipboard image not found")
    return FileResponse(str(image_path))
```

- [ ] **Step 4: 重新运行测试，确认通过**

Run: `uv run python -m pytest tests/test_clipboard_manager.py -q`

Expected: `1 passed` 或更高，通过且无新增失败。

### Task 2: 补齐 Electron 图片写回系统剪切板能力

**Files:**
- Modify: `electron/main.js`
- Modify: `electron/preload.js`
- Modify: `electron/clipboardMonitor/index.js`

- [ ] **Step 1: 先补接口契约，确保渲染层可以请求写入图片**

```js
writeClipboardImage: (payload) => ipcRenderer.invoke('clipboard:write-image', payload)
```

- [ ] **Step 2: 在主进程中增加最小图片写入实现**

```js
ipcMain.handle('clipboard:write-image', (event, payload = {}) => {
  const imagePath = String(payload.imagePath || '').trim()
  const imageToken = String(payload.imageToken || '').trim()

  if (!imagePath) {
    return false
  }

  if (imageToken) {
    clipboardMonitor?.markInternalWrite(imageToken, 'renderer_write_image')
  }

  clipboard.writeImage(nativeImage.createFromPath(imagePath))
  return true
})
```

- [ ] **Step 3: 保持图片监听去重语义**

```js
const imageContent = `image:${imageHash}`
```

渲染层回写图片时复用同样的 `image:<hash>` 令牌，避免重新采集。

- [ ] **Step 4: 运行现有 Node 测试，确认没有破坏统一悬浮层主流程**

Run: `node tests/test_unified_overlay_manager.js`

Expected: 退出码 `0`，终端无断言失败。

### Task 3: 收敛前端剪切板展示与图片资产逻辑

**Files:**
- Create: `src/utils/clipboardPresentation.js`
- Modify: `src/api/client.js`
- Modify: `src/composables/useElectron.js`
- Modify: `src/composables/useClipboard.js`

- [ ] **Step 1: 新建共享工具文件，集中管理筛选和图片展示辅助逻辑**

```js
export const OVERLAY_CLIPBOARD_FILTERS = [
  { value: 'all', label: '全部' },
  { value: 'favorite', label: '收藏' },
  { value: 'text', label: '文本' },
  { value: 'image', label: '图片' },
  { value: 'file', label: '文件' },
  { value: 'link', label: '链接' }
]
```

```js
export function buildClipboardQueryParams(filterValue, query, limit = 30) {
  const params = { limit }
  if (query) params.query = query
  if (filterValue === 'favorite') params.favorite_only = true
  if (['text', 'image', 'file', 'link'].includes(filterValue)) params.item_type = filterValue
  return params
}
```

- [ ] **Step 2: 在 API 层增加图片 URL 生成方法**

```js
async getClipboardItemImageUrl(id, version = '') {
  const baseUrl = await getBaseUrl()
  const suffix = version ? `?v=${encodeURIComponent(version)}` : ''
  return `${baseUrl}/clipboard/items/${id}/image${suffix}`
}
```

- [ ] **Step 3: 在 Electron composable 中补齐图片写入方法**

```js
writeImage: async (payload) => {
  if (!globalThis.window?.electronAPI?.writeClipboardImage) {
    throw new Error('Image clipboard API unavailable')
  }
  return globalThis.window.electronAPI.writeClipboardImage(payload)
}
```

- [ ] **Step 4: 在 `useClipboard` 中统一处理图片条目复制与 URL 注入**

```js
if (item.item_type === 'image' && item.metadata?.image_path) {
  await clipboard.writeImage({
    imagePath: item.metadata.image_path,
    imageToken: item.content || ''
  })
  return
}
```

- [ ] **Step 5: 运行构建前的静态验证**

Run: `npm run build`

Expected: Vite 构建通过，无语法错误。

### Task 4: 实现悬浮框筛选、右键收藏和图片悬停预览

**Files:**
- Modify: `src/components/overlay/ClipboardPanel.vue`
- Modify: `src/styles/clipboard-overlay.css`

- [ ] **Step 1: 先按共享筛选配置重构查询参数**

```js
const selectedFilter = ref('all')

items.value = await enrichClipboardItems(await api.getClipboardItems(
  buildClipboardQueryParams(selectedFilter.value, query.value.trim(), 30)
))
```

- [ ] **Step 2: 在模板中加入顶部紧凑筛选条**

```vue
<div class="overlay-filter-row">
  <button
    v-for="filter in OVERLAY_CLIPBOARD_FILTERS"
    :key="filter.value"
    class="overlay-filter-button"
    :class="{ active: selectedFilter === filter.value }"
  >
    {{ filter.label }}
  </button>
</div>
```

- [ ] **Step 3: 给列表项补齐收藏图标、右键收藏和图片预览浮层**

```vue
<div
  class="overlay-item"
  @contextmenu.prevent="toggleFavorite(item)"
  @mouseenter="handleItemHover(item, $event)"
  @mouseleave="clearHoveredPreview"
>
```

```vue
<span class="overlay-item-favorite" :class="{ active: item.is_favorite }">★</span>
```

```vue
<div v-if="hoveredPreview" class="overlay-image-preview">
  <img :src="hoveredPreview.preview_url" :alt="hoveredPreview.title">
</div>
```

- [ ] **Step 4: 用细按钮和预览卡样式完成视觉收口**

```css
.overlay-filter-button {
  min-height: 28px;
  padding: 0 10px;
  border-radius: 999px;
}
```

- [ ] **Step 5: 运行构建验证**

Run: `npm run build`

Expected: 构建通过，样式和模板编译无报错。

### Task 5: 修复主界面图片卡片展示

**Files:**
- Modify: `src/components/clipboard/ClipboardCard.vue`
- Modify: `src/components/clipboard/ClipboardPage.vue`

- [ ] **Step 1: 在主界面筛选中补齐图片入口并修正文案**

```js
const filters = [
  { value: 'all', label: '全部内容' },
  { value: 'text', label: '纯文本' },
  { value: 'image', label: '图片' },
  { value: 'file', label: '文件' },
  { value: 'link', label: '链接' }
]
```

- [ ] **Step 2: 让图片卡片显示图片名和悬停预览**

```vue
<div v-if="isImageItem" class="clipboard-card-image-state">
  <span>{{ imageMetaText }}</span>
</div>
<div v-if="isImageItem" class="clipboard-card-image-hover">
  <img :src="item.preview_url" :alt="displayTitle">
</div>
```

- [ ] **Step 3: 让卡片复制逻辑对图片走 Electron 图片写回**

```js
await copyItemContent(item)
```

沿用 Task 3 中统一过的复制实现，不在卡片内重复分支。

- [ ] **Step 4: 再次运行构建验证**

Run: `npm run build`

Expected: 构建继续通过。

### Task 6: 回归验证

**Files:**
- Modify: `tests/test_clipboard_manager.py`

- [ ] **Step 1: 运行 Python 回归测试**

Run: `uv run python -m pytest tests/test_clipboard_manager.py -q`

Expected: 全部通过。

- [ ] **Step 2: 运行 Node 回归测试**

Run: `node tests/test_unified_overlay_manager.js`

Expected: 退出码 `0`。

- [ ] **Step 3: 运行最终前端构建**

Run: `npm run build`

Expected: Vite 构建成功。

- [ ] **Step 4: 手工验证清单**

```text
1. 打开统一悬浮层剪切板模式，确认搜索栏上方出现全部/收藏/文本/图片/文件/链接按钮。
2. 切换到图片筛选，确认图片条目只显示图片名和摘要。
3. 右键任意条目，确认收藏星标即时切换。
4. 悬停图片条目，确认能看到清晰缩略图。
5. 在主界面剪切板页查看图片卡片，确认名称和悬停预览正常。
6. 选中图片条目后，确认系统剪切板恢复为图片而不是文本哈希。
```
