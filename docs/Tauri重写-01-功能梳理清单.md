# ATT 重写前置梳理：功能与架构清单（阶段①交付物）

> 目的：作为「Electron + Python → Tauri 2.x + Rust 完全重写」的功能对齐基线。
> 所有结论均给出代码出处（文件路径:行号）。与旧文档冲突时以代码为准。
> 本文档覆盖五个维度：① 用户可见功能全清单 ② 前端结构与样式（保留边界）
> ③ Python 后端职责与对外接口 ④ 快捷键实现与缺陷 ⑤ 数据流与状态管理。

---

## 0. 产品定位与总体架构现状

ATT（Audio to Text）是桌面语音转文字应用：全局快捷键触发录音 → ASR 转写（本地模型或在线服务）→ 可选 LLM 润色优化 → 记录落库（JSON + WAV）→ 支持复制/粘贴/导出。2026-10 功能减法后定位收敛为「简单录音笔记」：统一浮层、生词本、AI 动作已整体下线。

### 0.1 三层架构与进程拓扑（现状）

| 层 | 目录 | 职责 | 关键点 |
| --- | --- | --- | --- |
| Electron 主进程 | `electron/` | 应用生命周期、窗口、托盘、剪贴板监控、桌面代理（热键/静音）、拉起并监控 Python 后端 | 不做业务逻辑、不直接读写 `data/` |
| Vue 渲染进程 | `src/` | UI、**录音采集本体（MediaRecorder / AudioWorklet 在渲染进程！）**、录音编排、经 HTTP+WS 直连后端 | 业务编排大量落在渲染层 |
| FastAPI 后端 | `app/` | 转写编排、LLM 优化、JSON 持久化、ASR/LLM provider 装配、流式会话 | 独立进程，uvicorn 8000 端口 |

附属 Python 子进程（均由 Electron 主进程 spawn，stdio 通信）：

- `electron/python/desktop_agent.py` — 全局热键监听（pynput）+ 录音期间系统静音，JSON-RPC over stdio
- `electron/python/paste_helper.py` — 系统级「粘贴」模拟（一次性的 execFile 调用）
- `electron/python/audio_helper.py` — 非代理路径的静音控制兜底（`electron/audio/controller.js` 调用）

### 0.2 启动链路（electron/main.js:548-602）

```
app.whenReady()
  ├─ startDesktopAgent()     失败不阻断，仅缓存错误事件等渲染层就绪后下发
  ├─ startBackend()          PythonManager：探测/拉起 uvicorn，waitForReady 轮询 /api/health
  ├─ FloatingWindowManager / ClipboardMonitor 初始化
  ├─ setupIPC()              注册全部 ipcMain 通道
  └─ createWindow() + setupTray()
```

后端端口策略（electron/python/manager.js）：`ATT_BACKEND_PORT` → `PORT` → 8000；端口已有健康后端则直接复用（externalBackend）；否则向后找可用端口；打包态注入 `DATA_DIR=<userData>/data`。

### 0.3 重写必须接管的能力（→ Rust/Tauri）

1. 桌面外壳：无边框窗口 + 自绘标题栏、托盘（4 态图标 + 动态菜单）、悬浮条窗口（512×40）、单实例锁、剪贴板监控（800ms 轮询）
2. 全局热键（现为 pynput 子进程，见 §4 深挖）
3. 录音期间系统输出静音/恢复（现 desktop_agent / audio_helper 两路实现）
4. 后端进程管理（Rust 重写后后端即应用内嵌，不再需要跨进程拉起 uvicorn；但本地模型服务/桥接进程仍可能需要进程管理）
5. 系统粘贴模拟、系统文件选择对话框、拖拽文件路径获取
6. JSON 持久化、单写者锁、原子写、加密密钥管理（现为 app/repositories/）

---

## 1. 用户可见功能全清单（重写功能对齐基线）

> 按用户入口分组枚举。「现状」列描述现有行为；标 ⚠ 的是已知缺陷行为，重写时应修复而非复刻。

> 完整版（逐按钮/逐状态/带行号）见附录《[Tauri重写-01a-前端审计附录.md](Tauri重写-01a-前端审计附录.md)》§2；此处为对齐用的全量浓缩。

### 1.1 全局壳

- **TitleBar**（桌面端）：品牌标识、拖拽区（双击最大化/还原）、最小化/最大化/关闭三钮
- **Sidebar**：5 个导航项（首页/转文字/记录集/剪切板/设置）+ 底部后端连接状态灯（DEV 另显客户端数）；宽度可拖拽（≤110px 显折叠意图，释放折叠为 60px），布局持久化 localStorage
- **Toast**：底部居中、最多 3 条同屏、success/error/info 三色、3s 自动消失
- **托盘**（联动）：图标四态 idle/recording/transcribing/disconnected + tooltip；菜单=最近 10 条记录（点击=复制并粘贴）+ 设置 + 退出；单击托盘唤起主窗口；Windows 按 DPI 七档选图
- **悬浮条**（512×40 桌面底部居中置顶）：状态图标+波形/spinner、前缀文案（正在录音/实时记录/正在转写/正在优化/转写完成/处理失败）、multi/streaming 录音中与优化中显示**实时滚动文本**（mask 渐隐+打字光标）、打开设置/关闭按钮、complete 2s 与 error 3.2s 自动隐藏
- **全局热键**：仅按住模式（详见 §4）；「粘贴完成」→ Toast；托盘/悬浮窗「跳转设置」事件；桌面权限不足/代理错误 → Toast

### 1.2 首页（录音台 = RecorderSection + RecordsSection）

- 模型服务开关按钮（开启模型/预热中/已开启/加载中/当前模式不可用；在线模式凭据缺失时文案变「请先配置 ASR 密钥」；录音/转写中禁用）
- 两枚徽章：录音模式徽章（单段录音/多段准实时/Qwen 真流式/豆包在线流式）+ 状态徽章（就绪/录音中/处理中/优化中，语义色）
- 大录音圆钮（aria-label 随状态 7 态变化；不可用时禁用）+ 等宽 mm:ss 计时器 + 9 根波形条（multi/streaming 随实时音量跳动）
- panelStatus 11 种阶段文案（正在连接流式转写服务/正在生成最终转写结果/正在连续流式转写/正在捕获当前语音片段/上一段已送去转写/正在合并整场录音结果/静音切段监听中/正在录制整段音频/正在处理录音结果/正在后台优化文本/等待开始录音）
- 实时转录面板（LIVE 指示灯 6 态 + 实时文本 + 按模式占位文案）+ **LLM 流式优化面板**（等待首字 spinner/流式文本+光标/错误行/waiting 期原文预览/自动优化时显示取消钮）
- 转录区自动滚动（滚离底部 >48px 停止跟随）；底部信息条（状态/片段数/模式）
- 首页最近记录（RecordsSection）：选择模式（全选/删除选中/取消，删除二次确认）；RecordCard=播放试听（互斥）/两行预览（优化中显示流式文本+光标）/时长/相对时间/处理耗时/状态徽章（6 态）/动作钮（优化、取消、复制、导出 TXT、导出 JSON、收藏星标）；整卡左键复制、右键编辑、选择模式勾选
- RecordEditModal：原始/优化文本展示（点击复制）+ 编辑 textarea + 保存/取消

### 1.3 转文字页（本地文件转写）

- 整页拖拽投放（遮罩「松手，开始转写」；多文件取首个提示；转写中拒绝）+「选择文件」系统对话框 + 路径输入框（回车即转写）
- 波形母题三态动画（idle/live/done）
- **引擎三卡**：faster-whisper（本地默认）/Qwen3-ASR（本地）/豆包·实时大模型（在线；未配 Token 置灰+红点+「去设置」跳转）
- **LLM 校对面板**（默认关）：开关（懒加载模型列表）、LLM 模型下拉、窗口批大小 20/40/60 段、上下文重叠 200 字/不携带
- 参数行：语言 zh/en/ja/ko、分段时间戳复选；状态行（提交/等待秒数/后台任务/「LLM 校对中·窗口 x/y」）；错误红色告警条
- 结果卡（磁带日志风）：文件名/时长/引擎/缓存命中金色斜印章/「已校对（部分保留原文）」chip；**三视图 tab（校对后/原文/对照逐句 diff**，含「共 N 句·M 处修改」）；复制全文（2s「已复制 ✓」）；可折叠时间轴
- 本次会话历史架（最近 20 条，点击回看）；空态提示（容器列表、「结果不进入记录集」）

### 1.4 记录集页

搜索（300ms 防抖）+ 范围下拉（全部/仅原始/仅优化）+ 收藏筛选 toggle；N 条徽章 + 6 张骨架卡 + 空态；卡片网格（时间/状态徽章 7 态/收藏星标/5 行截断/优化|原始标识/复制/导出 TXT/导出 JSON/点卡编辑）；分页（page_size=100）；取消收藏即时移出「仅看收藏中」列表

### 1.5 剪切板页

搜索（250ms 防抖）+ 刷新；快捷过滤胶囊（今天 N/收藏 N/置顶 N + 类型：纯文本/图片/文件/链接）；卡片=类型徽章+相对时间+标题+6 行摘要（图片显示宽×高）+大小；**图片悬停懒加载预览**（加载中/不可用态）；置顶/收藏（乐观更新+失败回滚+忙碌禁用）/复制（文本/图片+元数据）/删除（二次确认）；剪贴板变化事件自动刷新

### 1.6 设置页

- 通用行为：初始化前禁用全部控件；深度 watch + 500ms 防抖自动保存；快照对比；服务端推送不覆盖未提交编辑；Token 明文只在输入时提交
- **分组 1 语音识别**：ASR Provider 下拉 4 选（whisper/qwen_asr/doubao_asr/chatgpt_asr）+ 各 provider 条件组（Whisper：模型五档+设备；Qwen：模型名/设备/dtype/max tokens/时间戳/音频上限/强制对齐器开关+模型；豆包：Token 只写+清除+Resource ID 四选+wss 端点+强制断句毫秒；ChatGPT：令牌只写+清除+端点+超时）
- **分组 2 录音设置**：录音模式下拉（单段/多段/流式）+ 流式运行时状态卡（可用性+原因）+ 静音分段阈值（仅 multi）+ 麦克风设备枚举下拉 + 录音时暂停其他音频 + 自动 AI 优化 + 自动复制（并尝试粘贴到焦点输入框）+ 默认优化预设下拉 + 查看提示词（只读弹窗）+ **快捷键录入**（点击监听态金色脉冲边框，按下组合即录入；「恢复默认」平台默认值）
- **分组 3 LLM 服务**（v2）：添加供应商/添加模型；损坏配置跳过警告条；供应商档案卡（名称/模型数/端点/密钥状态含尾号/验证状态+延迟/重新验证/编辑）；模型表（默认优化金 chip/备用 chip/参数摘要/失败原因/测试/设为默认/编辑/删除）；默认+备用模型提示
- **分组 4 诊断**：后端连接徽章 + WS 客户端数 + 运行环境/模型开关徽章
- **LLM 弹窗三件**：三步向导（供应商搜索网格→凭据+草稿验证（密钥不落盘）→模型多选（远端拉取失败降级手输）+参数折叠区+设默认复选）；档案编辑（验证横幅、级联删除提示）；模型编辑（模型串预览、思考强度五档+计费警示）
- **预设弹窗两件**：列表+默认徽章；只读查看（预设唯一真相源 prompts/*.txt）

### 1.7 全局快捷键（用户可感知行为）

- 默认快捷键按平台不同（electron/desktop/settings.js:20-45）：Windows `Win+Alt`（纯修饰键）、macOS `Cmd+Shift+R`、Linux `Ctrl+Shift+R`
- 行为模式：**仅「按住」模式** —— 按下组合键开始录音，松开结束录音（useRecorder.js:2363-2390）；不存在「点按切换」模式
- 录音中/启动中按下的 press 事件被忽略（useRecorder.js:2368-2370）；非录音中的 release 被忽略（:2385-2387）
- 设置页可重新录入快捷键（点击录入 → 按下组合 → 显示 `display` 字符串）
- 快捷键与录音模式（单段/多段/流式）正交，同一快捷键触发当前设置的录音模式
- macOS 缺辅助功能权限时，界面会收到权限提示（desktop:permission-required 事件）

### 1.8 系统联动（静音/粘贴/剪贴板监控）

- **录音期间可选自动静音系统输出**（`pause_audio_on_recording`，默认开）：录音开始快照当前静音状态→静音；结束恢复原状。平台实现：Windows pycaw COM EndpointVolume / macOS osascript / **Linux 不支持**（返回失败）；⚠ Windows 设备句柄启动时固化，默认输出设备热切换后仍操作旧端点（audio_helper.py:31-37）
- **托盘「复制并粘贴」**：写剪贴板（标记内部写入防回捕）→ paste-text 事件 → paste_helper.py 模拟 Ctrl/Cmd+V 粘贴到前台应用（每次 spawn 一个 Python 进程，约 200ms 级延迟）
- **转写完成后自动复制**（可配置）+ 可选自动粘贴到焦点输入框（按录音模式文案不同，见 statusCopy.mjs——真流式「复制或粘贴失败」被测试锁死，重写时统一）
- **剪贴板后台监控**：800ms 轮询系统剪贴板；文本规范化（CRLF→LF、trim）去重后 POST 后端；图片按 PNG 字节 SHA-256 哈希去重落盘 data/clipboard/images/；内部写入 5s TTL 防自捕获；捕获成功通知前端刷新；⚠ 打包态图片目录解析到 asar 内只读路径（写入必败，未实测）

---

## 2. 前端页面/组件结构与样式实现（Electron 部分）

### 2.1 入口与页面组织（本人已验证）

- **无 vue-router**：页面切换是 `appStore.currentPage` + `v-show` 平铺（src/App.vue:23-42），5 个页面常驻挂载（切换不丢状态）：`home`（RecorderSection + RecordsSection）、`transcribe`、`records`、`clipboard`、`settings`
- 双渲染入口：`src/index.html`（主窗口，建 Pinia）+ `src/floating.html`（悬浮条，**不建 Pinia**，独立轻量树）
- 6 个全局模态挂在 App 根部（RecordEdit/Preset/PresetEdit/LLMWizard/LLMModelEdit/LLMProviderEdit）+ Toast
- 布局：自绘 TitleBar（仅 isElectron 时渲染，App.vue:10）+ 可拖宽 Sidebar（CSS 变量 `--sidebar-width` 驱动，App.vue:104-106）+ 主内容区
- Tauri 迁移含义：页面组织/路由方式与 Electron 无耦合，**原样保留**；仅需把 `isElectron` 判定改为 Tauri 环境判定

### 2.2 Electron IPC 契约全表（preload.js:3-58，Tauri 命令替换清单）

**渲染 → 主（invoke，需返回值）**：

| preload 方法 | IPC 通道 | 用途 |
| --- | --- | --- |
| `getPort` | `backend:port` | 取后端端口（Tauri 下后端为内嵌进程，此发现机制整体消失/改为常量） |
| `openMediaFile` | `dialog:open-media-file` | 系统文件选择（音视频过滤器清单在 main.js:311-328） |
| `readClipboardText` | `clipboard:read-text` | 读剪贴板文本 |
| `writeClipboard` | `clipboard:write` | 写剪贴板文本（标记内部写入以去重） |
| `writeClipboardImage` | `clipboard:write-image` | 写剪贴板图片（按 image_path/imageToken） |
| `pasteClipboard` | `clipboard:paste-active` | 模拟「粘贴」到焦点窗口（spawn paste_helper.py） |
| `syncInputSettings` | `desktop:sync-input-settings` | 更新热键配置（热更新到 agent） |
| `syncDesktopSettings` | `desktop:sync-settings` | 同步桌面设置（静音策略等） |
| `syncRecordingState` | `desktop:recording-state` | 通知录音开始/结束（驱动系统静音） |
| `getDesktopSettings` / `saveDesktopSettings` | `desktop:settings:get` / `:save` | 读写 userData/desktop-settings.json |
| `getPathForFile` | （webUtils 直调） | 拖拽 File → 本地绝对路径（Tauri 有原生 onDragDropEvent） |

**渲染 → 主（send，单向）**：`window:minimize/maximize/close`、`tray:set-icon`、`tray:update-menu`、`floating:show/hide/toggle/update-state`、`debug:log`

**主 → 渲染（事件订阅）**：`hotkey:press`、`hotkey:release`、`paste-text`、`navigate:settings`、`clipboard:changed`、`desktop:permission-required`、`desktop:agent-error`（⚠ `desktop:agent-ready` 主进程有发送但 preload 未暴露，渲染层收不到——已知缺陷 #17）

### 2.3 useElectron.js：唯一桌面能力包装层（252 行，Tauri 适配的唯一改写点）

- `api`：44 个方法的 HTTP 透传（上传/会话/记录/优化流式/剪贴板/设置/LLM v2/提示词），全部走 `api/client.js`
- `windowControl` / `tray` / `floatingWindow` / `clipboard`（含 pasteActive 系统粘贴）
- 事件订阅族 `onXxx` + `removeAllListeners`；`syncInputSettings/syncDesktopSettings/syncRecordingState`
- `openMediaFile` / `filePathForFile` / `platform` / `getBackendPort`
- 非 Electron 环境全部可选链降级（浏览器可跑无桌面能力的开发模式）
- **迁移结论**：组件层（.vue）不直接引用 `window.electronAPI`（硬约束 7 基本生效，仅 2 处非组件泄漏见 §2.4），重写只需替换本文件与 preload 契约 → Tauri `invoke()`/`listen()`，组件层零改动

### 2.4 组件分层与样式体系

**样式实现方式（本人已验证，结论：可整体保留）**

- `src/styles/tokens.css`（232 行）是**全应用唯一设计令牌来源**：深色主题（color-scheme: dark）灰阶 12 档、语义表面色阶、文字三级（对比度全部 ≥4.5:1）、唯一强调色金色 `--accent` 家族、语义色 ok/warn/err/info、8 基数间距、四级圆角、七级字号、阴影四级、z-index 体系、动效三档时长+统一缓动、系统字体栈（离线可用）、统一滚动条、旧名兼容别名层
- 已内置 `prefers-reduced-motion` 全局降级（tokens.css:223-232）
- 组件样式为 SFC scoped CSS + 少量全局类（main.css）；无 CSS 框架、无 Tailwind、无 UI 库——**与 Electron 零耦合，Tauri 下原样可用**
- `electron-overrides.css` 名字有误导性：实际是「桌面壳适配层」，其中唯一 Electron 专有属性是标题栏拖拽区 `-webkit-app-region: drag/no-drag`（:26,51）→ Tauri 迁移时改为 `data-tauri-drag-region` 属性方案，其余全部保留
- 悬浮窗独立样式 `floating.css`（半透明表面 `--surface-float` + backdrop blur）
- **动效现状**：令牌层已有三档时长/统一缓动与 hover 过渡，但无波形指示、页面切换动画等（阶段③按 /ui-ux-pro-max 补充的改造点）

**组件分层**（common 原语 → 页面组件 → 模态）：

- `components/common/` 12 个零业务原语（BaseButton/BaseInput/BaseSelect/BaseTextarea/BaseBadge/Modal/ConfirmDialog/Toast/EmptyState/Skeleton/PageHeader/LogoMark）：scoped 样式，支持 CSS 变量注入定制——**零 Electron 依赖，直接平移**
- 页面组件 `home|records|clipboard|transcribe|settings|floating` + 布局 `layout/`（Sidebar/TitleBar）：模板与逻辑直接平移，仅经 store/composable 间接依赖桌面层
- `modals/` 6 个弹窗：直接平移
- 图标全内联 SVG（stroke=currentColor），LogoMark 经 Vite 内联 data URI；无图标库
- scoped/全局划分：原语+页面组件 scoped；布局骨架与跨组件类（sidebar/settings/records/modal/preset/llm-config）在 main.css(1466 行) 与 electron-overrides.css

**⚠ window.electronAPI 直读验证结果（修正 AGENTS.md 的理想约束）**：除 useElectron.js 本体外有 **2 处泄漏**——`src/stores/settings.js:9-14`（platform）与 `src/api/client.js:14-16`（getPort）；组件层 .vue 零直读。另有**悬浮窗独立 `window.floatingAPI`**（close/openSettings/onStateChange/removeAllListeners，FloatingWindow.vue:157-240）不经 useElectron——Tauri 多窗口 event 系统需单独映射。

**前端改造边界结论**（详见附录 §10）：
- **不动**（直接平移）：styles/ 全部、common 原语、6 弹窗、全部页面组件模板、4 个 store、`composables/recorder/*.mjs` 8 件套纯逻辑 + `utils/*.mjs` + `fileTranscribeModel.mjs`（**有 16 个 Node 测试锁定，最大红利区**）、useRecorder 主体、pcm-capture-worklet（标准 WebAudio，WebView2 支持）、useSettings/useRecords/useClipboard/useFileTranscribe/useSilenceDetector/useWebSocket（后端协议不变则零改动）
- **必动**：useElectron.js（唯一主战场，保持导出形状则上游零改动）、client.js 仅 getBackendPort、stores/settings.js 仅 getPlatform、FloatingWindow.vue（floatingAPI→Tauri event）、TitleBar+electron-overrides.css（drag-region 属性）、index.html CSP（追加 `ipc: http://ipc.localhost`）、vite.config.js 对接 Tauri、App.vue isElectron 语义改 Tauri 判定

### 2.5 构建与打包现状（迁移 Tauri 的配置改造点）

- Vite 单配置双入口（vite.config.js）：`src/index.html`（主窗口）+ `src/floating.html`（悬浮条），`base: './'`，dev server 固定 `127.0.0.1:8090`，产物 `dist/`
- 依赖仅 `vue` + `pinia`（package.json:24-26），无 UI 框架、无 vue-router、无 axios（自封装 fetch）
- electron-builder：asar 内只放代码（dist/electron/app/*.py/prompts），`python-dist/` 解释器、`app/` 后端源码、`prompts/*.txt` 解包到 resources/；**`data/` 与 `models/` 绝不进包**（密钥与模型安全）；win target `nsis+portable`（nsis 实际失败，见 §6）
- Tauri 迁移后：双入口构建可原样保留（Tauri 多窗口对应多 HTML 入口）；`dist/` 直接作为 Tauri frontendDist；`base: './'` 与 Tauri 兼容

---

## 3. Python 后端职责、接口与依赖

> 来源：FastAPI 后端全量梳理（app/ 目录 70+ 文件逐个通读）。生产挂载：main.py:164-171 逐个 include_router（records/sessions/settings/prompts/llm/clipboard/transcribe_file），全部 prefix=/api；聚合层 routes.py 不参与生产挂载。另有 GET /（main.py:182）服务前端 index.html。
> **端点总计：53 个 HTTP（含 GET /）+ 2 个 WebSocket。**

### 3.1 records 路由（12 个）

| 方法+路径 | 用途 | 关键契约 |
| --- | --- | --- |
| POST /api/audio/upload | 上传音频→建 pending 记录→后台转写 | multipart `file` + query `auto_transcribe`；响应 `{status:"uploaded",record_id,duration,transcription_status}`；413 超限（边读边校验 500MB）、400 格式非法 |
| GET /api/records | 分页列表（**响应体是裸数组**，非 {items,total}） | query `status/limit`(默认200≤1000)/`offset` |
| GET /api/records/search | 文本+收藏搜索 | `q/search_type(all\|raw\|optimized)/favorite_only/page/page_size`；响应 `{items,total,page,...}` |
| GET /api/records/{id} | 记录详情（全字段） | 404 不存在 |
| GET /api/records/{id}/status | 轮询转写状态（最小载荷） | `{record_id,status,raw_text,optimized_text,duration,processing_time,is_favorite}` |
| PUT /api/records/{id} | 更新文本/收藏 | `{raw_text?,optimized_text?,is_modified?,is_favorite?,revision?}`；**revision 乐观锁冲突→409** |
| DELETE /api/records/{id} | 删记录+音频 | `{status:"deleted"}` |
| GET /api/records/{id}/audio | 播放音频（FileResponse wav） | 空 audio_path/不存在→404 |
| POST /api/records/{id}/optimize | 非流式 LLM 优化 | 失败 502 + 脱敏 detail + WS 广播 optimization_failed |
| POST /api/records/{id}/optimize/stream | 流式优化（**NDJSON**） | 每行 `{type:"start\|delta\|done\|error",...}`；done 带 `degraded:true` 标记降级重跑；503 优化器未初始化 |
| POST /api/records/{id}/optimize/cancel | 取消流式优化 | `{record_id,cancelled}` |
| GET /api/records/{id}/export | 导出 txt/json | query `format/use_optimized`；Content-Disposition attachment |

### 3.2 sessions 路由（5 HTTP + 2 WS）

POST /api/sessions（建会话）、GET /api/sessions/{id}、POST /api/sessions/{id}/segments（上传分段→后台转写）、POST /api/sessions/{id}/merge（合并音频整段转写→auto_optimize 时流式优化→建记录；失败回退分段拼接文本）、DELETE /api/sessions/{id}。

**WS /api/ws（全局状态通道）**：连接即下发 `{"type":"connected","client_id","status":{...},"timestamp":毫秒}`；客户端可发 `status_update`/`ping`；服务端广播 12 类事件：`status`、`transcription_started`、`transcription_completed`（含 `electron_active`）、`error`、`segment_transcribed`、`segment_failed`、`optimization_started/delta/completed/failed/cancelled`（⚠ connected/status 用毫秒时间戳，业务事件用 ISO 字符串——历史不一致，重写需统一或保真）。

**WS /api/asr/stream（真流式 ASR，协议必须保真）**：

| 客户端发送 | 服务端响应 |
| --- | --- |
| `{"type":"stream_start","language","context"}` | `{"type":"stream_ready","data":{session_id,language,sample_rate:16000}}` |
| 二进制帧（16kHz 单声道小端 int16 PCM） | `{"type":"stream_partial","data":{text,language,duration}}` |
| `{"type":"stream_finish"}` | `{"type":"stream_finished","data":{record,raw_text,language,duration}}`（空文本→record=null 且删音频不留档） |
| `{"type":"stream_cancel"}` | `{"type":"stream_cancelled"}` |
| `{"type":"ping"}` / 异常 | `pong` / `stream_error` 并关闭 |

断开/异常统一在 finally cancel_session 释放（防会话与临时 wav 泄漏）。

### 3.3 settings / 模型服务路由（9 个）

GET /api/health（**27 字段**，双命名：`qwen_streaming_service_*` 旧名 7 个与 `model_service_*`/`streaming_*` 新名同源，前端仍读旧名）、GET/POST /api/model-service(/start|/stop)、三个 `/api/streaming/service*` **别名**（前端只用 model-service*）、GET /api/settings（密钥脱敏：只回报 `doubao_asr_api_key_set`/`chatgpt_asr_api_key_set` 布尔）、PUT /api/settings（白名单 UserSettingsUpdate：未知键丢弃、null=未提供、类型非法 400 中文；MODEL_RUNTIME_KEYS 变更触发停模型服务+重建 provider）。

### 3.4 prompts / llm / clipboard / transcribe_file 路由

- **prompts（2 个，只读）**：GET /api/prompts（每次重载 prompts/*.txt，6 个预设：会议记录/小说创作/文案优化/程序员模式/结构化整理/通用优化）、PUT /api/prompts/default
- **llm v2（17 个）**：providers 列表（14 家注册表）、state（profiles+models+默认绑定+skipped_entries）、profiles CRUD + verify + remote-models、verify-draft（密钥不落盘）、models 批量建(1-50)/改/删/verify(5-token)/activate、PUT settings（default/fallback 绑定）
- **clipboard（7 个）**：items 列表（pinned 优先+时间倒序）/stats/建（内容重复合并 duplicate_count）/详情/图片（路径必须落在 data/clipboard/images/ 内）/改/删
- **transcribe_file（2 个）**：POST /api/transcribe（`{path,language,want_segments,engine,proofread,proofread_model_id,proofread_options}`；短文件同步返回、长文件/带校对 202+job_id；错误码 404/401(X-ATT-Token)/413/415/422/409/503(Retry-After)/500；内容 sha256 LRU 缓存 200 文件）；GET /api/transcribe/jobs/{job_id}（内存任务表、30 分钟保留、上限 100 条）

### 3.5 STT provider 体系（Rust trait 对齐目标）

**接口**：`ASRProvider`（provider_type/model_name/capabilities()/get_runtime_capabilities(recording_mode)/transcribe()/warmup()/close()）+ `StreamingASRProvider`（create_session()）+ `StreamingASRSession`（append_audio(pcm16k)/finish()，豆包另有 cancel()）。factory 合并顺序：Settings(.env) → user_settings(**settings.json 覆盖 env**) → runtime 参数；密钥解析优先级：settings.json 密文 > 明文字段 > env。

| provider | 本质 | 批量 | 流式 | 关键依赖/限制 |
| --- | --- | --- | --- | --- |
| whisper | **本地** faster-whisper | ✅ | ❌ | 模型文件 models/faster_whisper；中文优化参数（beam5/temp0/vad） |
| qwen_asr | **本地** Qwen3-ASR（qwen-asr 包 + torch） | ✅ | 经 vLLM/桥接 | 长文件>30s 静音切点分块；**Windows 判流式不支持**；soundfile 可读格式 |
| qwen 流式 | 本地 vLLM 或 HTTP 桥接 | — | ✅ | 桥接 scripts/qwen_streaming_service.py（127.0.0.1:9000）；`streaming_backend∈{auto,local_vllm,remote_bridge}` |
| doubao_asr | **在线** 火山 SAUC 双向流式 WS | ✅（整段按 200ms 推同一会话） | ✅ | Access Token 密文落盘；SAUC 二进制协议（4 字节头+gzip/json）；握手三头 X-Api-Key/X-Api-Resource-Id/X-Api-Connect-Id；**Node undici 不能带鉴权头（Rust 用 tokio-tungstenite 自定义 header 恰好无此问题）** |
| chatgpt_asr | **在线批量** ChatGPT 网页听写逆向接口 | ✅ | ❌ | access token 密文；四鉴权头；建议 <11 分钟 |

`model_service_mode` 三值：`provider`（本地 warmup）/ `managed_streaming_bridge`（桥接托管）/ `online`（凭据配齐即 ready，无需「开启模型」）。

**桥接服务 HTTP 协议**（scripts/qwen_streaming_service.py）：GET /api/health（`ready && loaded` 双条件；`loading` 语义特殊——「未就绪且没在加载」≠loading，前端会据此锁死按钮）、POST /api/start、POST /api/chunk/{id}（octet-stream 原始 PCM）、POST /api/finish/{id}、DELETE /api/session/{id}、POST /api/preload（加载模型+0.5s 静音真实推理预热，吃掉 vLLM 首次编译 ≈32s）。

### 3.6 LLM v2 域

- **网关**：LiteLLM `acompletion` 统一调用点；强制注入 timeout/num_retries；**litellm 懒加载 + 启动期后台预热**（import ≈2.5s 不落首次请求）；流式「首 token/块间空闲」双看门狗；reasoning_effort 下传（预检+被拒去掉重试）；unsupported_provider 自动包 `openai/<ID>` 重试
- **注册表**：14 家（deepseek/openai/anthropic/gemini/dashscope/doubao/moonshot/zhipu/groq/mistral/xai/openrouter/ollama/custom）
- **存储**：data/llm_v2.json，档案级 api_key_encrypted（Fernet）+ api_key_hint；逐条容错加载（坏条目计 skipped_entries）；v1 自动迁移
- **优化器**：TextOptimizer=「ASR 口语→书面语整理」（唯一 LLM 热路径）；预设模板 prompts/*.txt；空正文+finish_reason=length → 不可重试 output_truncated（防「思考吃满 max_tokens→空正文→回退重跑 22s+18s」事故）
- **流式优化服务**：同记录并发「后来者取消先来者」；首条增量前失败且可重试→回退非流式一次（done 带 degraded:true）；失败/取消/断开必须把记录从 optimizing 复位 transcribed
- **文件转写校对**：滑窗（40 段/4000 字/200 字前文）+「LLM→验证(difflib 相似度)→反馈→重试」最多 3 步；LLM 全挂→applied=false 降级交原文

### 3.7 持久化全貌（data/ 实际文件）

| 文件/目录 | 用途 |
| --- | --- |
| settings.json | 用户设置（含 doubao/chatgpt 密文键），线程锁+原子写 |
| llm_v2.json | LLM 档案+模型+默认绑定（v1 备份 llm_configs.json.v1.bak） |
| .encryption_key | Fernet 密钥（44B） |
| .writer.lock | 单写者锁 {pid,host,heartbeat_at}；心跳 15s、陈旧 60s 可接管、Windows 用 OpenProcess 探活 |
| .index/records.json | 记录索引派生清单（mtime_ns+size 增量刷新，可随时删除重建） |
| records/*.json（实测 915 个） | 每记录一文件（13 字段含乐观锁 revision） |
| sessions/*.json（44 个） | 多段会话 |
| clipboard/*.json（1030 个）+ images/ | 剪贴板条目与图片（content_hash 去重） |
| audio/records/*.wav | 记录音频（16k mono wav） |
| audio/temp \| permanent | 临时/永久音频（temp 有历史残留） |
| transcribe_file_cache.json | 文件转写缓存（sha256→结果，LRU 200） |
| prompts/presets.json、wordbook/ | **遗留**，无代码读取，不需迁移 |

**原子写**：同目录 .tmp → write+flush+fsync → os.replace（Windows 同卷原子）；锁体系三层：按资源路径的 asyncio.Lock/threading.RLock（进程内）+ .writer.lock（跨进程单写者）。

### 3.8 配置全表

- **Settings（env，37 键）**：见 app/config.py:254-374（应用/路径/ASR/Whisper/Qwen/Qwen 流式/豆包/ChatGPT/文件转写 8 键/上传上限）；extra="ignore"（.env 含桥接与 compose 专用键）
- **UserSettings（settings.json，30+ 键）**：见 app/config.py:159-195；密钥只写不读（PUT 收 `doubao_asr_api_key` 明文→存密文，GET 只回报 `_set` 布尔）；已退役键读写两侧剥离
- 设置变更副作用：MODEL_RUNTIME_KEYS → 停模型服务+重建 provider；recording_mode 运行时生效不重建

### 3.9 后端重写建议摘要（阶段②细化）

**可简化**：聚合路由层（routes.py，为 Python 测试 monkeypatch 服务 → Rust 用 Arc<AppState>）、健康双命名旧字段、/api/streaming/service* 别名、非流式 /optimize 入口（状态机已统一在流式服务）、v1 LLM 迁移、4 套本地模型路径探测变体。
**必须保真**（前端依赖）：两个 WS 帧格式全量、GET /api/records 裸数组、health 27 字段、settings 脱敏契约、NDJSON 优化事件、记录状态机与失败复位、豆包 SAUC 二进制协议与 definite 前缀推进、桥接 HTTP 协议、原子写/单写者/乐观锁语义、密钥红线（明文永不落盘/不进日志/GET 响应绝不含密文字段）。
**Rust 依赖对应**：axum + tokio-tungstenite（自定义 header）+ ffmpeg/ffprobe 外部命令 + **litellm 无 Rust 对应物**（需按 provider 前缀自行路由 OpenAI 兼容客户端，或保留 Python 桥）+ Fernet 兼容实现（才能读旧密文）。

---

## 4. 快捷键系统现状与缺陷深挖（重写的核心动机）

### 4.1 现有实现链路（完整）

```
[OS 键盘事件]
  → pynput keyboard.Listener（electron/python/desktop_agent.py:241-245，独立 Python 子进程）
  → 组合键状态机 HotkeyMonitor（desktop_agent.py:213-332）
  → JSON-RPC notify: hotkey.press / hotkey.release（desktop_agent.py:324,332）
  → stdio → DesktopAgentManager（electron/desktopAgent/manager.js:180-188，EventEmitter）
  → mainWindow.webContents.send('hotkey:press'/'hotkey:release')（electron/main.js:244-251）
  → preload.js contextBridge → 渲染进程 useElectron().onHotkeyPress/onHotkeyRelease
  → useRecorder.js:2363-2390 消费：press→startRecording()，release→stopRecording()
```

配置流转：设置页表单 → `desktop:sync-input-settings` IPC（main.js:475-494）→ 存 `userData/desktop-settings.json`（仅 `hotkey` + `pause_audio_on_recording` 两个键，desktop/settings.js:97-104）→ `hotkey.update` JSON-RPC → agent 内 `normalize_hotkey()` 热更新（无需重启）。

### 4.2 组合键状态机语义（desktop_agent.py:276-332）

- 维护 `active_modifiers`（按下且未松开的修饰键集合）与 `active_keys`（未松开的普通键集合）
- `_matches_hotkey()`：修饰键集合**精确相等** + 普通键集合精确匹配（key=None 时要求无普通键按下）
- 首次匹配成功 → latch 置位 → 上报 `hotkey.press`；离开匹配态 → 上报 `hotkey.release`
- `PRESS_GUARD_MS = 250`（:55）：release 后 250ms 内的再次匹配不上报（防抖/防自动重复）
- 修饰键组合（无普通键）场景：latched 后按下其他普通键不打断（:327-328），直到修饰键集合变化才 release

### 4.3 具体缺陷清单（为什么「无法完整支持」；两路独立复核合并，均定位到行号）

| # | 分类 | 缺陷 | 代码证据 | 后果 |
| --- | --- | --- | --- | --- |
| 1 | 键位能力 | **主键仅支持单个字母/数字**：`_resolve_key_char()` 只接受 `len==1 && isalnum` | desktop_agent.py:298-303,130-134 | F1-F12、方向键、空格、回车、标点、小键盘、媒体键全部不可绑定 |
| 2 | 键位能力 | **无主键时要求 ≥2 修饰键，否则整体回退默认**；修饰键仅 5 种 | desktop_agent.py:136-137；desktop/settings.js:86-88 | 「任意组合」不可配置，合法组合被静默拒绝 |
| 3 | 键位能力 | **Shift+数字组合永不命中**：Shift 按住时数字键 char 是 `!` 等符号，被 `_resolve_key_char` 拒收，但配置侧允许通过 | desktop_agent.py:298-303 + settings.js:82-88 组合推论 | 用户可设置一个永远无法触发的快捷键，无任何提示 |
| 4 | 键位能力 | **非拉丁键盘布局下字母主键失效**：靠 `KeyCode.char` 匹配，无 vk/布局归一化 | desktop_agent.py:298-303 | 非美式布局（如俄语/法语键盘）下热键静默失灵 |
| 5 | 键位能力 | **修饰键集合严格相等匹配**：多按任一修饰键立即不匹配 | desktop_agent.py:305-313 | 无「至少包含」语义；与其他修饰键习惯组合时易误触发 release |
| 6 | 注册机制 | **无系统级注册 → 无冲突检测、无吞键**：pynput 是被动全局监听 | desktop_agent.py:241-245 | ① 与其他应用系统热键冲突无仲裁；② 快捷键按键**原样透传焦点应用**（录 R 真的会打出 r、按 Win 弹开始菜单）；③ 全系统按键都被捕获（隐私劣势） |
| 7 | 注册机制 | **只能注册一组快捷键、只绑定录音一个动作** | desktop_agent.py 全文单一 `self.hotkey` | 无法多动作多键；重写要求的应用内冲突检测无从谈起 |
| 8 | 状态机 | **修饰键状态粘滞（「粘滞录音」根因）**：焦点切换/UAC 安全桌面/锁屏/休眠恢复吞掉 key-up 后 `active_modifiers` 残留，且**无任何超时/周期校正/电源事件订阅** | desktop_agent.py:276-332（纯事件驱动） | release 永不上报→按住模式录音无法停止；或组合永远无法再匹配（热键失灵直到重启） |
| 9 | 状态机 | **更换热键不发旧组合的 release** | desktop_agent.py:269-274 | 换键瞬间若正按住旧热键，录音悬挂 |
| 10 | 状态机 | **250ms 防抖吞快速二次按压**（点按模式的天然障碍） | desktop_agent.py:55,318-325 | 快速连按丢事件；「点按切换」模式无法在此机制上实现 |
| 11 | 可用性 | **agent 崩溃后无自动重启**：exit 仅置空引用+转发错误 | main.js:262-276 | agent 意外退出后热键+静音全部静默失效直到重启应用 |
| 12 | 可用性 | **对 agent 的请求无超时**：pending Promise 只在进程 close/error 时 reject | desktopAgent/manager.js:104-121,151-166 | agent 挂死不回包则调用方永久 pending |
| 13 | 可用性 | **`desktop:agent-ready` 必然丢失**：主进程 send 但 preload 未暴露监听器（且不进启动期事件缓存） | main.js:240-243 vs preload.js:3-58；缓存机制只覆盖 permission/error（main.js:60-85） | 「代理就绪后再录音」类逻辑永远等不到该事件 |
| 14 | 可用性 | **macOS 无辅助功能权限时 agent 仍报 ready**：检测失败只发通知不阻断，Listener 照常启动并静默失效 | desktop_agent.py:229-245 | 「设置成功但按键无反应」，提示还要求重启应用 |
| 15 | 架构 | **热键业务逻辑在渲染进程**：press/release 消费（开始/停止录音）在主窗口 Vue 里 | useRecorder.js:2363-2391 | 渲染层卡死/被节流/窗口销毁（macOS 关窗）时热键形同虚设；Tauri 重写应把状态机放 Rust 侧 |
| 16 | 架构 | **仅「按住」模式**，无「点按切换」 | useRecorder.js:2363-2390；statusCopy.mjs:20 | 点按模式是重写新增能力 |
| 17 | 架构 | **延迟链路长**：OS→pynput 回调→stdio JSON→Electron 主进程→IPC→渲染进程事件循环→才 getUserMedia（prepare 实测 ≈385ms） | 链路见 §4.1；已知问题.md:376 | 按下到采样的延迟叠加在用户感知上 |
| 18 | 架构 | **快捷键归一化 4 处重复实现**（后端 config.py:93-152 / electron desktop/settings.js:71-95 / agent desktop_agent.py:112-143 / 前端 stores/settings.js:75-95），且 `hotkey` 双真相源（data/settings.json + userData/desktop-settings.json） | 见左 | 规则改动四处同步，历史已多次漂移；重写必须收敛为 Rust 单一真相源 |
| 19 | 平台 | **Linux 依赖 X11**：pynput 在 Wayland 基本不可用，无检测无提示；Windows Win 键与系统手势冲突未处理 | desktop_agent.py:229-257（仅 macOS 有权限分支） | 三端行为不一致；Wayland 用户热键静默无效 |
| 20 | 平台 | **静音联动脆弱**：Windows 设备句柄启动时固化不随默认设备切换；agent 异常死亡时系统停留静音（无清理钩子）；录音期间用户手动改静音会被恢复逻辑覆盖 | audio_helper.py:31-37；desktop_agent.py:192-210（恢复仅在正常路径） | 静音状态泄漏/错乱 |

> 平台差异总表：Windows 默认 `Win+Alt`（无主键，Win 键系统行为不可控）、pynput 低层钩子无需权限；macOS 默认 `Cmd+Shift+R`、需辅助功能权限（有主动检测+stderr 兜底，但兜底只匹配英文文案 manager.js:43-48）；Linux 默认 `Ctrl+Shift+R`、X11-only、静音不支持。

### 4.4 重写目标对照（来自任务要求，阶段②细化）

- 任意主键（含 F 区/方向键/空格/回车等）+ 任意修饰键组合，用户可配置
- 多组快捷键 + 冲突检测（应用内冲突必检；系统级冲突尽力检测并提示）
- 点按模式（按下开始/再按结束）与按住模式（按住开始/松开结束）可切换
- Windows/macOS/Linux 三端一致；焦点切换、休眠恢复、锁屏后不出现粘滞录音（键态超时校正 + 电源/会话事件监听 + 录音状态看门狗）
- Tauri 侧预期方案（阶段②定稿）：Windows `RegisterHotKey`（吞键+系统级冲突检测）、macOS `Carbon RegisterEventHotKey`/`NSEvent addGlobalMonitorForEvents`（吞键受限需注明）、Linux X11 `XGrabKey`；按住模式需要 key-up 检测（RegisterHotKey 只给按下，需低级钩子 WH_KEYBOARD_LL / CGEventTap / XRecord 组合，或注册「快捷键+任意释放」策略——阶段②详细设计）

---

## 5. 当前数据流与状态管理方式

### 5.1 录音三模式数据流（现状）

**单段（single）**：
```
热键/UI → startRecording → prepareRecordingEnvironment（开麦克风+环境检查）
  → MediaRecorder 录完整段（webm）
  → stop → blob → POST /api/audio/upload（边读边限流+校验+转 WAV 落盘）
  → 返回 record_id（status=pending）→ BackgroundTasks 后台转写
  → 前端 1s 轮询 GET /api/records/{id}/status（连续失败上限 30 次）
  → 同时全局 WS /api/ws 推送 transcription_started/completed、optimization_* 事件
  → 完成后自动优化（可选）→ Toast + 自动复制/粘贴（可选）
```

**多段（multi）**：
```
startRecording → POST /api/sessions 建 session
  → MediaRecorder 持续录 + useSilenceDetector 静音检测
  → segmentPolicy 判定切分（最短 2.2s/最长 10s/强制切分下限 0.8s）
  → 每段 POST /api/sessions/{id}/segments（分段并行后台转写，WS 推 segment_transcribed）
  → 松手 → 上传最后一片 → POST /api/sessions/{id}/merge
  → 合并时若有 merged_audio_path 整段重转一次（失败回退拼接文本）→ 生成 Record
```

**流式（streaming）**：
```
startStreamingRecording = Promise.all([connectStreamingSocket, setupStreamingAudioCapture])
  → WS /api/asr/stream：发 {"type":"stream_start"} 等 stream_ready(session_id)
  → AudioWorklet 每 128 采样（≈8ms）post Float32 → 转 PCM16 16k 单声道
  → 建连前样本积压 streamingPendingSamples，stream_ready 后统一冲刷（协议顺序：音频永远在 stream_ready 之后上行）
  → 之后每 STREAM_FLUSH_SAMPLES=3200 采样（≈200ms）flush 一次二进制帧
  → 服务端推 stream_partial（增量文本实时上屏）
  → 松手 → 尾包冲刷（必须先于 cleanupStreamingAudio，否则丢尾）→ stream_finish → stream_finished → Record 落库
```

关键防御性设计（重写需保留的语义）：
- 「采集与建连并行 + 采集代次守卫 streamingCaptureGeneration」防幽灵采集与开头丢字（已知问题.md:374）
- 「启动窗口内松手排队 pendingStopAfterStart」（recordingStopState.mjs）防幽灵录音（已知问题.md:376）
- 尾包冲刷先于音频清理防丢尾包（已知问题.md:377）

### 5.2 前端状态管理

- Pinia 四 store：`app`（currentPage/侧边栏/isRecording/isTranscribing/toasts/modals 六开关/选择模式）、`settings`（约 40 键设置+runtimeStatus 新旧双命名+预设+LLM v2 全家+模型服务就绪轮询 1s×5min）、`records`（CRUD+每次变更推托盘菜单）、`optimizationStream`（单槽流式优化状态机 phase: idle/waiting/streaming/done/error/cancelled，双驱动：自动=WS 事件、手动=NDJSON 流）
- 录音编排核心 `useRecorder.js`（2443 行）是**模块级单例**：27-86 行模块级 ref + 约 30 个 let 标志；`useRecorder()` 只挂生命周期并返回 computed。纯逻辑已抽 `composables/recorder/*.mjs` 8 件套（pcm/segmentPolicy/pollPolicy/statusCopy/streamProtocol/streamScroll/trayState/floatingSession）+ `utils/recordingStopState.mjs`/`asyncSingleton.mjs`，全部有 Node 测试锁定
- 录音状态源三组：`appStore.isRecording/isTranscribing`（全局忙碌）、`livePhase`（UI 阶段：idle→connecting→streaming / listening→speaking→segment-processing→finalizing / transcribing→optimizing→completed|error）、`streamingConnectionState`
- 事件流：热键 IPC→useRecorder→HTTP/WS 后端→全局 WS `handleWebSocketMessage`（唯一注册者，处理 11 类事件）→store/托盘/悬浮窗→组件；悬浮窗状态经 `syncFloatingWindow` 每次变化推送（IPC→主进程→floating webview）
- 启动窗口保护：`shouldQueueStopDuringStart`（启动中松手一律排队 pendingStopAfterStart 统一收口，9 种旧×新模式组合收敛到取消路径）；`recoverLeakedTranscribingState` 自愈泄漏状态；流式 15s 收尾看门狗 + 60s 后台优化看门狗

### 5.3 后端记录状态机（app/records/models.py:11-18）

```
pending → transcribing → transcribed → optimizing → optimized
                └──────────┴─────────────┴──────────→ failed
```

约束：自动优化失败必须复位 `transcribed`；转写失败置 `failed` 并广播 error（⚠ 当前不写 error 原因，见 §6-7）。

---

## 6. 已知缺陷清单（重写时不予复刻的行为）

> 完整证据见 docs/已知问题.md（2026-10-10 审计 + 后续修复记录）。仍存在且影响重写设计的：

| # | 缺陷 | 对重写的启示 |
| --- | --- | --- |
| 1 | `--win` 打包 nsis 阶段失败 + 打包漏 `scripts/`（桥接起不来） | Tauri 打包需把 Rust 侧桥接/模型服务纳入产物清单并有自检 |
| 2 | 多段录音最后一片上传失败 → `recorder.onstop` 无 catch → 永久卡「正在转写」、录音丢失 | Rust 侧录音收尾必须有超时兜底 + 失败可恢复路径 |
| 3 | 音频文件名与 record.id 完全错位（719 记录 vs 768 wav，交集 0；13 条 audio_path 悬空） | 重写数据层必须 id 即文件名，并提供旧数据迁移/清洗 |
| 4 | 音频无 retention，data/ 只涨不降（实测 505MB/768 wav） | Rust 侧需内置音频清理策略（housekeeping 增强） |
| 5 | 转写失败不写 Record.error（重启后无失败原因） | 重写必须落错误原因并区分网络/密钥/音频损坏 |
| 6 | 存储层静默吞异常（clipboard manager `except: continue` 无日志） | Rust 侧持久化失败必须可观测 |
| 7 | MAX_UPLOAD_SIZE 半失效（processor 硬编码 500MB 却报 400 格式非法） | 配置单一真相源 |
| 8 | Pydantic v1 写法残留（升级即崩） | 无迁移负担（Rust 重写天然消除） |
| 9 | 全局 WS 固定 3s 重连无退避无上限、onerror 不触发重连、收尾只由 WS 事件驱动无超时降级 | 事件总线需带指数退避 + 上限 + 超时降级 |
| 10 | `desktop:agent-ready` 主进程 send 但 preload 未暴露（contextIsolation）→ 渲染层永远收不到 | IPC 契约需要类型化/契约测试 |
| 11 | 文案被测试锁死（真流式「复制或粘贴」vs 其他「复制并粘贴」） | 重写时统一并同步测试 |
| 11b | **打包态新发现（本次审计，未实测）**：① 三个 Python helper（desktop_agent/paste_helper/audio_helper）位于 app.asar 内，外部 Python 进程无法读取（无 asarUnpack）→ 打包态热键/粘贴/静音可能整体失效；② manager.js 与 runtime-manager.js 打包态 Windows python 路径不一致（`Scripts/python.exe` vs `python.exe`）；③ 剪贴板图片落盘路径打包态解析到 asar 内只读路径 | Tauri 重写天然无 asar 问题（Rust 内嵌）；但要在阶段③验证打包自检覆盖 |
| 11c | 前端：`settingsStore.llmWizardMode` 未声明 ad-hoc 属性（LLMServiceSection.vue:196 写/LLMWizardModal.vue:169 读）；RecordsLibraryPage deep watch+防抖整页重拉；platform 回退值不一致（settings.js:13 'win32' vs useElectron.js:249 'web'）；useClipboard.importCurrentClipboard 死代码；main.css `.record-edit-display:empty::before` 重复定义 | 重写时清理；Tauri 迁移时 electronAPI 泄漏 2 处（client.js getPort、settings.js platform）一并收口 |
| 12 | 2026-10 已修复项（LLM 超时/思考强度默认 high 吃满 max_tokens/开头丢字/幽灵录音/丢尾包等）| 这些修复的**语义**必须在重写中保留，详见 §5.1 防御性设计 |

---

## 7. 重写功能对齐核对表（阶段②③使用，逐项确认后勾选）

### 7.1 功能对齐（现状 → 重写）

| # | 功能域 | 现状要点 | 重写要求 | 增/保/删 |
| --- | --- | --- | --- | --- |
| 1 | 单段录音 | MediaRecorder webm 整段上传→后台转写→轮询+WS | 保留语义；Rust 侧接管采集与编排（阶段②定采集位置：渲染进程 WebAPI 或 Rust cpal） | 保 |
| 2 | 多段录音 | 静音检测切段（2.2/10/0.8s 策略）+分段上传+merge；有「最后一片失败卡死」缺陷 | 保留模式；收尾必须有超时兜底+失败恢复（缺陷 #2 不复刻） | 保+修 |
| 3 | 流式录音 | AudioWorklet 16k PCM→WS 二进制帧；防丢开头/丢尾/幽灵采集三防御 | 保留三防御语义；缓冲/背压/线程模型按任务要求重新设计并说明 | 保+强化 |
| 4 | 全局快捷键 | pynput 按住模式，20 条缺陷（§4.3） | 任意键+组合、点按/按住双模式、冲突检测、三端一致、无粘滞 | 重做（核心） |
| 5 | 录音期间系统静音 | Windows pycaw/macOS osascript/Linux 不支持；句柄固化/崩溃不恢复 | 三端实现；设备热切换跟随；异常路径恢复 | 保+修 |
| 6 | 悬浮窗 | 512×40 透明置顶底部居中；6 phase 文案+实时滚动文本+自动隐藏 | 保留全部交互；Tauri 多窗口重建 | 保 |
| 7 | 托盘 | 四态图标+最近 10 条菜单+复制并粘贴+唤起窗口 | 保留；Windows DPI 多档图标 | 保 |
| 8 | 剪贴板监控+剪贴板页 | 800ms 轮询去重入库+搜索/收藏/置顶/图片预览 | 保留；修打包态图片路径缺陷 | 保+修 |
| 9 | 记录库 | CRUD/搜索/收藏/编辑/导出 TXT+JSON/播放/乐观锁/批量删 | 保留；修「音频文件名与 id 错位」（数据迁移） | 保+修 |
| 10 | 文件转写 | 多引擎三卡+LLM 滑窗校对+三视图 diff+缓存+202 异步+会话历史 | 保留 | 保 |
| 11 | LLM 优化 | v2 档案/模型/默认+备用、流式 NDJSON、思考强度、降级标记 | 保留协议；Rust 侧需替代 LiteLLM（OpenAI 兼容路由）或保留桥 | 保 |
| 12 | 设置页 | 4 分组+自动保存+只写密钥+快捷键录入 | 保留；新增快捷键双模式/冲突检测 UI | 保+增 |
| 13 | 模型服务管理 | 三模式 provider/bridge/online+健康探测+启停 | 保留语义；健康 27 字段双命名收敛 | 保+简化 |
| 14 | 提示词预设 | 只读 6 预设+设默认 | 保留 | 保 |
| 15 | 粘贴模拟 | spawn paste_helper.py | Rust 原生键盘模拟（enigo 类方案） | 保（换实现） |
| 16 | 后端事件总线 | 全局 WS 12 类事件 | Tauri 下改为进程内事件（或保留 WS 形状给前端）；加指数退避 | 保+修 |
| 17 | Toast/动效 | 令牌体系+基础过渡 | 阶段③按 /ui-ux-pro-max 补波形/页面切换/加载动效 | 增 |
| 18 | 单实例锁 | Electron requestSingleInstanceLock | Tauri single-instance 插件 | 保 |

### 7.2 明确不迁移（历史遗留/已下线）

统一浮层、生词本（wordbook/）、AI 动作、data/prompts/presets.json、llm_configs.json v1（迁移逻辑可留只读兼容）、/api/streaming/service* 别名、聚合路由层 routes.py、statusCopy 文案分歧（统一后同步改测试）。

### 7.3 数据迁移需求（重写必须提供）

- data/settings.json + llm_v2.json（**含 Fernet 密文——Rust 需兼容读取旧密文**）+ records/*.json + sessions/*.json + clipboard/* + transcribe_file_cache.json → 新存储格式（或直接沿用 JSON 布局最小化迁移）
- 修复型迁移：音频文件名与 record.id 错位（719 记录 vs 768 wav，交集 0；13 条 audio_path 悬空）——按内容/时长启发式重建映射或标记孤儿
- userData/desktop-settings.json（hotkey+静音策略）→ 收敛进 Rust 单一配置源
