# UI 改进实施计划

## Context

本项目是一个 Electron + Vue 3 + FastAPI 的音频转文字应用，采用无框窗口设计。用户提出了 11 个 UI 改进需求，涵盖布局、交互、样式和品牌等多个方面。本计划按优先级和依赖关系组织任务，确保改进有序进行。

---

## Phase 1: 布局核心改动

### 任务 1.1: Header 重设计与侧边栏整合

**目标:** 将 "Audio To Text" 标题整合到侧边栏头部，并支持 Electron 拖拽。完全移除原来的 Header 组件。

**涉及文件:**
- `src/components/layout/Sidebar.vue`
- `src/App.vue` - 移除 Header 组件引用
- `src/styles/main.css`

**修改内容:**
1. Sidebar.vue: 将 logo-icon 文字从 "AT" 改为 "ATT"
2. App.vue: 移除 Header 组件的导入和使用
3. main.css: 确保 sidebar-header 拖拽区域正常工作
4. 完全移除副标题「桌面录音·实时转写·提示词优化」

**验证:**
- 启动应用，确认侧边栏显示 "ATT" 和 "Audio To Text"
- 确认窗口可通过侧边栏头部拖拽移动
- 确认首页不再显示独立的 Header 区域

---

### 任务 1.2: 布局与宽度问题

**目标:** 解决无历史记录时宽度不够问题，实现动态布局适应 Electron 窗口大小。

**涉及文件:**
- `src/styles/main.css`

**修改内容:**
1. 将 `.container` 的 `max-width: 880px` 改为动态计算
2. 添加 `min-width` 确保最小宽度
3. 配合侧边栏展开/折叠状态调整

```css
.container {
    width: 100%;
    min-width: 400px;
    max-width: calc(100vw - 260px); /* 220px sidebar + 40px padding */
}

.main-content.sidebar-collapsed .container {
    max-width: calc(100vw - 100px);
}
```

**验证:**
- 无历史记录时检查首页宽度
- 调整窗口大小测试响应式

---

### 任务 1.3: 首页和设置布局对齐

**目标:** 统一首页和设置页面的布局结构。

**涉及文件:**
- `src/App.vue`
- `src/components/settings/SettingsPage.vue`
- `src/styles/main.css`

**修改内容:**
1. App.vue: 为设置页面也添加 `.container` 包裹
2. SettingsPage.vue: 移除外层 `.settings-page` 容器的 margin-left 逻辑
3. main.css: 统一 `.container` 样式

**验证:**
- 切换首页和设置页面，确认布局一致
- 测试侧边栏折叠状态下的布局

---

## Phase 2: 交互改进

### 任务 2.1: 记录卡片按钮简化

**目标:** 简化按钮，实现左键复制、右键编辑。

**涉及文件:**
- `src/components/home/RecordCard.vue`
- `src/styles/main.css`

**修改内容:**
1. 移除"优化"、"导出"、"删除"按钮
2. 保留"编辑"和"复制"按钮（编辑在前）
3. 左键点击 → 触发复制
4. 右键点击 → 触发编辑

```vue
<!-- 按钮区域 -->
<div class="record-actions">
    <button class="btn-record-action" @click.stop="$emit('edit')">编辑</button>
    <button class="btn-record-action btn-copy" @click.stop="$emit('copy')">复制</button>
</div>

<!-- 点击处理 -->
@click="handleClick"      // 左键 → copy
@contextmenu="handleRightClick"  // 右键 → edit
```

**验证:**
- 左键点击记录项，确认复制成功
- 右键点击记录项，确认进入编辑模式

---

### 任务 2.2: 状态显示调整

**目标:** 优化状态不显示在顶部状态栏，只在记录卡片显示。

**涉及文件:**
- `src/components/home/RecorderSection.vue`

**修改内容:**
1. RecorderSection.vue: 移除"优化中"状态显示
2. 保留"准备就绪"、"录音中"、"转写中"状态
3. RecordCard.vue 已有 "optimizing" 徽章显示，无需修改

**验证:**
- 录音后自动优化时，顶部状态栏不显示优化状态
- 记录卡片显示"优化中"徽章

---

### 任务 2.3: 编辑记录弹窗加宽

**目标:** 将编辑弹窗宽度从 600px 增加到 800px。

**涉及文件:**
- `src/styles/main.css`

**修改内容:**
```css
#recordEditModal .modal-content {
    max-width: 800px;
    width: 90%;
}
```

**验证:**
- 打开编辑弹窗，确认宽度合适

---

## Phase 3: UI 组件优化

### 任务 3.1: 下拉选择框样式重设计

**目标:** 将 select 下拉框样式现代化。

**涉及文件:**
- `src/styles/main.css`

**修改内容:**
1. 添加自定义下拉箭头（SVG）
2. 改进 hover 和 focus 状态
3. 统一 `.setting-select` 和 `.form-select` 样式

```css
.setting-select,
.form-select {
    padding: var(--space-sm) var(--space-lg) var(--space-sm) var(--space-md);
    background-image: url("data:image/svg+xml,...");
    background-repeat: no-repeat;
    background-position: right 12px center;
    padding-right: 36px;
    border-radius: var(--radius-md);
}
```

**验证:**
- 检查所有下拉框样式是否统一
- 测试 hover 和 focus 效果

---

### 任务 3.2: LLM 配置 UI 简化

**目标:** 简化 LLM 配置管理流程。

**涉及文件:**
- `src/components/settings/SettingsPage.vue`
- `src/components/modals/LLMModal.vue`
- `src/components/modals/LLMEditModal.vue`
- `src/components/settings/LLMConfigManager.vue`

**修改内容:**

1. **SettingsPage.vue:**
   - 移除"LLM 服务商"设置项（第 171-180 行）
   - 在默认配置下拉框旁添加"管理配置"按钮

2. **LLMModal.vue:**
   - 移除 footer 中的关闭按钮（用户可用右上角 × 关闭）

3. **LLMEditModal.vue:**
   - 移除删除按钮（上级页面已有）
   - 移除测试连接按钮

4. **LLMConfigManager.vue:**
   - 将"编辑"、"删除"按钮改为图标按钮
   - 移除"测试"、"设默认"按钮（简化操作）

**验证:**
- 确认设置页面 LLM 配置项简化
- 测试配置管理流程完整可用

---

## Phase 4: 视觉和品牌

### 任务 4.1: 图标和品牌统一

**目标:** 为导航添加 SVG 线性图标，更新 Logo 为 ATT。

**涉及文件:**
- `src/components/layout/Sidebar.vue`
- `src/styles/main.css`

**修改内容:**
1. Sidebar.vue: 将 "录" 和 "设" 文字替换为 SVG 线性图标
2. Logo 显示 "ATT"（已在任务 1.1 完成）
3. 更新 `.nav-icon` 样式支持 SVG

**SVG 图标设计:**

首页图标（麦克风/录音）:
```html
<svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
    <path d="M12 1a3 3 0 0 0-3 3v8a3 3 0 0 0 6 0V4a3 3 0 0 0-3-3z"/>
    <path d="M19 10v2a7 7 0 0 1-14 0v-2"/>
    <line x1="12" y1="19" x2="12" y2="23"/>
    <line x1="8" y1="23" x2="16" y2="23"/>
</svg>
```

设置图标（齿轮）:
```html
<svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
    <circle cx="12" cy="12" r="3"/>
    <path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 0 1 0 2.83 2 2 0 0 1-2.83 0l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-2 2 2 2 0 0 1-2-2v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 0 1-2.83 0 2 2 0 0 1 0-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1-2-2 2 2 0 0 1 2-2h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 0 1 0-2.83 2 2 0 0 1 2.83 0l.06.06a1.65 1.65 0 0 0 1.82.33H9a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 2-2 2 2 0 0 1 2 2v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 0 1 2.83 0 2 2 0 0 1 0 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82V9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 2 2 2 2 0 0 1-2 2h-.09a1.65 1.65 0 0 0-1.51 1z"/>
</svg>
```

**验证:**
- 确认侧边栏显示 SVG 线性图标
- 测试折叠状态下的图标显示
- 确认图标颜色随状态变化（hover/active）

---

### 任务 4.2: 托盘图标重设计

**目标:** 改为圆形黑色图标，录音时变为红色。

**涉及文件:**
- `electron/tray/index.js`

**修改内容:**
修改 `createDefaultIcon` 方法，绘制圆形图标:

```javascript
createDefaultIcon(iconType) {
    const size = 22
    const canvas = Buffer.alloc(size * size * 4)

    const colors = {
        idle: [30, 30, 30, 255],        // 黑色
        recording: [248, 113, 113, 255], // 红色
        transcribing: [30, 30, 30, 255], // 黑色
        disconnected: [30, 30, 30, 128]  // 半透明
    }

    // 绘制圆形
    for (let y = 0; y < size; y++) {
        for (let x = 0; x < size; x++) {
            const idx = (y * size + x) * 4
            const distance = Math.sqrt(Math.pow(x - center, 2) + Math.pow(y - center, 2))
            if (distance <= radius) {
                // 填充颜色
            }
        }
    }
}
```

**验证:**
- 启动应用检查托盘图标
- 录音时确认图标变为红色

---

## Phase 5: Bug 修复

### 任务 5.1: 测试连接功能修复

**目标:** 修复 LLM 测试连接功能。

**涉及文件:**
- `app/llm/gateway.py` 或相关 provider 文件
- `app/api/routes.py`

**问题分析:**
当前实现使用 `models.list()` 测试连接，但某些 API 可能不支持此端点。

**修改内容:**
1. 改进 `test_connection` 方法，使用最小化测试请求
2. 添加详细错误信息返回

```python
async def test_connection(self) -> dict:
    try:
        response = await self.client.chat.completions.create(
            model=self.config.model,
            messages=[{"role": "user", "content": "Hi"}],
            max_tokens=5
        )
        return {"success": True, "error": None}
    except Exception as e:
        return {"success": False, "error": str(e)}
```

**验证:**
- 配置正确的 API Key，测试连接成功
- 配置错误的 API Key，显示错误信息

---

## 关键文件清单

| 文件路径 | 用途 |
|---------|------|
| `src/components/layout/Sidebar.vue` | 侧边栏布局、Logo、导航图标 |
| `src/styles/main.css` | 全局样式、容器、按钮、下拉框 |
| `src/components/home/RecordCard.vue` | 记录卡片交互（左键/右键） |
| `src/components/settings/SettingsPage.vue` | 设置页面布局、LLM 配置项简化 |
| `electron/tray/index.js` | 托盘图标生成 |

---

## 验证清单

- [ ] 侧边栏显示 "ATT" Logo 和导航图标
- [ ] 窗口可通过侧边栏头部拖拽
- [ ] 首页无记录时宽度正常
- [ ] 首页和设置页面布局对齐
- [ ] 记录项左键复制、右键编辑
- [ ] 编辑弹窗宽度合适
- [ ] 下拉框样式现代化
- [ ] LLM 配置流程简化
- [ ] 托盘图标为圆形，录音时红色
- [ ] 测试连接功能正常
