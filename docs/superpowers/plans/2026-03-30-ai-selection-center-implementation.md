# AI 划词动作中心 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 为 ATT 增加一条可运行的 AI 划词动作中心 MVP 链路，覆盖桌面选区捕获回退、AI 动作 API、生词本落盘、主应用词库页面与基础设置项。

**Architecture:** Electron 负责桌面选区检测、选区回退捕获与浮层窗口；FastAPI 负责 AI 动作编排与生词本管理；Vue 负责词库页面、动作结果卡片与设置项。翻译、解释、总结、改写统一走现有 LLM Gateway，不新增独立翻译模型。

**Tech Stack:** Electron 29、Vue 3、Pinia、FastAPI、pytest、Node test、现有 LLM Gateway、本地 JSON 存储。

---

## 文件结构

- Create: `app/ai_actions/models.py`
- Create: `app/ai_actions/service.py`
- Create: `app/ai_actions/routes.py`
- Create: `app/wordbook/models.py`
- Create: `app/wordbook/manager.py`
- Create: `app/wordbook/routes.py`
- Create: `tests/test_ai_actions_service.py`
- Create: `tests/test_wordbook_manager.py`
- Create: `tests/test_selection_toolbar_manager.js`
- Create: `electron/python/selection_capture_helper.py`
- Create: `electron/selectionToolbar/index.js`
- Create: `src/components/selection/SelectionToolbarWindow.vue`
- Create: `src/components/selection/SelectionResultCard.vue`
- Create: `src/components/wordbook/WordbookPage.vue`
- Modify: `app/main.py`
- Modify: `app/config.py`
- Modify: `src/api/client.js`
- Modify: `src/composables/useElectron.js`
- Modify: `src/components/layout/Sidebar.vue`
- Modify: `src/components/settings/SettingsPage.vue`
- Modify: `src/stores/settings.js`
- Modify: `src/stores/app.js`
- Modify: `src/App.vue`
- Modify: `electron/preload.js`
- Modify: `electron/input/listener.js`
- Modify: `electron/main.js`

### Task 1: 生词本领域层

**Files:**
- Create: `tests/test_wordbook_manager.py`
- Create: `app/wordbook/models.py`
- Create: `app/wordbook/manager.py`
- Create: `app/wordbook/routes.py`
- Modify: `app/main.py`

- [ ] **Step 1: 写出生词本管理器失败测试**

```python
async def test_capture_entry_merges_duplicate_queries(tmp_path):
    manager = WordbookManager(tmp_path)
    await manager.initialize()

    first = await manager.capture_entry(
        source_text="accurate",
        normalized_text="accurate",
        language="en",
        entry_type=WordbookEntryType.WORD,
        translation="准确的"
    )
    second = await manager.capture_entry(
        source_text="accurate",
        normalized_text="accurate",
        language="en",
        entry_type=WordbookEntryType.WORD,
        translation="准确的"
    )

    assert second.id == first.id
    assert second.query_count == 2
```

- [ ] **Step 2: 运行测试确认失败**

Run: `uv run python -m pytest tests/test_wordbook_manager.py -q`
Expected: FAIL，提示 `ModuleNotFoundError: No module named 'app.wordbook'`

- [ ] **Step 3: 实现最小生词本模型与管理器**

```python
@dataclass
class WordbookEntry:
    id: str = field(default_factory=lambda: str(uuid.uuid4()))
    source_text: str = ""
    normalized_text: str = ""
    language: str = "unknown"
    entry_type: WordbookEntryType = WordbookEntryType.WORD
    translation: str = ""
    query_count: int = 1
```

```python
async def capture_entry(...):
    duplicate = await self._find_duplicate_entry(normalized_text, language, entry_type)
    if duplicate:
        duplicate.query_count += 1
        ...
        return duplicate
```

- [ ] **Step 4: 暴露词库 API 并在主应用注册**

```python
router = APIRouter(prefix="/api/wordbook", tags=["wordbook"])

@router.get("")
async def list_wordbook_entries(...): ...

@router.post("")
async def create_wordbook_entry(request: WordbookCreateRequest): ...

@router.get("/stats")
async def get_wordbook_stats() -> dict: ...
```

- [ ] **Step 5: 运行测试确认通过**

Run: `uv run python -m pytest tests/test_wordbook_manager.py -q`
Expected: PASS

### Task 2: AI 动作服务与路由

**Files:**
- Create: `tests/test_ai_actions_service.py`
- Create: `app/ai_actions/models.py`
- Create: `app/ai_actions/service.py`
- Create: `app/ai_actions/routes.py`
- Modify: `app/main.py`

- [ ] **Step 1: 先写 AI 动作服务失败测试**

```python
async def test_translate_action_uses_gateway_and_persists_wordbook(monkeypatch, tmp_path):
    provider = StubProvider("准确的；精确的")
    service = AIActionService(
        llm_gateway=StubGateway(provider),
        wordbook_manager=wordbook_manager
    )

    result = await service.run_action(
        action_type=AIActionType.TRANSLATE,
        source_text="accurate",
        source_app="chrome.exe"
    )

    assert result.translation == "准确的；精确的"
    assert result.saved_to_wordbook is True
```

- [ ] **Step 2: 运行测试确认失败**

Run: `uv run python -m pytest tests/test_ai_actions_service.py -q`
Expected: FAIL，提示 `ModuleNotFoundError` 或缺少 `AIActionService`

- [ ] **Step 3: 实现统一的 LLM 动作服务**

```python
ACTION_PROMPTS = {
    AIActionType.TRANSLATE: "...返回 JSON，包含 translation、language、entry_type、explanations、examples ...",
    AIActionType.EXPLAIN: "...",
    AIActionType.SUMMARIZE: "...",
    AIActionType.REWRITE: "...",
}
```

```python
async def run_action(...):
    provider = self._require_default_provider()
    raw_output = await provider.optimize(source_text, prompt_template)
    parsed = self._parse_json_output(raw_output, action_type)
    if action_type == AIActionType.TRANSLATE:
        entry = await self.wordbook_manager.capture_entry(...)
```

- [ ] **Step 4: 暴露 AI 动作接口**

```python
@router.post("/translate")
async def translate_text(request: AIActionRequest) -> dict:
    return await ai_action_service.run_action_dict(AIActionType.TRANSLATE, request)
```

- [ ] **Step 5: 运行后端测试**

Run: `uv run python -m pytest tests/test_ai_actions_service.py tests/test_wordbook_manager.py -q`
Expected: PASS

### Task 3: Renderer 词库页与设置集成

**Files:**
- Create: `src/components/wordbook/WordbookPage.vue`
- Modify: `src/api/client.js`
- Modify: `src/composables/useElectron.js`
- Modify: `src/stores/settings.js`
- Modify: `src/stores/app.js`
- Modify: `src/components/layout/Sidebar.vue`
- Modify: `src/components/settings/SettingsPage.vue`
- Modify: `src/App.vue`

- [ ] **Step 1: 先补 API 调用与页面路由入口**

```javascript
getWordbookEntries: (params) => request('GET', `/wordbook${query ? `?${query}` : ''}`),
createWordbookEntry: (data) => request('POST', '/wordbook', { body: data }),
getWordbookStats: () => request('GET', '/wordbook/stats'),
runAITranslate: (data) => request('POST', '/ai-actions/translate', { body: data }),
runAIExplain: (data) => request('POST', '/ai-actions/explain', { body: data }),
```

- [ ] **Step 2: 添加词库页**

```vue
<WordbookPage v-show="currentPage === 'wordbook'" class="container page-view" />
```

```vue
<button class="nav-item" :class="{ active: currentPage === 'wordbook' }" @click="navigateTo('wordbook')">
  <span v-if="!collapsed" class="nav-text">生词本</span>
</button>
```

- [ ] **Step 3: 设置页增加 AI 动作中心配置**

```javascript
selection_toolbar_enabled: true,
selection_toolbar_auto_add_to_wordbook: true,
selection_toolbar_show_examples: true,
selection_toolbar_trigger_delay_ms: 220,
selection_toolbar_fallback_to_clipboard: true,
```

- [ ] **Step 4: 构建词库页面最小可用 UI**

```vue
<section v-else class="wordbook-grid">
  <article v-for="item in items" :key="item.id" class="wordbook-card">
    <strong>{{ item.normalized_text }}</strong>
    <p>{{ item.translation }}</p>
  </article>
</section>
```

- [ ] **Step 5: 运行前端构建验证**

Run: `npm run build`
Expected: exit 0

### Task 4: Electron 桌面入口与工具条浮层

**Files:**
- Create: `tests/test_selection_toolbar_manager.js`
- Create: `electron/python/selection_capture_helper.py`
- Create: `electron/selectionToolbar/index.js`
- Create: `src/components/selection/SelectionToolbarWindow.vue`
- Create: `src/components/selection/SelectionResultCard.vue`
- Modify: `electron/preload.js`
- Modify: `electron/input/listener.js`
- Modify: `electron/main.js`

- [ ] **Step 1: 写选择工具条管理器失败测试**

```javascript
test('showAtSelection sends activation payload to renderer', async () => {
  const manager = new SelectionToolbarManager({ BrowserWindow: FakeBrowserWindow, screen: fakeScreen })
  manager.showAtSelection({ text: 'accurate', point: { x: 100, y: 200 } })
  assert.equal(manager.window.webContents.sent[0].channel, 'selection-toolbar:activate')
})
```

- [ ] **Step 2: 运行 Node 测试确认失败**

Run: `node --test tests/test_selection_toolbar_manager.js`
Expected: FAIL，提示 `Cannot find module '../electron/selectionToolbar'`

- [ ] **Step 3: 实现工具条窗口管理器**

```javascript
class SelectionToolbarManager {
  showAtSelection(payload) {
    const window = this.createWindow()
    window.setBounds(this.calculateBounds(payload.point))
    this.pendingActivationPayload = payload
    ...
  }
}
```

- [ ] **Step 4: 实现 Windows 复制回退 helper**

```python
def capture_selected_text():
    previous_text = read_clipboard_text()
    send_ctrl_c()
    selected_text = wait_for_new_clipboard_text(previous_text)
    restore_clipboard_text(previous_text)
    return selected_text
```

- [ ] **Step 5: 把桌面输入监听接到工具条**

```javascript
onSelectionTrigger: (payload) => {
  selectionToolbarManager?.showAtSelection(payload)
}
```

```javascript
if (button === LEFT_MOUSE_BUTTON && this.selectionTracking.dragging) {
  this.onSelectionTrigger?.({ trigger: 'mouse_selection', point: { x: event.x, y: event.y } })
}
```

- [ ] **Step 6: 运行桌面入口测试与构建**

Run: `node --test tests/test_selection_toolbar_manager.js tests/test_desktop_input_listener.js`
Expected: PASS

### Task 5: 全链路验证

**Files:**
- Test: `tests/test_wordbook_manager.py`
- Test: `tests/test_ai_actions_service.py`
- Test: `tests/test_selection_toolbar_manager.js`
- Test: `tests/test_desktop_input_listener.js`

- [ ] **Step 1: 运行后端测试**

Run: `uv run python -m pytest tests/test_wordbook_manager.py tests/test_ai_actions_service.py -q`
Expected: PASS

- [ ] **Step 2: 运行 Node 测试**

Run: `node --test tests/test_selection_toolbar_manager.js tests/test_desktop_input_listener.js`
Expected: PASS

- [ ] **Step 3: 运行前端构建**

Run: `npm run build`
Expected: exit 0

- [ ] **Step 4: 记录未完成项**

```text
- 直接 UI Automation 选区读取仍未实现，仅交付复制回退路径
- 继续追问、OCR、Agent 办公动作不在本轮实现范围
```
