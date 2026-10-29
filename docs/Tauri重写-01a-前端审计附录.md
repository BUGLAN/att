# ATT 前端 src/ 梳理报告（Tauri 2.x 重写基线）

> 范围：`E:\repo\att` 的 `src/`、`vite.config.js`、`tests/*.mjs` 文件名。所有结论均出自实际读到的代码，引用 `文件路径:行号`；不确定处标「未确认」。Electron 主进程（`electron/`）不在本次范围，仅在改造面清单中提及职责映射。

---

## 1. 应用入口与页面结构

### 1.1 双入口（无 vue-router）

Vite 双 HTML 入口（`vite.config.js:19-23` rollupOptions.input）：

| 入口 | HTML | JS | 渲染目标 | 说明 |
| --- | --- | --- | --- | --- |
| 主窗口 | `src/index.html` | `src/main.js` | `App.vue` | 装 Pinia；引 tokens→main→electron-overrides 三层样式（`src/main.js:1-13`） |
| 悬浮窗 | `src/floating.html` | `src/floating.js` | `FloatingWindow.vue` | **不装 Pinia**；只引 tokens + floating.css（`src/floating.js:1-7`） |

- `index.html:7` 带 CSP meta：`connect-src ws://localhost:* ws://127.0.0.1:* http://localhost:* http://127.0.0.1:*`、`media-src ... http://127.0.0.1:*`——Tauri 迁移时需追加 `ipc: http://ipc.localhost`。
- dev server：`127.0.0.1:8090` strictPort（`vite.config.js:31-37`）；alias `@ → src`；manualChunks 把 vue+pinia 单独分包（`vite.config.js:24-28`）。

### 1.2 页面切换：App.vue 内状态切换，无路由

`App.vue:23-42` 用 `v-show="currentPage === '…'"` 挂五页（全部常驻，不销毁）；`currentPage` 来自 `appStore.currentPage`（`src/stores/app.js:60`），由 `Sidebar.vue:159-161 navigateTo()` 切换。弹窗按 `appStore.modals` 的布尔位 v-if 挂载（`App.vue:45-50`），Toast 全局常驻（`App.vue:52`）。

| 页面 key | 组件 | 职责 |
| --- | --- | --- |
| home | RecorderSection + RecordsSection | 录音台（三模式）+ 首页最近记录列表 |
| transcribe | TranscribePage | 本地音视频文件转写（多引擎 + LLM 校对） |
| records | RecordsLibraryPage | 记录库：搜索/筛选/分页/收藏/导出 |
| clipboard | ClipboardPage | 剪贴板历史（文本/图片，收藏/置顶） |
| settings | SettingsPage（含 LLMServiceSection） | 全部设置 + LLM v2 配置 |

App.vue 启动时还做：挂 paste/navigate/桌面错误四组 Electron 事件监听（`App.vue:108-125`）、`settingsStore.loadSettings()`、`syncDesktopSettings({pause_audio_on_recording})`、`connect()` 全局 WS（`App.vue:127-137`）、`useTrayIcon()` 托盘派生（`App.vue:97`）。

---

## 2. 用户可见功能完整清单（重写对齐基线）

### 2.1 全局壳（TitleBar / Sidebar / Toast / 托盘 / 热键）

**TitleBar**（仅 Electron 环境渲染，`App.vue:10`）：
- 品牌：LogoMark 图标 + "Audio To Text" 文案（`TitleBar.vue:4-7`）
- 拖拽区双击 = 最大化/还原（`TitleBar.vue:3`）
- 最小化 / 最大化 / 关闭 三按钮（`TitleBar.vue:11-23`，经 `useElectron().windowControl`）

**Sidebar**：
- 5 个导航项：首页/转文字/记录集/剪切板/设置，内联 SVG 图标 + active 金色高亮（`Sidebar.vue:13-73`，样式 `main.css:616-699`）
- 底部连接状态：绿点「已连接后端」/ 红点「未连接后端」（`Sidebar.vue:76-80`，来自全局 WS 状态）
- DEV 模式额外显示「N 个客户端在线」（`Sidebar.vue:82-85`，`import.meta.env.DEV` 门控）
- 宽度拖拽：右缘手柄 pointer 拖动实时预览，≤110px 显示折叠意图箭头，释放即折叠（60px）或展开；折叠态与宽度持久化到 localStorage `att.sidebar.layout`（`Sidebar.vue:88-194`、`stores/app.js:4,103-143`）；拖动期间全局 cursor 锁 col-resize（`App.vue:155-159`）

**Toast**：底部居中，最多 3 条同屏堆叠，tone=success/error/info 三色圆点，默认 3s 自动消失（`stores/app.js:12-13,203-227`、`Toast.vue`）。

**托盘联动**（不可见 UI 但用户可感知）：
- 图标四态：idle/recording/transcribing/disconnected（`recorder/trayState.mjs:14-19`）；useRecorder 在自身时序显式切图标，`useTrayIcon.js:20-30` 再 watch `isRecording/isTranscribing/isConnected` 派生补「后端断连」边
- 记录增删改后把最近记录推给托盘菜单（`stores/records.js:19-28 syncTrayMenu → tray.updateMenu`，菜单点击行为在主进程侧实现，前端不可见）

**全局热键**（桌面端推送，前端只收事件）：
- 长按开始录音 / 松开结束（`useRecorder.js:2363-2391`）
- 「粘贴完成」事件 → Toast「已复制到剪贴板」（`App.vue:109-111`）
- 「跳转设置」事件（托盘菜单/悬浮窗触发）→ `setCurrentPage('settings')`（`App.vue:113-115`）
- 桌面权限不足 / 桌面代理错误 → Toast（`App.vue:117-125`）

### 2.2 首页（RecorderSection + RecordsSection）

**RecorderSection**（`src/components/home/RecorderSection.vue`）：
- 页头右侧「模型服务」开关按钮，状态文案：开启模型/模型预热中/模型已开启/模型加载中/当前模式不可用；录音/转写中禁用（`RecorderSection.vue:179-216,424-452`）
- 两枚徽章：模式徽章（单段录音 / 多段准实时 / Qwen 真流式 / 豆包在线流式，218-222）+ 状态徽章（就绪/录音中/处理中/优化中，含语义色 err/warn/ok，230-242）
- 大录音圆钮：aria-label 随状态变（开始录音/结束录音/模型加载中/请先开启模型/请先配置 ASR 密钥/当前环境暂不支持流式/当前模式不支持开启模型，201-209）；录音/转写/流式不可用时禁用（194-200）
- 等宽计时器 `mm:ss`（244-248）；9 根波形条，multi/streaming 模式随实时音量 audioLevel 跳动，single 固定幅度，空闲低幅（282-292）
- panelStatus 随 livePhase 变化（250-268）：正在连接流式转写服务 / 正在生成最终转写结果 / 正在连续流式转写 / 正在捕获当前语音片段 / 上一段已送去转写 / 正在合并整场录音结果 / 静音切段监听中 / 正在录制整段音频 / 正在处理录音结果 / 正在后台优化文本 / 等待开始录音
- panelHint 模式说明三态（270-280）
- 实时转录面板：LIVE 指示灯文案 RECORDING / LIVE AI / LIVE STREAM / PROCESSING / STREAMING / STANDBY（341-348）；实时文本；占位文案按模式+可用性（流式不可用时优先展示后端 streaming_error 原因，315-339）
- LLM 流式优化面板（非录音中且 optimizationStream 活跃/错误/取消时，350-358）：等待首字 spinner、流式文本+打字光标、错误行、waiting 期间原文预览「原始转写：…」（88-121）；仅自动优化（source=auto）时显示「取消」按钮（375-379）
- 转录区自动滚动：用户向上滚离底部 >48px 后停止跟随，新优化会话恢复（`streamScroll.mjs` + `RecorderSection.vue:381-414`）
- 底部信息条：状态 / 片段数（multi 显示「N 段」，streaming 显示阶段文案）/ 模式（完整录音/自动切段/连续流式，129-133,298-313）
- asr_provider 或 recording_mode 变化时重新拉取运行时状态（probe_runtime=true，471-477）

**RecordsSection**（首页最近记录，`src/components/home/RecordsSection.vue`）：
- 标题 + 「N 条」徽章；「选择模式/退出选择」按钮（9-12）
- RecordCard 每条记录（`RecordCard.vue`）：
  - 播放/暂停试听按钮（互斥全局播放，`useRecords.js:248-295`）
  - 文本预览两行截断；流式优化中显示流式文本+光标、否则优化文→原文→「处理中...」（197-202）
  - 元信息：时长、时间（今天显示时刻/昨天/月日，228-246）、处理耗时（有值时）
  - 状态徽章：优化中(warn)/转写中(accent)/优化中(optimizing)/失败(err)/已编辑(info)/已优化(warn)（53-67）
  - 动作按钮：「优化」（transcribed/failed 且尚无优化文时，70-76）、「取消」（流式优化中，78-85）、「复制」（88-94）、导出 TXT 图标钮（96-111）、导出 JSON 图标钮（113-127）、收藏/取消收藏星标（129-147）
  - 交互：整卡左键 = 复制（204-211）；右键 = 打开编辑弹窗（213-219）；选择模式下整卡 = 勾选
- 选择模式：每卡复选框 + 底部工具条「全选 / 删除选中(N) / 取消」，删除走 ConfirmDialog 二次确认（`RecordsSection.vue:41-65`）
- 空态 EmptyState「暂无录音记录」

**RecordEditModal**（`modals/RecordEditModal.vue`）：原始文本展示区（点击复制）、优化文本展示区（点击复制）、编辑 textarea（初始值=优化文优先）、保存（写 optimized_text 或 raw_text 并置 is_modified=true，64-81）、取消；Esc/遮罩关闭由 Modal 原语提供。

### 2.3 转文字页（TranscribePage，文件转写）

- 页头：眉题 "LOCAL FILE · 本地文件转写台" + 大标题「转文字」+ 说明（视频自动抽音轨/秒回缓存/长文件后台任务）
- **整页拖拽投放**：dragenter/leave 计数防抖，悬停显示遮罩「松手，开始转写」（`TranscribePage.vue:5-8,31-33,367-395`）；多文件取第一个并 Toast 提示；转写中拒绝
- 「选择文件」按钮 → Electron 系统对话框（`useFileTranscribe.js:67-80 chooseFile → openMediaFile`）；路径输入框（可手粘绝对路径，回车即转写）；「开始转写/转写中」按钮
- 波形母题三态动画：idle 静默 / live(转写中或拖拽)跳动 / done 归一线（352-360，样式 570-626）
- **引擎三卡**（`fileTranscribeModel.mjs:27-47`）：faster-whisper（本地/默认）、Qwen3-ASR（本地）、豆包·实时大模型（在线）；豆包未配 Token 时置灰 + 红点 + 警告「未配置 Access Token — 去设置」（可点击跳设置页，`TranscribePage.vue:83-98,314-328`）
- **LLM 校对面板**（默认关，102-144）：开关（开启时懒加载 LLM 模型列表，空则 Toast 提示去配置，330-338）；LLM 模型下拉（默认模型/各模型条目）；窗口批大小 20/40/60 段；上下文重叠 200 字/不携带
- 参数行：LANG（zh/en/ja/ko）、SEG 分段时间戳复选（146-164）
- 状态行（等宽字体+块状光标）：提交/等待秒数/后台任务/「LLM 校对中 · 窗口 x/y」（`useFileTranscribe.js:126-131`）；错误走红色告警条
- **结果卡（磁带日志风）**：文件名、时长 chip、引擎、缓存命中金色斜印章、「已校对（·部分保留原文）」chip；有校对原文时出现三视图 tab：校对后 / 原文 / 对照（逐句 diff：改动句上红删除线+下绿修正，未变句灰，含「共 N 句 · M 处修改」摘要，`fileTranscribeModel.mjs:95-128`）；「复制全文」（成功后 2s 显示「已复制 ✓」）；可折叠时间轴（start 秒 + 点线引导 + 文本，233-242）
- 空态提示行（支持的容器列表、「结果不会进入记录集」）
- **本次会话历史架**：最近 20 条（`useFileTranscribe.js:19`），编号/文件名/引擎/60 字预览/时长，点击切换回看结果（253-269）

### 2.4 记录集页（RecordsLibraryPage）

- 搜索框（300ms 防抖）+ 范围下拉（全部文本/仅原始文本/仅优化文本）+「筛选收藏/仅看收藏中」toggle（`RecordsLibraryPage.vue:15-35`）
- 「N 条」徽章；加载时 6 张骨架卡；空态「没有匹配的记录」
- SettingsRecordCard 卡片网格（`SettingsRecordCard.vue`）：时间、状态徽章（待处理/转写中/已转写/优化中/已优化/失败/已编辑，109-142）、收藏星标、文本 5 行截断、「优化文本/原始文本」标识、复制、导出 TXT、导出 JSON、点卡片 = 打开 RecordEditModal
- 分页：上一页/下一页 + 「第 x / y 页」，page_size=100（60-78,111）
- 收藏取消时若处于「仅看收藏中」则即时从列表移除（177-199）

### 2.5 剪切板页（ClipboardPage + ClipboardCard）

- 搜索框（图标内嵌 bare Input，250ms 防抖）+「刷新」按钮（`ClipboardPage.vue:8-27`）
- 快捷过滤胶囊行：「今天 N 条记录」/「收藏 N」/「置顶 N」+ 类型过滤（纯文本/图片/文件/链接）（29-64）
- 加载骨架 ×6；空态大号 EmptyState
- ClipboardCard（`ClipboardCard.vue`）：
  - 类型徽章（文本/链接/长文本/文件/图片，`clipboardPresentation.js:20-26`）+ 相对时间（刚刚/N 分钟前/N 小时前/昨天时刻/年月日时，146-182）
  - 标题（metadata.display_name → file_name → title → 未命名图片/记录，`clipboardPresentation.js:52-70`）、摘要 6 行截断（图片显示宽×高）
  - 图片卡：悬停弹出预览层——懒加载（fetch 图片端点→blob→dataURL，184-212），「图片加载中.../图片不可用」态；卡片提示「悬停查看缩略图」
  - 大小（N 字符 / KB）；动作：置顶/收藏（**乐观更新+失败回滚+按钮忙碌禁用**，`ClipboardPage.vue:218-262`）、复制（文本走 writeText，图片走 writeImage + 元数据，`useClipboard.js:89-101`）、删除（ConfirmDialog 二次确认）
- 剪贴板变化 Electron 事件自动刷新列表（270-276），卸载时移除监听（278-280）
- 注意：`useClipboard.importCurrentClipboard`（手动导入当前剪贴板，`useClipboard.js:48-71`）在页面中**无任何入口**（死代码候选，未确认是否有意保留）

### 2.6 设置页（SettingsPage + LLMServiceSection）

**通用行为**：初始化完成前显示「正在读取已保存的设置…」并禁用全部控件（fieldset，`SettingsPage.vue:8-9`）；表单 watch 深度监听 + 500ms 防抖自动保存；快照对比避免重复提交；服务端推送不覆盖未提交编辑（`settingsFormSync.mjs`）；Token 明文只在输入时提交、保存后即清空表单（763-769）。

**分组 1：语音识别**（10-237）
- ASR Provider 下拉：whisper / qwen_asr / doubao_asr / chatgpt_asr（切换提示需重新开启模型）
- Whisper 条件组：模型（tiny→large-v3 五档）、设备（自动/CUDA/CPU）
- Qwen3-ASR 条件组：模型名、推理设备、dtype（auto/bf16/fp16/fp32）、最大输出 token、返回时间戳开关、音频长度上限、强制对齐器开关（依赖时间戳开启）+ 强制对齐器模型名
- 豆包条件组：Access Token（password 只写不读，已保存显示「留空表示不修改」）、清除已保存 Token 开关、Resource ID 四选（ASR 2.0/1.0 × 时长/并发）、接入点 wss 地址、强制断句静音时长（ms）
- ChatGPT 听写条件组：访问令牌（password）、清除令牌开关、端点、请求超时（秒）

**分组 2：录音设置**（239-366）
- 录音模式下拉：单段/多段/流式（三模式切换入口）
- 流式运行时状态卡：可用/不可用徽章 + 详细原因（后端环境/桥接/错误，449-465）
- 静音分段阈值（仅 multi 模式显示，0.5-5s 步进 0.5）
- 麦克风设备下拉（先 getUserMedia 授权再 enumerateDevices，678-686）
- 录音时暂停其他音频（toggle，经 Electron 桌面端）
- 自动 AI 优化（toggle）
- 自动复制到剪贴板（toggle：完成自动复制并尝试粘贴到焦点输入框）
- 默认优化预设下拉 + 「查看提示词」按钮（打开只读 PresetEditModal）
- 录音快捷键：点击输入框进入监听态（金色脉冲边框），按下组合键即录入（修饰键≥2 或修饰键+单键）；「恢复默认」按钮（平台默认：darwin=⌘⇧R / win32=Win+Alt / 其他=Ctrl+Shift+R，476-498）；保存后经 `syncInputSettings` 同步到桌面端（771-776）

**分组 3：LLM 服务**（LLMServiceSection，v2 档案+模型）
- 头部：「添加供应商」「添加模型」按钮；说明「LiteLLM 统一驱动」
- 损坏配置跳过警告条（N 条旧配置被跳过）
- 供应商档案卡列表：logo 字母、名称+模型数、provider·端点·密钥状态（已保存(尾号)/未设置/无需密钥，`llmLogic.mjs:173-178`）、验证状态（已验证·延迟/验证失败/密钥无法解密/未验证）、「重新验证」「编辑」
- 模型表格：显示名（+「默认优化」金 chip /「备用」chip）、失败原因行、LiteLLM 模型串、参数摘要（temp/max/超时/思考，`llmLogic.mjs:154-170`）、状态、动作：「测试」「设为默认」「编辑」「删除」（删除二次确认）
- 底部提示：默认模型 + 备用模型（默认不自动切换，避免意外费用）
- 空态文案两处（无档案/无模型）

**分组 4：诊断**（372-400）：后端连接徽章 + WS 客户端数；运行环境 + 模型开关状态徽章

**LLM 相关弹窗**：
- LLMWizardModal（三步向导）：①供应商选择（可搜索网格，含 litellm 前缀与是否需密钥）②凭据（档案名/API Key/API Base custom 必填）→「验证并选择模型」先草稿验证（verify-draft，密钥不落盘）通过才建档案 ③模型选择：远端模型列表多选（拉取失败降级手输）、手输解析（逗号/分号/换行分隔，非法名提示忽略）、参数折叠区（temperature/max_tokens/timeout_s/retries/思考强度五项）、「保存后设为默认优化模型」复选；从「添加模型」入口进入时跳过 ①② 直达 ③
- LLMProviderEditModal：档案名/密钥（留空保持）/API Base；验证横幅（验证前先静默落盘，所见即所测）；「删除档案」（二次确认，提示级联删模型）
- LLMModelEditModal：显示名/模型串（提示前缀组装规则与完整串预览）/五参数网格；「测试」（先静默保存再验证）、「设为默认」、「删除」（二次确认）；思考强度五档（跟随默认/关闭/低/中/高，`llmLogic.mjs:139-145`）及「思考 token 计入 Max Tokens」警示文案

**预设弹窗**：PresetModal（列表 + 默认徽章 + 「查看」）与 PresetEditModal（只读：名称/描述/提示词模板）——预设只读，真相源是 prompts/*.txt。

### 2.7 悬浮窗（FloatingWindow，独立入口）

- 胶囊 UI：状态图标（录音金色脉冲环 / 错误红）、5 根波形（recording 跳动）或 spinner、前缀+内容文本、右侧两按钮
- 前缀文案：正在录音/实时记录(multi·streaming)/正在转写/正在优化/转写完成/处理失败（`FloatingWindow.vue:109-117`）
- 内容：multi/streaming 录音中与 optimizing 时显示**实时滚动文本**（mask 渐隐视口 + rAF 缓动跟随 + 打字光标，129-225）；其余阶段显示固定提示
- 「打开设置」按钮 → floatingAPI.openSettings（主窗口跳设置页）；「关闭悬浮窗」按钮
- complete 2s / error 3.2s 后自动隐藏（93-94,154-172）
- 状态由主进程经 floatingAPI.onStateChange 推送（phase/text/mode）

---

## 3. 样式实现方式

### 3.1 设计令牌（tokens.css，232 行）

唯一令牌源，两入口最先引入（`src/main.js:1-2`、`src/floating.js:1-2`）。`:root` 内 12 类变量（行号见 `src/styles/tokens.css`）：
1. 原始灰阶 gray-950→gray-50（16-27）
2. 语义表面五级 bg-base/surface/card/raise/hover(+strong) + 描边 line 三档（29-39）+ 半透明浮面 surface-float
3. 半透明白 white-veil/tint(+strong)（46-48）
4. 遮罩 scrim 三档（54-56）
5. 文字三级 text-1/2/3，注释标注实测对比度全部 ≥4.5:1（58-61）
6. 唯一金色 accent 家族 13 档（63-78）
7. 语义色 ok/warn/err/info + soft/line/tint 衍生（80-101）
8. 间距 s-1~s-12（8 基数）、圆角 r-sm~r-full 五级、字号 fs-cap~fs-2xl 七级（103-124）
9. 阴影 e-1~e-4 + e-hover、描边高光 edge-hi、焦点环 focus-ring/halo（126-134）
10. 表面渐变 grad-* 五条（136-143）
11. z-index 八级（145-153）、动效三档时长+统一缓动（155-159）
12. 字体栈纯系统字体（离线，161-163）、统一滚动条（165-170）
13. 旧名兼容别名（172-213）保证旧类名无改动享受新令牌
14. 末尾 `prefers-reduced-motion` 全局动效降级（223-232）

`color-scheme: dark`（13 行）——**暗色是唯一主题，无浅色模式/主题切换**。

### 3.2 全局样式（main.css，1466 行）

reset、body 噪点纹理（SVG feTurbulence data URI 叠加，80-91）、`.container` 1180px 容器、全局统一滚动条、`.modal/.modal-content` 体系、`.record-item` 记录卡、`.sidebar` 全套（含折叠态）、`.settings-*` 设置页栅格（两列 minmax(0,1fr)+320px，982-996）、`.toggle-switch` 自绘开关、`.hotkey-input`、`.preset-*`、`.llm-config-*`、统一焦点环（332-355）、响应式 640/768/1040 断点。**不再声明 :root，不加载远程字体**（1-10 注释）。

### 3.3 electron-overrides.css（275 行）与 floating.css（302 行）

- **electron-overrides.css 名字有误导**：内容其实是 ①自绘标题栏（.title-bar 及按钮，4-88）②记录集页/设置记录卡/分页（.records-library-page、.settings-records-*、.settings-record-card、.records-pagination，35-275）。与 Electron 的真正耦合只有两处：`-webkit-app-region: drag/no-drag`（26,51）与 `env(titlebar-area-x)`（17，无 WCO 时取 0，`max(12px, …)` 兜底）
- **floating.css**：悬浮窗专属——透明 body、pill 网格布局、波形/脉冲/spin/cursorBlink 动画、流式滚动 mask 渐隐（207-219）；明确不再自定义 :root（1-4 注释）

### 3.4 scoped 与全局的划分

- **scoped**：common 全部原语（BaseButton/BaseInput/BaseSelect/BaseTextarea/BaseBadge/EmptyState/Skeleton/PageHeader/LogoMark/Toast/ConfirmDialog）+ 页面组件 RecorderSection、TranscribePage、ClipboardPage、ClipboardCard、LLMServiceSection、3 个 LLM 弹窗、FloatingWindow、SettingsRecordCard/RecordCard 局部、App.vue 局部
- **全局**：布局骨架与跨组件类（sidebar/settings/records/modal/record-item/preset/llm-config）在 main.css 与 electron-overrides.css
- 原语支持 CSS 变量注入定制（`--ui-icon-size`、`--ui-btn-radius`，如 `RecordCard.vue:22`、`ClipboardCard.vue:12`）

### 3.5 字体与图标

- 字体：`--font-sans` 系统栈（Segoe UI/PingFang SC/Microsoft YaHei…）、`--font-mono`（Consolas 等），无远程字体
- 图标：全部内联 SVG（stroke=currentColor，继承文字色），无图标库依赖；LogoMark 用 `<img>` 引 `src/assets/logo-mark.svg`（Vite 内联 data URI，`LogoMark.vue:6-9`）

### 3.6 迁移 Tauri 时样式保留/适配判定

**原样保留**：tokens.css、main.css、floating.css 全部；electron-overrides.css 绝大部分；全部组件模板与 scoped 样式。WebView2/WebKit 均支持所用特性（backdrop-filter、mask-image、CSS 变量、feTurbulence data URI）。

**需适配**（共 3 处）：
1. `.title-bar-drag` 的 `-webkit-app-region: drag` → Tauri 的 `data-tauri-drag-region` 属性（`electron-overrides.css:26`，对应 TitleBar.vue:3 模板加属性）；`-webkit-app-region: no-drag` 同理（:51，按钮容器本就是 no-drag，Tauri 默认按钮可点，风险低）
2. `env(titlebar-area-x)` 在 Tauri 无 Window Controls Overlay，恒取 0，`max(12px,0)` 兜底已安全，可不动
3. `index.html:7` CSP 需加 `ipc: http://ipc.localhost`（Tauri IPC）；`media-src/connect-src` 的 localhost 放行已兼容本地后端

---

## 4. 状态管理与数据流

### 4.1 四个 Pinia store

| store | 关键 state | 职责 |
| --- | --- | --- |
| `app`（stores/app.js） | currentPage、sidebarCollapsed/Width（localStorage 持久化）、isRecording/isTranscribing、currentRecordId/currentRecord/recordStatus、recordingDuration、isConnected/connectedClients、modals 六开关、toasts 队列、selectMode/selectedRecords | UI 全局壳状态；Toast 队列（定时器 Map 防泄漏，216-223）；侧边栏宽度钳制 60-280px、折叠阈值 110px |
| `settings`（stores/settings.js） | settings（约 40 键默认值，109-146）、runtimeStatus（新旧双命名 health 字段全收，150-174）、presets/defaultPresetId、LLM v2 全家（llmProfiles/llmModels/llmProviders/llmDefault/Fallback/Skipped/currentProfile/currentModel） | 设置读写（HTTP+桌面设置双源合并，199-258）、运行时状态、模型服务启停+就绪轮询（303-439）、预设只读加载、LLM 全部 CRUD 动作（写后统一重拉 state） |
| `records`（stores/records.js） | records、currentRecord、isLoading/error | 记录 CRUD 封装；sortedRecords 按 created_at 倒序；**每次变更 syncTrayMenu 推托盘**（19-28） |
| `optimizationStream`（stores/optimizationStream.js） | recordId、text、phase(idle/waiting/streaming/done/error/cancelled)、source(auto/manual)、startedAt/firstTokenAt | 单槽 LLM 流式优化状态机；begin/append/finish/fail/cancel/reset |

### 4.2 composable 与 store 的关系

- `useSettings/usePresets/useLLMService`（composables/useSettings.js）：薄封装——store 动作 + toast 反馈语义（保存成功/失败、设默认等）
- `useRecords`（composables/useRecords.js）：动作层 + 模块级播放状态（currentAudio/playingRecordId 单例）+ 手动流式优化全流程（optimizeRecord：AbortController、同记录复用、不同记录先取消旧流、首增量前失败降级非流式端点、取消/失败恢复原文显示，65-191）
- `useRecorder`：重编排，模块级单例状态（见 §5）
- `useClipboard`/`useFileTranscribe`：自持局部状态，不经 store
- `useElectron`：桌面能力唯一包装层（见 §6）
- `useTrayIcon`：watch 派生托盘图标
- `useSilenceDetector`：工厂函数（非 composable），multi 模式静音检测

### 4.3 录音状态机

状态源有三组：
- `appStore.isRecording / isTranscribing`（全局忙碌标志，按钮禁用/徽章依赖）
- `livePhase`（useRecorder 模块级，UI 阶段文案）：`idle → connecting(流式) → streaming(流式) / listening → speaking → segment-processing → finalizing(multi) / transcribing(悬浮窗) → optimizing → completed | error`
- `streamingConnectionState`：idle/connecting/streaming/error

典型流转（single）：
`prepareRecordingEnvironment`（**先置位 startPending/isRecording/floatingSessionActive 再 await 麦克风**，631-698）→ `start(1000)` → recording → 松手 `stopSingleRecording`（derivePostStopState：≥1s → transcribing=true，`recordingStopState.mjs:1-33`）→ `onstop → uploadSingleRecording` → HTTP 上传 → `transcription_started/completed`（WS）或 1Hz 轮询兜底 → 未开自动优化即完成粘贴；开了则 `optimization_*` 事件驱动 optimizing→optimized（后台优化期间可开下一段，record_id 归属判定防止旧事件劫持新会话）。

启动窗口保护：`shouldQueueStopDuringStart`（startPending 且非 fromStart → 排队 pendingStopAfterStart，`recordingStopState.mjs:60-62`）；`recoverLeakedTranscribingState` 自愈泄漏的转写中（1729-1748）。

### 4.4 事件流（Electron → composable → store → 组件）

```
桌面热键 IPC(长按/松开)
  → useElectron.onHotkeyPress/onHotkeyRelease（useRecorder.js:2363-2391 挂载）
  → useRecorder.startRecording/stopRecording（按 recording_mode 分派）
  → getUserMedia / MediaRecorder / AudioWorklet / 流式 WS
  → 后端 FastAPI（HTTP 上传 / WS 事件）
  → 全局 WS useWebSocket.onmessage → messageHandlers 广播
  → useRecorder.handleWebSocketMessage（1906-2249，11 类事件）
      transcription_started/completed、segment_transcribed/failed、
      optimization_started/delta/completed/cancelled/failed、error
  → recordsStore.updateRecordStatus / optimizationStream.begin·append·finish
     / appStore.setCurrentRecord / tray.setIcon / floatingWindow.updateState
  → 组件（RecorderSection/RecordCard/FloatingWindow 经主进程事件）响应式更新
```

托盘图标：useRecorder 显式时序调用 + useTrayIcon watch 兜底断连；悬浮窗：`syncFloatingWindow`（111-127）每次状态变化 `floatingWindow.updateState`（IPC → 主进程 → floating webview `floatingAPI.onStateChange`）。

---

## 5. 录音编排深挖（useRecorder.js，2443 行）

### 5.1 结构

- **模块级单例状态**：27-86 行，duration/mediaRecorder/stream/audioLevel/liveTranscript/liveSegments/livePhase/activeRecordingMode/currentSessionId/processedSegmentCount/streamingConnectionState/sessionStatusText 等 ref，加约 30 个 let 标志（startPending、pendingStopAfterStart、floatingSessionActive/RecordId、realtime* 十余个、streaming* 十余个、streamingCaptureGeneration 代次守卫等）。`export function useRecorder()`（2347-2443）只做 onMounted/onUnmounted 生命周期（挂 WS handler 与热键，计数防重复；卸载时全量清理定时器/WS/AudioContext/轨道，2395-2419）并返回 computed 视图。
- **纯逻辑已抽离**到 `composables/recorder/*.mjs` 8 个模块（pcm/pollPolicy/segmentPolicy/statusCopy/streamProtocol/streamScroll/trayState/floatingSession），本文件只保留时序编排（10-18 注释）。
- 主要函数：`prepareRecordingEnvironment`（631）、`createRecorder`（747）、`startSingle/stopSingle`（1591/1799）、`startMulti/stopMulti`（1641/1840）、`startRealtimeSegment/stopRealtimeSegment/finalizeRealtimeSession/uploadRealtimeSegment/maybeFinalize`（1320/1386/1216/1178/1312）、`startStreaming/stopStreaming`（1411/1517）、`connectStreamingSocket`（851）、`setupStreamingAudioCapture`（1060）、`sendStreamingSamples/queueStreamingSamples/takeStreamingSamples`（416/435/409）、`handleWebSocketMessage`（1906）、`pollRecordStatus/startPolling/stopPolling(ForRecord)`（2251/2330/459）、`maybeCopyText`（568，自动复制+粘贴）、`completeFloatingSession/awaitBackgroundOptimization/armOptimizationWatchdog`（135/200/163）、`armFinishWatchdog`（310，15s 流式收尾看门狗）、`handleUnexpectedStreamingClose/handleFinishInterruptedClose`（339/366）、`recoverLeakedTranscribingState`（1729）。

### 5.2 三种模式完整流程

**single（单段）**：`startRecording`（1766，守卫 startPending/isRecording/isTranscribing+泄漏自愈）→ `startSingleRecording` → prepare（悬浮窗先显、麦克风+桌面静音并行、失败完整收口含 Toast「无法访问麦克风」）→ MediaRecorder(webm;codecs=opus，timeslice 1000ms) → 计时钟 → 松手 stop → `derivePostStopState`（<1s →「录音时间过短」直接复位）→ `onstop → uploadSingleRecording`（POST /api/audio/upload，1121-1176）→ 乐观插入 recordsStore + WS status_update + `startPolling` 1Hz 轮询兜底 → WS `transcription_completed`（1988）或轮询到 transcribed（2277）→ `maybeCopyText`（写剪贴板+pasteActive 系统粘贴）→ Toast 三态文案（statusCopy）→ 开自动优化则继续等 `optimization_*`。

**multi（多段准实时）**：`startMultiRecording` → prepare → POST /api/sessions 建会话 → `createSilenceDetector`（独立 AudioContext+Analyser，40ms 采样 RMS→dB；onSpeechStart 开段、onSpeechEnd 停段，useSilenceDetector.js:43-99）→ `startRealtimeSegment`（MediaRecorder timeslice 250ms + 10s 强制切段定时器）→ 每段 stop → `uploadRealtimeSegment`（POST /sessions/:id/segments，门槛判定在 segmentPolicy：非强制须 ≥2.2s、强制时 <0.8s 且全场无语音则丢）→ WS `segment_transcribed` 追加 liveSegments → `updateLiveTranscriptPreview`（按 sequence 排序拼接 \n\n，510-524）→ 松手 `stopMultiRecording`（realtimeStopRequested=true，强制停当前段）→ 全部在途段完成后 `finalizeRealtimeSession`（1216）：无语音则删会话报错；否则 POST /sessions/:id/merge → 拉取/合并 record → 落库 → optimizationStream.finish → 悬浮窗 complete → 复制粘贴 Toast。

**streaming（真流式，Qwen3-ASR/豆包）**：`startStreamingRecording`（1411）→ prepare → provider 支持判定（streamProtocol，仅 qwen_asr/doubao_asr）→ **建连与采集并行**（Promise.all 1446-1449：连接 0.5-0.7s 期间 WebAudio 不会回放，串行会丢开头语音，注释 1440-1445）→ `setupStreamingAudioCapture`：AudioContext 强制 16kHz + addModule(pcm-capture-worklet) + 代次守卫（await 后核对 streamingCaptureGeneration，被收口则自关，1060-1104）→ worklet 每 128 帧拷贝 Float32 postMessage（worklets/pcm-capture-worklet.js）→ `queueStreamingSamples`（音量计算+累积）→ 每 3200 采样（≈200ms）转 PCM16 二进制上行（pcm.mjs）→ WS 消息：stream_ready（拿 session_id，resolve）/stream_partial（实时刷新 liveTranscript+悬浮窗）/stream_finished（record 落库；无语音 record=null 轻提示收尾；开自动优化则悬浮窗转 optimizing+60s 看门狗而非 complete，945-1019）/stream_cancelled/stream_error → 松手 `stopStreamingRecording`：**尾包先冲刷再 cleanup**（1559-1576，防丢尾 ≈200ms）→ send stream_finish + 15s 收尾看门狗 → socket 未就绪时松手=放弃启动（streamingStartAborted，1578-1588）。

### 5.3 静音检测用途（useSilenceDetector.js）

仅 multi 模式：`silence_level`（dB 阈值，默认 -40）+ `silence_threshold`（静音时长 s，min 400ms，SettingsPage 可调）驱动「说完一句停顿自动切段」；同时输出 normalizedLevel 供 UI 音量条。参数来自 settingsStore（1678-1701）。

### 5.4 与后端的交互点汇总

- HTTP：POST /api/audio/upload、POST /api/sessions、POST /api/sessions/:id/segments、POST /api/sessions/:id/merge、DELETE /api/sessions/:id、GET /api/records/:id/status（轮询）
- 全局 WS `ws://127.0.0.1:port/api/ws`：收 11 类事件；发 status_update / ping
- 流式 WS `ws://…/api/asr/stream`：发 stream_start(JSON) → 二进制 PCM16 帧 → stream_finish(JSON)；binaryType='arraybuffer'（861）

### 5.5 floatingSession 与 trayState

- `floatingSession.mjs`：`shouldAwaitBackgroundOptimization`（auto_optimize 且未收过优化完成事件 → 悬浮窗等后台优化，避免 complete 后 2s 自动隐藏期间无提示）与 `ownsFloatingRecord`（record_id 归属判定，未绑定/不匹配一律 false，防旧事件误收新悬浮窗）
- `trayState.mjs`：录音 > 转写 > 断连 > 就绪 四态优先级派生；connected 只认显式 false

---

## 6. 前端对 Electron 的全部依赖点（Tauri IPC 改造面）

### 6.1 useElectron.js 暴露面（`src/composables/useElectron.js`）

**命令类**（window.electronAPI.* 同名方法）：
| 分组 | 方法 | 行号 |
| --- | --- | --- |
| 窗口 | minimize/maximize/close（→minimizeWindow/maximizeWindow/closeWindow） | 66-70 |
| 托盘 | setIcon（setTrayIcon）、updateMenu（updateTrayMenu） | 72-75 |
| 悬浮窗 | show/hide/toggle（showFloatingWindow 等）、updateState（updateFloatingWindowState） | 77-82 |
| 剪贴板 | readText（readClipboardText）、writeText（writeClipboard，失败回退 navigator.clipboard）、writeImage（writeClipboardImage）、pasteActive（pasteClipboard 系统级粘贴） | 86-145 |
| 设置同步 | syncInputSettings（热键）、syncDesktopSettings、getDesktopSettings、saveDesktopSettings、syncRecordingState（录音静音其他音频） | 182-208 |
| 文件 | openMediaFile（系统对话框）、filePathForFile（getPathForFile，拖拽 File→绝对路径） | 210-223 |
| 其他 | getPort（经 re-export getBackendPort）、platform、debugLog | 249-251,13-15 |

**事件监听类**：onHotkeyPress、onHotkeyRelease、onPasteText、onNavigateSettings、onClipboardChanged、onDesktopPermissionRequired、onDesktopAgentError、removeAllListeners(channel)（150-180）。热键用法：长按=开始、松开=停止（useRecorder.js:2363-2391）；channel 名 `'hotkey:press'/'hotkey:release'/'paste-text'/'navigate:settings'/'clipboard:changed'`（App.vue:140-145、ClipboardPage.vue:279）。

**api 对象**（17-64）：40+ HTTP 方法的直通转发（uploadAudio…setDefaultPreset），实际实现全在 `api/client.js`。

### 6.2 调用方清单

- App.vue：onPasteText/onNavigateSettings/onDesktopPermissionRequired/onDesktopAgentError/syncDesktopSettings/debugLog/removeAllListeners（85-94,108-145）
- useRecorder.js：api、tray、floatingWindow、clipboard（maybeCopyText）、syncRecordingState、onHotkeyPress/Release、removeAllListeners、debugLog（贯穿全文件）
- stores/settings.js：api、getDesktopSettings、saveDesktopSettings（106）+ **直读 window.electronAPI.platform**（9-14，见 6.3）
- stores/records.js：api、tray（6,19-28）
- useTrayIcon.js：tray（18）
- useRecords.js / useClipboard.js / useFileTranscribe.js：api、clipboard、openMediaFile、filePathForFile
- TitleBar.vue：windowControl（32）
- SettingsPage.vue：syncDesktopSettings、syncInputSettings、platform（426,771-780）
- ClipboardPage.vue：onClipboardChanged、removeAllListeners（118,271-279）

### 6.3 window.electronAPI 直接引用验证（要求「应当为零」→ **不为零，有 2 处泄漏**）

grep 全 src/ 结果：直接引用共 42 处，分布——
- `src/composables/useElectron.js`（包装层本体，预期内）
- **`src/stores/settings.js:9-14`**：`getPlatform()` 直读 `window.electronAPI?.platform`（违反项目硬约束 #7「桌面能力只走 useElectron」）
- **`src/api/client.js:14-16`**：`getBackendPort()` 直读 `window.electronAPI?.getPort`
- **组件层（.vue）零直接引用 ✅**（TitleBar/SettingsPage 等都经 useElectron 解构）

另有**独立 preload 通道 `window.floatingAPI`**（悬浮窗 webview 专用，FloatingWindow.vue:157/175/179/228/240）：close()、openSettings()、onStateChange(cb)、removeAllListeners('floating:state')——不经 useElectron，是悬浮窗与主进程的专用桥，Tauri 迁移时必须单独映射（Tauri 多窗口 event 系统）。

### 6.4 Tauri 改造面结论

保持 useElectron.js 导出形状不变、内部换成 `invoke`/`listen`（@tauri-apps/api），上游组件/composable 即零改动；仅 6.3 的两处泄漏文件需一并处理。Rust 侧需实现的命令/事件清单即上表 + floatingAPI 四个方法。**长按热键语义**（press/release 两事件）是全局热键插件上需要专门事件化的点；pasteActive（向焦点输入框模拟粘贴）在 Tauri 需 Rust 键盘模拟。

---

## 7. api/client.js（HTTP 包装层）

### 7.1 基址确定

`getBackendPort()`：优先 `window.electronAPI.getPort()`（Electron 主进程分配端口，结果缓存），失败/浏览器环境回退 **8000**（client.js:9-25）→ `getBaseUrl()` = `http://127.0.0.1:{port}/api`（27-30）。dev 与 prod 同一逻辑（无 import.meta.env 分支，dev 下无 electronAPI 即 8000）。

### 7.2 封装的端点（函数 → 端点，全表）

| 函数 | 方法+端点 | 备注 |
| --- | --- | --- |
| uploadAudio | POST /audio/upload | FormData，300s 超时 |
| createSession | POST /sessions | |
| uploadSessionSegment | POST /sessions/:id/segments | FormData |
| mergeSession | POST /sessions/:id/merge | 180s |
| deleteSession | DELETE /sessions/:id | |
| getRecords | GET /records | |
| searchRecords | GET /records/search?q&search_type&favorite_only&page&page_size | |
| getRecord / getRecordStatus | GET /records/:id(\/status) | |
| updateRecord / deleteRecord | PUT/DELETE /records/:id | |
| optimizeRecord | POST /records/:id/optimize | 180s（非流式降级用） |
| optimizeRecordStream | POST /records/:id/optimize/stream | **NDJSON 流式**，eventCount 返回 |
| cancelOptimizeRecord | POST /records/:id/optimize/cancel | 10s |
| getClipboardItems | GET /clipboard/items?query&item_type&favorite_only&pinned_only&limit | |
| getClipboardStats | GET /clipboard/stats | |
| createClipboardItem / updateClipboardItem / deleteClipboardItem | POST/PUT/DELETE /clipboard/items(/:id) | |
| getClipboardItemImageUrl | GET /clipboard/items/:id/image?v= | 只拼 URL（fetch 在调用方） |
| exportRecord | GET /records/:id/export?format= | 只拼 URL，下载在 utils/exportDownload |
| getRecordAudio | GET /records/:id/audio | 只拼 URL（Audio 播放） |
| transcribeFile | POST /transcribe | 15min 同步超时；202 走任务 |
| getTranscribeJob | GET /transcribe/jobs/:jobId | |
| getSettings / saveSettings | GET/PUT /settings | |
| getHealth | GET /health?probe_runtime= | |
| getStreamingServiceStatus | GET /model-service | |
| startStreamingService / stopStreamingService | POST /model-service/start|stop | start 180s |
| getLLMState / getLLMProviders | GET /llm/state、/llm/providers | |
| createLLMProfile / updateLLMProfile / deleteLLMProfile | POST/PUT/DELETE /llm/profiles(/:id) | |
| verifyLLMProfile / getLLMRemoteModels | POST /llm/profiles/:id/verify、/remote-models | 180s |
| verifyLLMDraft | POST /llm/verify-draft | 草稿密钥不落盘 |
| createLLMModels / updateLLMModel / deleteLLMModel | POST/PUT/DELETE /llm/models(/:id) | |
| verifyLLMModel / activateLLMModel | POST /llm/models/:id/verify、/activate | |
| updateLLMSettings | PUT /llm/settings | default/fallback 绑定 |
| getPresets / setDefaultPreset | GET /prompts、PUT /prompts/default | 预设只读 |
| getWebSocketUrl | ws://127.0.0.1:port/api/ws | |
| getStreamingWebSocketUrl | ws://127.0.0.1:port/api/asr/stream | |

### 7.3 错误处理

- `extractErrorMessage`（43-83）：FastAPI 422 detail 数组 → `loc: msg` 用「；」拼接；detail 对象取 message；兜底 `HTTP {status}: {statusText}`——保证中文可读
- `request`（85-137）：AbortController 超时（30s 默认）→ 超时转「请求超时（超过 N 秒未响应）」；连接失败转「无法连接后端服务：…」；非 2xx 抛 Error 且挂 `error.status`/`error.detail`（131-133，409 分支用）
- `streamNdjson`（143-247）：支持外部 signal 取消（cancelled 标记，224-227）、超时区分（timedOut）、SyntaxError 转中文、断流转「流式连接中断」
- `uploadForm`（249-285）：300s 上传超时 + 同错误提取

---

## 8. useWebSocket.js 与 optimizationStream store

### 8.1 全局 WS（src/composables/useWebSocket.js）

- **用途**：后端事件总线。连接后收 `connected`/`status`（更新 appStore.isConnected/connectedClients，73-86）与业务事件（广播给 messageHandlers Set——useRecorder 的 handleWebSocketMessage 是唯一注册者，139-145）；发送 `status_update`（录音状态上报）与 30s `ping` 心跳（104-109）
- **重连逻辑**：`scheduleGlobalReconnect`——**固定 3000ms，无指数退避、无重试上限**（111-116）；由 `onclose`（43-45）与首次连接失败（68）触发；`shouldReconnect` 开关由 disconnect 关闭；`onerror` 只记 console 不参与重连（48-51）；`createSingletonAsyncInvoker`（utils/asyncSingleton.mjs）防并发建连；已 OPEN/CONNECTING 直接复用（22-24,119-121）
- 模块级单例状态 + `useGlobalWebSocket()` 返回 connect/disconnect/send/addMessageHandler/removeMessageHandler/sendRecordingStatus（147-157）

### 8.2 optimizationStream store

LLM 流式优化的单槽状态机（同一时刻只优化一条）。两条驱动路径：
- **自动优化**（录音后）：useRecorder 的 WS 事件 `optimization_started/delta/completed/failed/cancelled` 驱动（2030-2233）
- **手动优化**（记录卡「优化」钮）：useRecords.optimizeRecord 经 NDJSON HTTP 流驱动，含取消（AbortController+cancel 端点）、首增量前失败降级非流式、降级 Toast、truncated 警告 Toast（useRecords.js:65-191）
组件消费：RecorderSection 流式面板与 RecordCard 流式文本/光标/取消按钮。

---

## 9. 已知前端缺陷（代码/注释可见，带行号）

1. **recorder.onstop 无 catch → 多段最后一片上传失败卡死**：`createRecorder` 的 `recorder.onstop = async () => { await onStop(blob) }`（useRecorder.js:763-766）无 catch；`uploadRealtimeSegment` 只有 try/finally 无 catch（1199-1213）→ 异常沿 onstop 抛出后 `maybeFinalizeRealtimeSession`（1369 的 await）永不执行 → 界面永久「正在转写」、该段录音丢失。与 AGENTS.md §5.2 一致。
2. **全局 WS 固定 3s 重连无退避/无上限**（useWebSocket.js:111-116）；且 `onerror` 不触发任何收口（48-51），重连完全依赖 onclose；后端长时间不可用时每 3s 打一次。
3. **状态轮询固定 1s 无退避**（pollPolicy.mjs:7-8 注释自认「没有任何指数退避」；仅 30 次连续失败熔断，2315）。
4. **window.electronAPI 直读泄漏 2 处**（stores/settings.js:9-14、api/client.js:14-16），违反项目自身硬约束 #7（应只走 useElectron）——Tauri 迁移时的隐性雷点。
5. **`settingsStore.llmWizardMode` 是未声明的 ad-hoc 属性**：LLMServiceSection.vue:196 写入、LLMWizardModal.vue:169 读取，但 stores/settings.js 的 return 列表（568-608）没有它——运行时可用（Pinia store 是 reactive 对象），但绕过了状态声明面，devtools 不可见。
6. **悬浮窗独立 floatingAPI 未抽象**：FloatingWindow.vue 直读 window.floatingAPI（157/175/179/228/240），同样绕过包装层约束（属独立 preload 设计，但对迁移是额外改造点）。
7. **RecordsLibraryPage 对 recordsStore.records 的 deep watch + 防抖重拉**（218-220）：任何记录状态变化（如后台优化完成）都会触发整页重新搜索请求，页码之外的滚动位置/瞬时状态可能被打断（未确认用户可感知程度）。
8. **死代码候选**：`useClipboard.importCurrentClipboard`（useClipboard.js:48-71）无任何 UI 入口；`OVERLAY_CLIPBOARD_FILTERS`（clipboardPresentation.js:3-10）随统一浮层下线后已无引用（未确认是否有测试引用）。
9. **历史保留的已知怪癖**：`takeStreamingSamples` 队列不足时计数器可为负（useRecorder.js:406-414 注释明言逐字保留旧行为）。
10. **样式小重复**：`.record-edit-display:empty::before` 在 main.css 定义两次（1353-1356 与 1445-1447）。
11. **平台回退值不一致**：stores/settings.js:13 无 electronAPI 时 platform 回退 `'win32'`，而 useElectron.js:249 回退 `'web'`（SettingsPage 的默认热键逻辑用后者）——纯浏览器 dev 下两处默认热键显示不同（未确认实际影响，Electron 下均被真实值覆盖）。
12. RecorderSection 在 asr_provider/recording_mode 每次 变化都 `probe_runtime=true` 全量健康探测（471-477），后端探测成本未确认。

---

## 10. 迁移 Tauri 2.x 的前端改造边界

### 10.1 完全不动（直接平移）

- `src/styles/tokens.css`、`main.css`、`floating.css`（§3.6）；`electron-overrides.css` 仅 1 处属性替换
- `src/components/common/` 全部 12 个原语（零 Electron 依赖）
- `src/components/modals/` 6 个弹窗（RecordEditModal 用 clipboard 但走 useElectron 包装）
- `src/components/{home,records,clipboard,transcribe,settings}/` 的模板与逻辑（仅经 store/composable 间接依赖桌面层）
- `src/stores/app.js`、`optimizationStream.js`、`records.js`（tray 经 useElectron）
- `src/composables/recorder/*.mjs` 8 个纯逻辑模块、`transcribe/fileTranscribeModel.mjs`、`src/utils/*.mjs` 全部——**迁移最大红利区，且有 Node 测试锁定**
- `src/composables/useRecorder.js` 主体（useElectron 引用面形状不变则零改动）
- `src/worklets/pcm-capture-worklet.js`（标准 WebAudio，WebView2 支持）
- `src/composables/{useSettings,useRecords,useClipboard,useFileTranscribe,useSilenceDetector,useWebSocket}.js`（后端 HTTP/WS 协议不变）
- `tests/` 全部 16 个 .mjs（纯逻辑，不受壳影响）

### 10.2 必动（按优先级）

| 文件 | 改动 | 量级 |
| --- | --- | --- |
| `src/composables/useElectron.js` | 全部 window.electronAPI.* → Tauri invoke/listen；**保持导出形状**；命令清单见 §6.1；Rust 侧对应实现（端口、窗口、托盘、悬浮窗 WebviewWindow、剪贴板、粘贴模拟、对话框、getPathForFile、桌面设置 JSON、热键长按/松开事件、debugLog） | 大（唯一主战场） |
| `src/api/client.js` | 仅 getBackendPort（14-16 行）→ 从 Tauri invoke/环境取端口 | 小 |
| `src/stores/settings.js` | getPlatform（9-14 行）→ @tauri-apps/api os | 小 |
| `src/components/floating/FloatingWindow.vue` | window.floatingAPI.{close,openSettings,onStateChange,removeAllListeners} → Tauri window API + event listen/emit（主窗口 emit 事件替代主进程转发） | 中 |
| `src/components/layout/TitleBar.vue` + electron-overrides.css | `-webkit-app-region: drag` → `data-tauri-drag-region`（模板加属性 + css 属性清理） | 小 |
| `src/index.html` | CSP 追加 `ipc: http://ipc.localhost`（如保留严格 CSP） | 小 |
| `vite.config.js` | 双入口结构保留；对接 Tauri（devUrl 8090、build 兼容 tauri 协议）；base './' 已兼容 | 小 |
| `src/App.vue` | 基本不动（isElectron 语义改为「Tauri 环境」判定，用于 TitleBar 挂载） | 极小 |

### 10.3 需在 Rust 侧重建的 Electron 主进程职责（影响前端契约）

后端 FastAPI 进程管理与端口分配、系统级全局热键（长按/松开两事件 + Win+Alt 无主键组合）、系统剪贴板监控（clipboard:changed 推送）、粘贴到焦点窗口（键盘模拟）、录音时系统音频暂停（syncRecordingState）、托盘四态图标+最近记录菜单（updateTrayMenu 的数据结构 {id,created_at,raw_text,optimized_text,status,is_favorite}）、悬浮窗生命周期与状态转发（updateFloatingWindowState 的 payload {phase,duration,text,mode}）。前端契约以 useElectron.js 导出面 + floatingAPI 四方法为准（§6.1、§6.3）。

---

## 附：tests/*.mjs 前端纯逻辑测试清单（16 个）

test_clipboard_card_actions.mjs、test_async_singleton.mjs、test_recorder_pcm.mjs、test_recorder_poll_policy.mjs、test_recorder_segment_policy.mjs、test_recorder_tray_state.mjs、test_export_download.mjs、test_recorder_stream_protocol.mjs、test_recorder_stop_flush_order.mjs、test_recording_stop_state.mjs、test_settings_form_sync.mjs、test_recorder_status_copy.mjs、test_transcribe_file_model.mjs、test_recorder_floating_session.mjs、test_streaming_ui.mjs、test_llm_logic.mjs

覆盖面：recorder 纯逻辑 8 件套全部有测（pcm/poll/segment/tray/streamProtocol/stopFlush/stopState/statusCopy/floatingSession）、转写模型、LLM 逻辑（模型串组装/错误文案/参数摘要）、设置表单同步、剪贴板卡动作、导出下载、异步单例、流式 UI 判定（streamScroll 相关）。
