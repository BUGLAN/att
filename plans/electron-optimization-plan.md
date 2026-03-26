# Electron 优化实现计划

## 背景

对 Electron 应用进行多项优化，包括 UI 改进、新功能添加和 Bug 修复。

---

## 1. 自定义标题栏 Header

**目标**: 在 AudioToText 右侧布局顶部添加可拖拽的 Header，包含窗口控制按钮。

### 实现方案

#### 1.1 创建 TitleBar 组件
**新建文件**: `src/components/layout/TitleBar.vue`

```vue
<template>
  <div class="title-bar" :class="{ collapsed: sidebarCollapsed }">
    <div class="title-bar-drag">
      <!-- 可拖拽区域 -->
    </div>
    <div class="title-bar-buttons">
      <button class="title-bar-btn minimize" @click="minimize">
        <svg>...</svg>
      </button>
      <button class="title-bar-btn maximize" @click="maximize">
        <svg>...</svg>
      </button>
      <button class="title-bar-btn close" @click="close">
        <svg>...</svg>
      </button>
    </div>
  </div>
</template>
```

#### 1.2 修改 App.vue 布局
**文件**: `src/App.vue`

- 在 `.main-content` 顶部插入 `<TitleBar />` 组件
- 调整页面内容区域的 padding-top 以容纳标题栏
- 标题栏需要感知 sidebar 的折叠状态

#### 1.3 CSS 关键点
**文件**: `src/styles/main.css`

```css
.title-bar {
  position: fixed;
  top: 0;
  left: 220px; /* sidebar 宽度 */
  right: 0;
  height: 32px;
  background: var(--bg-secondary);
  border-bottom: 1px solid var(--border);
  -webkit-app-region: drag; /* 可拖拽 */
  z-index: 50;
}

.title-bar.collapsed {
  left: 60px; /* 折叠后的 sidebar 宽度 */
}

.title-bar-buttons {
  -webkit-app-region: no-drag; /* 按钮不可拖拽 */
}
```

### 关键文件
- `src/components/layout/TitleBar.vue` (新建)
- `src/App.vue`
- `src/styles/main.css`

---

## 2. 设置页面添加记录集

**目标**: 在设置页面添加"记录集"部分，支持查看、筛选和搜索录音记录。

### 实现方案

#### 2.1 后端：添加 is_favorite 字段
**文件**: `app/records/models.py`

```python
@dataclass
class Record:
    # ... 现有字段
    is_favorite: bool = False  # 是否收藏
```

**文件**: `app/records/manager.py`

更新 `update_record` 方法以支持 `is_favorite` 参数。

#### 2.2 后端：添加搜索 API
**文件**: `app/api/routes.py`

```python
@router.get("/records/search")
async def search_records(
    q: Optional[str] = None,
    search_type: str = "all",  # "raw", "optimized", "all"
    favorite_only: bool = False,
    limit: int = 100
):
    """搜索录音记录"""
```

#### 2.3 前端：设置页面记录集组件
**文件**: `src/components/settings/SettingsPage.vue`

新增记录集设置组：
- 搜索输入框
- 筛选条件（搜索类型：全部/转写文本/优化文本，仅收藏）
- 记录列表（使用紧凑型卡片）

**新建文件**: `src/components/settings/SettingsRecordCard.vue`

紧凑型记录卡片，显示文本摘要、时间、收藏状态。

### 关键文件
- `app/records/models.py`
- `app/records/manager.py`
- `app/api/routes.py`
- `src/components/settings/SettingsPage.vue`
- `src/components/settings/SettingsRecordCard.vue` (新建)

---

## 3. 历史记录添加收藏功能

**目标**: 在历史记录卡片的复制按钮后添加收藏图标。

### 实现方案

#### 3.1 更新 RecordCard 组件
**文件**: `src/components/home/RecordCard.vue`

```vue
<!-- 复制按钮后添加 -->
<button
  v-if="!selectMode"
  class="btn-favorite-inline"
  :class="{ favorited: record.is_favorite }"
  @click.stop="$emit('toggle-favorite')"
>
  {{ record.is_favorite ? '★' : '☆' }}
</button>
```

#### 3.2 更新事件处理
**文件**: `src/components/home/RecordsSection.vue`

添加 `toggle-favorite` 事件处理器。

**文件**: `src/composables/useRecords.js`

添加 `toggleFavorite` 函数调用 API 更新收藏状态。

#### 3.3 CSS 样式
**文件**: `src/styles/main.css`

```css
.btn-favorite-inline {
  background: transparent;
  border: 1px solid var(--border);
  color: var(--text-muted);
}

.btn-favorite-inline.favorited {
  color: var(--accent);
  border-color: var(--accent);
}
```

### 关键文件
- `src/components/home/RecordCard.vue`
- `src/components/home/RecordsSection.vue`
- `src/composables/useRecords.js`
- `src/styles/main.css`

---

## 4. 修复自动复制粘贴重复 Bug

**问题根因**: Electron 渲染器 (`useRecorder.js`) 和独立托盘应用 (`tray/main.py`) 同时监听 WebSocket 消息并独立触发复制粘贴操作。

### 实现方案

#### 4.1 WebSocket 消息添加来源标识
**文件**: `app/services/websocket.py`

```python
def get_active_web_client(self) -> Optional[str]:
    """检查是否有 web 客户端（Electron 应用）连接"""
    return next(iter(self.get_web_clients()), None)
```

**文件**: `app/api/routes.py`

在广播消息中添加 `electron_active` 字段：

```python
electron_active = ws_manager.get_active_web_client() is not None

await ws_manager.broadcast({
    "type": "transcription_completed",
    "data": {
        "record_id": record_id,
        "text": result.text,
        "electron_active": electron_active  # 新增
    }
})
```

#### 4.2 托盘应用跳过复制粘贴
**文件**: `tray/main.py`

```python
@Slot(dict)
def _on_transcription_completed(self, data: dict):
    self._refresh_history()

    # 如果 Electron 应用活跃，跳过托盘的复制粘贴
    if data.get('electron_active', False):
        logger.info("Electron active, skipping tray copy/paste")
        return

    # 正常的复制粘贴逻辑
    if self.auto_copy_to_clipboard and not self.auto_optimize:
        text = data.get('text', '')
        if text:
            self._copy_and_paste(text)
```

同样修改 `_on_optimization_completed`。

### 关键文件
- `app/services/websocket.py`
- `app/api/routes.py`
- `tray/main.py`

---

## 5. 图标设计

**目标**:
- App 图标：使用 ATT 黄色文字设计
- 托盘图标：高清，默认黄色，录音时为暖橙红色（非纯红）

### 实现方案

#### 5.1 托盘图标颜色
**文件**: `tray/main.py`

修改 `_generate_icons()` 方法：

```python
# 黄色强调色
yellow = (201, 162, 39, 255)  # --accent: #c9a227

# 录音状态：暖橙红色（非纯红）
recording_color = (230, 126, 34, 255)  # 温暖的橙红色
```

#### 5.2 App 图标
**新建文件**: `electron/icons/`

创建 ATT 黄色文字风格的图标文件：
- `icon.ico` (Windows)
- `icon.icns` (macOS)
- `icon.png` (Linux)

**文件**: `electron/main.js`

```javascript
mainWindow = new BrowserWindow({
  // ...
  icon: path.join(__dirname, 'icons', 'icon.png')
})
```

### 关键文件
- `tray/main.py`
- `electron/icons/` (新建)
- `electron/main.js`

---

## 实现顺序

| 阶段 | 任务 | 预估时间 |
|------|------|----------|
| 1 | 后端 is_favorite 字段 + 搜索 API | 0.5 天 |
| 2 | 修复重复粘贴 Bug | 0.5 天 |
| 3 | 自定义标题栏 | 0.5 天 |
| 4 | 收藏功能 | 0.5 天 |
| 5 | 设置页记录集 | 0.5 天 |
| 6 | 图标设计 | 0.5 天 |

---

## 验证步骤

### 标题栏
- [ ] 窗口可通过标题栏拖拽
- [ ] 最小化/最大化/关闭按钮正常工作
- [ ] sidebar 折叠时标题栏位置正确调整

### 记录集
- [ ] 设置页面显示记录集
- [ ] 搜索返回正确结果
- [ ] 筛选条件正常工作
- [ ] 收藏状态持久化

### 收藏功能
- [ ] 星标图标切换正常
- [ ] 收藏状态保存到后端
- [ ] 首页和设置页状态同步

### 重复粘贴修复
- [ ] Electron 应用运行时仅粘贴一次
- [ ] 独立托盘应用粘贴正常
- [ ] 日志确认正确行为

### 图标
- [ ] App 图标在任务栏/桌面显示正确
- [ ] 托盘图标状态切换正确
- [ ] 录音图标为暖橙红色（非纯红）
