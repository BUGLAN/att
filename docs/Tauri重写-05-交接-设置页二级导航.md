# 交接：设置页改为二级导航（进行中）

> 设计稿：https://claude.ai/artifact/5iBqkchGCs8QEYQtPDejhk（主画板 1200×800 可点击，另有 800×600 最小窗口版）。
> 状态（2026-10-30）：分页组件、后端命令与设置页外壳 `SettingsPage.vue` 均已完成，自动化验收通过；**实机验收清单（见文末）尚未逐项走完**。代码已随本次设置页提交入库。
>
> 外壳补充：内容列宽 ≤520px 时（800px 窗口 + 展开的主侧栏），`.setting-item` 用容器查询改为上下排，开关仍在右侧。

## 已完成

| 文件 | 内容 |
| --- | --- |
| `src/components/settings/settingsNav.mjs` | 分组（通用 / 语音识别 / AI / 系统）、8 个分页元数据（id / label / desc / keywords）、`buildNavGroups(query)` 搜索、`resolveSectionId` |
| `tests/test_settings_nav.mjs` | 3 项，已通过 |
| `src/components/settings/settingsContext.js` | `SETTINGS_CTX`：父组件 provide、分页 inject |
| `src/components/settings/pages/*.vue` | `SettingsRecordingPage`、`SettingsHotkeyPage`、`SettingsOutputPage`、`SettingsAsrPage`（引擎卡片 + 只显示当前引擎配置 + 高级折叠 + 识别语言）、`SettingsLlmPage`、`SettingsPresetsPage`、`SettingsPermissionsPage`、`SettingsDiagnosticsPage` |
| `src/styles/main.css` | 新增 `.settings-stack` / `.choice-grid` / `.choice-card` / `.settings-note` / `.settings-advanced` |
| `src/stores/app.js` | `settingsSection` + `openSettings(section)`（转文字页「去设置」已改为 `openSettings('asr')`） |
| `src/composables/useElectron.js` | `getPermissionStatus` / `openPermissionSettings` / `getAppInfo` / `revealAppPath` |
| `src-tauri/src/permissions.rs` + `commands.rs` | 命令 `get_permission_status`（macOS：麦克风 / 输入监控 / 辅助功能，只查询不弹窗）、`open_permission_settings`、`get_app_info`、`reveal_app_path`；Cargo 加 `objc2 = "0.6"`（仅 macOS）；`cargo build` 已通过 |
| `LLMServiceSection.vue` | 新增 `showTitle` 属性；去掉过时的 LiteLLM 文案（向导弹窗同步） |

## 剩余工作：改 `SettingsPage.vue` 外壳

1. **脚本保持现有逻辑不动**（normalizeSettings / buildSettingsPayload / handleSave / 各 watch / hasPendingSettingsEdits 守卫 / Token 提交后清空），只做增量：
   - `activeSection`：初始取 `appStore.settingsSection` → `localStorage('att.settings.section')`（try/catch）→ `DEFAULT_SECTION_ID`，经 `resolveSectionId`；`watch(appStore.settingsSection)` 切页后清空该字段；切页时持久化，并 `stopHotkeyCapture()`。
   - `navQuery` + `navGroups = computed(() => buildNavGroups(navQuery.value))`；搜索框回车跳到第一个命中页。
   - `openPresetModal(preset?)`：有入参就打开该预设，否则用当前选中预设。
   - `goSection(id)`。
   - `provide(SETTINGS_CTX, { form, audioInputDevices, streamingRuntimeAvailable, streamingRuntimeSummary, builtinPresets, selectedPresetId, openPresetModal, hotkeyDisplay, isCapturingHotkey, startHotkeyCapture, resetHotkey, goSection })`——分页组件按这些名字 inject，缺一会报错。
   - 删掉外壳不再用的 import（LLMServiceSection、BaseBadge/BaseInput/BaseSelect/BaseButton、PageHeader）及 `showWhisperSettings` 等已移入分页的 computed。
2. **模板**：左侧 `<aside>` 二级导航（标题「设置」、搜索框、按组渲染 `<button aria-current="page">`、底部「改动自动保存 / 正在保存…」），`position: sticky; top: 0`（外层 `.page-view` 负责滚动）；右侧头部（「设置 / 分组」、标题、描述）+ 原有 `settings-pending` 提示 + `<fieldset :disabled="!isInitialized">` 包住 `<component :is="当前分页组件" />`（用 v-if 语义切换即可，不需要 KeepAlive）。窄窗口（800px）导航宽约 164px。
3. App.vue 的 `navigate:settings`（托盘 / 悬浮窗「设置」）可改为 `appStore.openSettings()`。

## 验收

```bash
node scripts/run-node-tests.mjs        # 含 test_settings_nav.mjs；test_desktop_agent_integration.js 会读 SettingsPage.vue 源码做「不含已下线字段」断言
npm run build
cargo test --workspace --manifest-path src-tauri/Cargo.toml
```

实机（`npm run tauri:dev`）：8 个分页切换；改任一设置后 500ms 自动保存且刷新不丢；识别引擎切换只显示对应配置；快捷键录入；权限页三项状态与「打开系统设置」；诊断页「在文件管理器中显示」；从转文字页「去设置」直达识别引擎页；搜索「token」只剩识别引擎。
