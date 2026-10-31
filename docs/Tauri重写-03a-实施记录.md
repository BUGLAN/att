# 3a 实施记录：工程骨架与数据面（阶段③第一步）

> 对应《[Tauri重写-02-架构与模块设计.md](Tauri重写-02-架构与模块设计.md)》§13 的 3a。
> 状态：**已交付，等待实机验收**（`npm run tauri:dev`）。

## 1. 本步落地的内容

### Rust 侧（src-tauri/，工作区 4 包）

| 模块 | 内容 | 测试 |
| --- | --- | --- |
| `att-core/storage` | JSON 原子写（tmp+fsync+rename）/宽容读/按资源路径锁；**纯 Rust Fernet 规范实现**（不用 fernet crate，避免 OpenSSL 依赖；与 Python cryptography 互操作由真实密文 fixture 锁定） | 4 |
| `att-core/settings` | UserSettings 全 30+ 键（与 Python 默认值逐项对齐）+ PUT 白名单契约（null=未提供/未知键丢弃/类型错误中文/密钥只写不读/`*_clear`）+ 脱敏公共视图；热键模型：HID 物理键码、`{action, trigger(Toggle/Hold), modifiers, hid_key}` 多绑定、旧 `{modifiers,key,display}` 双向转换 | 8 |
| `att-core/records` | Record 13 字段模型（旧数据缺 error/revision 可读）、仓库（裸数组列表/搜索分页/乐观锁 revision→409/删除含音频/导出视图/`set_audio_path`）、id 安全防护 | 5 |
| `att-core/migration` | 一次性迁移：退役键剥离、旧 hotkey + 旧 Electron `userData/desktop-settings.json`（探测 `%APPDATA%/audio-to-text`）合并为 `hotkeys[]`、**音频错位修复三遍法**（改名→时长唯一匹配±0.1s→孤儿隔离不删除）、报告落盘 `migration-report.json` | 3 |
| `att-core/events` + `state` | tokio broadcast 事件总线（旧事件名映射）；CoreState 组装 | 2 |
| `att-hotkey` | 平台后端 trait（register/Conflict/poll_physical_state/resync_all——防粘滞契约）+ Noop 实现（3c 接真实现） | 1 |
| `att-server` | axum 数据面（仅回环随机端口）：health（**双命名 21 字段全量**）、settings GET/PUT、records 全套（含音频播放/导出 txt+json）、prompts 只读+设默认、LLM 空态、模型服务骨架、未接端点 501 带阶段标注 | 5 |
| `att-app` | Tauri 壳：数据目录解析（env > dev 仓库 data/ > 应用数据目录）、迁移、事件桥、单实例（二次启动唤起）、**19 个 invoke 命令**（对旧 electronAPI 1:1） | — |

### 前端适配（6 个文件，组件层零改动）

- `src/composables/useElectron.js`：换 Tauri 内核（invoke/listen），导出形状不变；`isElectron` 语义=桌面环境（+`isDesktop` 别名）；事件通道名沿用旧名
- `src/api/client.js`：getBackendPort → `invoke('get_backend_port')`（浏览器模式仍回退 8000）
- `src/stores/settings.js`：getPlatform 改 UA 同步推断（精确值走 getPlatform()）
- `src/components/layout/TitleBar.vue`：加 `data-tauri-drag-region`
- `src/index.html`：CSP 追加 `ipc: http://ipc.localhost`
- `package.json`：+`@tauri-apps/api`/`cli`，+`tauri:dev`/`tauri:build`/`test:rust` 脚本

## 2. 验证状态（本机自动化）

| 验证 | 结果 |
| --- | --- |
| `cargo test --workspace`（src-tauri） | ✅ 33/33（core 27 + hotkey 1 + server 5） |
| `cargo build`（整包含 Tauri） | ✅ 0 警告 0 错误 |
| `npm run build`（vite 双入口） | ✅ |
| `npm run test:node`（前端纯逻辑 22 项） | ✅ 22/22（useElectron/client/settings 改动零破坏） |

**关键兼容性测试**：Rust 解密真实 Python 密文 fixture（`tests/fixtures/python_fernet.json`，由仓库 .venv 的 cryptography 生成）✅；settings.json 旧格式加载/退役键/旧热键迁移 ✅；记录旧数据缺字段可读 ✅。

## 3. 实机验收第一轮发现的问题与修复（2026-10-29）

| 现象 | 根因 | 修复 |
| --- | --- | --- |
| 侧边栏红花「未连接后端」+ 历史记录 0 条 | WebView 内 `invoke('get_backend_port')` 未成功 → 前端回退 8000（旧 Python 端口），数据面全部连错 | 主窗口改为 **Rust 侧手工创建**，用 `initialization_script` 在页面脚本执行前注入 `window.__ATT_BACKEND_PORT__`；`client.js` 端口解析链改为「注入值 → invoke → 8000」并打印来源 |
| 「未连接后端」红点常亮 | 该指示的判定源是旧全局 WS（架构决策 D4 已移除 WS 端点），前端仍在连 WS | Tauri 模式下 `useWebSocket` 改为 **5 秒健康检查轮询**驱动连接状态；WS 路径仅在非 Tauri 环境保留 |
| 启动弹「http://127.0.0.1:8090 想要使用麦克风」 | 五个页面 `v-show` 常驻挂载，设置页一挂载即 `getUserMedia` 枚举设备 | Tauri 模式下跳过设备枚举（3b 改为 Rust 命令枚举）；麦克风只在真正点录音时才会申请 |
| 启动慢 | 迁移每次启动都全量扫 900+ 记录与 768 个 wav | 迁移加 `data/.migration-3a.done` 完成标记，二次启动直接跳过；`ATT_FORCE_MIGRATION=1` 可强制重跑 |

**复测方法**：直接重跑 `npm run tauri:dev`（Rust 会增量编译约 10~20s）。预期：红点消失、历史记录出数、启动不再弹麦克风、二次启动明显变快。

### 第二轮（2026-10-29 复测后）：四类请求全失败

现象：`加载记录失败 / 加载剪贴板记录失败 / LLM 配置加载失败 / 保存设置失败` 同时出现——说明不是单端点问题，而是**前端整体连不上数据面**。本轮从「不再依赖单点假设」角度做了三层加固：

| 加固 | 内容 |
| --- | --- |
| 端口来源 | Rust 端改为**优先固定端口 17890**（占用则回退随机），并落盘 `data/.backend-port`；前端端口解析改为四级：注入值 → invoke → **健康探测 17890** → 旧 8000，且网络错误后自动重解析（保留注入值优先级） |
| 运行时形态 | 绑定与 accept 循环拆分：`bind_local()` 同步占端口，`serve_on()` 在 Tauri 运行时显式 spawn——消除「block_on 内 spawn 的任务在运行时退出后不再被驱动」导致的「TCP 可连但无响应」隐患 |
| 跨域 | CORS 从固定 origin 清单改为**回环来源谓词放行**（localhost/127.0.0.1 任意端口 + tauri:// + null），避免协议/端口推断偏差把整站请求拦掉 |
| 可观测 | 启动时裸 TCP 自检 `/api/health` 并打印结论（`数据面自检通过: HTTP/1.1 200 OK`）；前端请求失败会打印 `[API] 请求失败 GET http://127.0.0.1:PORT/api/...`；剪贴板只读端点改为空数据 200（消除误报） |

**回归测试**：新增 `crates/att-server/tests/serve_smoke.rs`——真实 listener + 裸 TCP 自检 + CORS 放行/拒绝断言，共 35 个 Rust 测试全绿。

**仍异常时的自查三步**：① 终端找 `数据面 API 就绪` 与 `数据面自检` 两行；② 浏览器直接打开终端里那个 `http://127.0.0.1:PORT/api/health`（能出 JSON 说明服务正常，问题在浏览器侧）；③ DevTools Console 第一行 `[API] 后端端口: N（来源）`。

### 第三轮（2026-10-29 上午）：三个连锁根因修复 + 数据恢复

现象：`npm run tauri:dev` 报端口占用起不来 / 起来后界面全部报错。逐层排查发现三个真实缺陷（前两个是上一轮加固掩盖的连锁问题）：

| # | 根因 | 后果 | 修复 |
| --- | --- | --- | --- |
| 1 | 残留进程：上次退出未清理的 att-app（持单实例锁）与 vite（占死 8090，`strictPort` 下新 vite 直接失败） | 「启动不起来」的直接表象 | 排障时先清进程（注意：本机 WMI/taskkill /T 会 hang，用 `Stop-Process` 直杀 PID） |
| 2 | `lib.rs` 的 `_shutdown_tx` 是 setup 局部变量，setup 返回即 drop → watch 通道关闭被 `with_graceful_shutdown` 当作停机信号 | **数据面在自检通过后几秒内静默死亡**（优雅关闭路径无日志），界面全部请求失败 | 发送端挂进 `AppState` 随 `app.manage` 存活；新增回归测试 `发送端存活期间_数据面持续在线` |
| 3 | `settings.json` 里 Python 侧写出的 `silence_level: -40.0`（浮点）让 i64 反序列化**整体**失败 | 全部用户设置回落默认值；一旦保存设置即覆盖丢失真实配置（含密钥密文） | 5 个整数字段加 `deserialize_i64_lenient`（浮点取整）；PUT 路径同策略 |
| 4 | 记录的 `audio_path` 历史形态（`data\audio\records\x.wav` 带前缀、docker 绝对路径）被直接 `audio_dir.join()` 拼错路径 | 所有旧音频 404、删除清理失效；**首轮迁移因此把 886 个 wav 误隔离进 orphans/、801 条记录标 missing**（时长匹配也因候选大量撞车而 0 命中） | `records::normalize_audio_rel()` 归一化 + `resolve_audio_path()`，`audio_file`/`delete`/迁移 pass-1/2/3 全部切换；referenced 集合改用归一化名 |

**数据恢复实操**（事故文件未删除，设计上「隔离不删除」救了这次）：把 `data/audio/orphans/` 的 886 个 wav 移回 `records/`、删 `data/.migration-3a.done`、重启触发重迁移。结果：**精确认领 800 + 时长匹配 6 + 真孤儿 80 + 缺失 1**（缺失那条是 `test/path.wav` 测试垃圾）。修复前 404 的记录音频回传 200。

**验证**：`cargo test --workspace` 40 项全绿（新增 5 个回归测试）；实机 health 启动 2 分钟仍 200；settings 返回真实配置（chatgpt_asr / 双密钥 set / Win+Alt）；PUT 浮点取整写入；records 搜索 155 命中；音频/导出/剪贴板空态/LLM 空态/模型服务骨架/501 标注全部符合契约；`npm run build` ✅；Node 测试 22/22 ✅。

## 4. 如何验收（需要你实机执行）

```bash
# 0. 强烈建议先备份 data/（首启动会执行真实迁移：改名错位音频 + 孤儿移入 audio/orphans/）
# 1. 启动（自动拉起 vite + Tauri 壳 + 内嵌数据面）
npm run tauri:dev
```

验收清单（3a 标准：应用启动、五页可浏览、旧数据迁移正确）：
1. 窗口出现，自绘标题栏可拖动/最小化/最大化/关闭，侧边栏五页切换正常
2. 首页「最近记录」显示旧记录（数据面经 invoke 发现端口直连 axum）
3. 记录集页搜索/分页/收藏/编辑弹窗/删除正常；点卡片可试听音频（完整回传，暂不支持拖动进度条）
4. 设置页加载旧设置值（含旧快捷键显示）；修改设置 500ms 自动保存生效
5. 检查 `data/migration-report.json`：重命名/时长匹配/孤儿/缺失清单是否符合预期
6. 已知 3a 限制（预期行为，非缺陷）：录音按钮仍走旧渲染层路径（Tauri WebView 无麦克风授权会提示失败）→ **3b 由 Rust 接管采集**；托盘/悬浮窗/粘贴/模型服务为占位；LLM 页为空态

## 5. 与设计的偏差记录

- `fernet` crate → 自实现（OpenSSL 依赖不可接受），行为由规范+fixture 锁定
- `cargo test` 需 `--workspace`（工作区根同时是 app 包）
- Tauri 命令模块需 `pub mod` + `pub fn`（`__cmd__` 宏只对 pub 命令导出）

## 6. 下一步（3b：音频管道 + 单段录音）

cpal 采集（SPSC 环 + 重采样 16k）→ WAV 落盘 → whisper.cpp Provider → Record 落库 → 录音状态/音量事件；替换 useRecorder 单段路径为 useRecordingBridge。

## 7. 豆包主链路打通（2026-10-29，提前落地 3b/3c/3d 的必要子集）

3a 交付后应用「能浏览不能用」：录音按钮、快捷键、识别、粘贴全部是占位。本轮按设计 D1/D3
（采集与编排在 Rust）把**豆包语音这条主链路**端到端打通，其余引擎/功能仍按阶段计划推进。

| 模块 | 内容 | 测试 |
| --- | --- | --- |
| `att-core/stt/sauc` | 火山 SAUC 二进制协议编解码（逐项移植 Python `volcengine_sauc.py`：帧头/错误帧两种布局/gzip/definite 前缀推进/错误归一化） | 6 |
| `att-core/stt/doubao` | tokio-tungstenite 会话：三头鉴权握手、后台读任务、200ms 切帧推流、末包+静默窗口收尾、批量转写；TLS 用 native-tls（不引入 OpenSSL/aws-lc 构建依赖） | 3 + 实网探针 |
| `att-core/audio` | 16k PCM16 WAV 读写（tmp+rename）、跨块保相位的线性重采样器 | 2 |
| `att-audio`（新 crate） | cpal 采集：回调内 mono 混音→rtrb SPSC 环（零分配零锁）→采集线程 20ms 读环→16k i16 分块+电平；停止时先停设备再冲刷尾部 | — |
| `att-hotkey` | 底层键盘监听：macOS CGEventTap（listen-only，超时停用自动重启）/ Windows WH_KEYBOARD_LL；匹配状态机移植旧 pynput 语义（**支持纯修饰键组合**，如 Win+Alt / ⌘⌥）；锁存期间 250ms 物理键态重同步防粘滞 | 4 |
| `src/recording.rs` | 录音 Actor：Idle→Connecting→Recording→Finalizing；流式=采集与建连并行（建连期间样本缓存、就绪后冲刷），单段/多段=录完批量识别；失败也落盘建 failed 记录（录音不丢）；中途断连按已识别文本降级保存 | — |
| `src/desktop.rs` / `floating.rs` | 自动复制+模拟粘贴（macOS CGEvent / Windows SendInput）、录音静音（macOS osascript）、底部居中悬浮窗（Actor 直接驱动） | — |
| 前端 | `useRecorder` 桌面端改为发指令+订阅 `recording:*` 事件（映射规则 `recorder/nativeSession.mjs`，Node 用例 6 个）；设置页麦克风列表改走 Rust 枚举；`floating.js` 垫片 `window.floatingAPI` | 6 |

**实网验证**（本机 `data/settings.json` 的密文 Token）：
`cargo run -p att-core --example doubao_probe -- ../data <wav> [stream]`——批量与实时节奏推流均返回正确文本，
流式中间结果逐字到达，末包后约 0.8s 出最终结果。

**权限（macOS）**：全局快捷键需「输入监控」，自动粘贴需「辅助功能」，录音需「麦克风」；
开发态这些权限归属**启动 `tauri dev` 的终端应用**。快捷键不可用的原因可经 `get_hotkey_status` 命令查询，
日志写入 `data/logs/att-app.log`（每次启动覆盖）。

**仍未接入**（按原阶段计划）：LLM 优化（3f，`auto_optimize` 暂不生效）、文件转写、剪贴板历史、托盘、
多段模式的 VAD 实时分段（当前按单段处理）、Windows 端系统静音、Whisper/Qwen/ChatGPT 引擎
（选中时开始录音会提示切换到豆包）。

## 8. LLM 配置模块（2026-10-30，阶段 3f 的配置管理部分）

现象：设置页「LLM」无法添加供应商，服务商目录与旧配置都是空的。根因：Rust 数据面只有
`GET /api/llm/state|providers` 两个返回空数据的占位端点，增删改/验证/拉取模型端点不存在，
旧版 `data/llm_configs.json` 也没有迁移。

| 模块 | 内容 | 测试 |
| --- | --- | --- |
| `att-core/llm/registry` | 14 家服务商目录（与 Python `registry.py` 逐项对齐）、模型串组装（custom 恒为 `openai/<ID>` 且幂等）、线上模型名解析 | 3 |
| `att-core/llm/store` | `data/llm_v2.json`（与 Python 同格式）：逐条容错加载、custom 模型串自愈、默认/备用绑定修复、v1 自动迁移（同 key 同端点聚合档案，v1 改名 `.v1.bak`）、落盘失败回滚内存 | 4 |
| `att-core/llm/gateway` | reqwest（native-tls + 系统代理）直发：OpenAI 兼容 `/chat/completions`（Ollama 走 `/v1`）、Anthropic `/v1/messages`；模型/档案验证、`GET /models` 列取；`max_tokens` 被拒时改 `max_completion_tokens` 重试 | 5（本地 HTTP 桩） |
| `att-core/llm/errors` | 错误分级（状态码 + 错误体）、密钥脱敏（字面量 + `api key:`/Bearer/`sk-` 模式） | 2 |
| `att-server/llm` | `/api/llm/*` 全部 17 个端点，契约同 Python 路由（验证类 200+`{ok,...}`、409 `manual_only`、默认模型删除 409、fallback 显式 null 清除） | 1（全链路） |

**实机验证**：运行中的 `tauri dev` 自动重编译后完成真实迁移（DeepSeek 档案 + 默认模型，密钥解密成功）；
`POST /api/llm/models/mdl-m0/verify` 真实调用 DeepSeek 通过（≈1.5s），`remote-models` 拉到模型列表；
假密钥的草稿验证返回 `auth_failed`，上游回显的密钥已被打码。

与 Python 版的差异：v1 迁移时模型显示名改为取各自条目的名字（Python 版会沿用前一条的名字）；
custom 档案改 API Base 为空会被拒绝（Python 版放行后会导致调用失败）。

记录优化与 `auto_optimize` 已在第 9 节补齐。

## 9. 与 Python/Electron 版的功能对齐（2026-10-30）

逐项对照 `Tauri重写-01-功能梳理清单.md` §1 / §7.1 与 Python 路由全集后补齐的部分：

| 功能 | Rust 实现 | 验证 |
| --- | --- | --- |
| **LLM 记录优化** | `att-core/llm/{prompt,cleaning,timeouts,optimizer}`：预设模板 → system/user + `<transcript>` 信封（中和伪造标签、16000 字截断）；SSE 流式（首字 / 块间空闲看门狗）；首字前失败回退非流式、思考吃满预算（`output_truncated`）不回退；三层输出清洗；同记录互斥与取消；失败/取消复位 `transcribed` 并写 `error`。思考强度：DeepSeek「关闭」= `thinking:{type:disabled}`，其余传 `reasoning_effort`，上游不认时自动去掉重试 | 单测 4 + 纯函数 8；真实 DeepSeek + 「程序员模式」预设：26 增量、首字 1.5s、清洗后落盘 `optimized` |
| 优化端点 | `/api/records/{id}/optimize`（502 + 脱敏）、`/optimize/stream`（NDJSON start/delta/done/error；取消以 error 收尾，避免前端误回退重跑）、`/optimize/cancel` | 实机 404/取消回执 |
| **录音后自动优化** | 录音 Actor 新增 Optimizing 相位：转写落库 → 流式优化（`recording:optimization` 增量 → 前端优化面板 + 悬浮窗「正在优化」）→ 粘贴**优化后**文本；失败兜底粘贴原文、取消不粘贴；单段模式优化期间吞掉新录音，流式模式允许直接开始下一段（上一段结果转后台只入库，`recording:background-optimization`） | Node 用例 2；Rust 编译 |
| **托盘** | `src/tray.rs`：四态图标（沿用旧图）+ tooltip；菜单 = 最近 10 条记录（点击复制并粘贴到前台应用）+ 设置 + 退出；macOS 左键弹菜单、Windows 左键唤起主窗口；录音/优化落库后自动重建菜单 | 单测 1 |
| **剪贴板历史** | `att-core/clipboard`（同 JSON 布局，旧 23 条直接可读）+ `/api/clipboard/*` 全套（含图片预览，路径必须落在 images 目录内）；`src/clipboard.rs` 800ms 监控：文本规范化去重、图片按 PNG SHA-256 落盘、本应用写入 5s 内标记跳过 | 单测 4；实机 23 条 + 图片 200 |
| **文件转写** | `att-core/transcribe`：ffmpeg 解码为 16k PCM 管道 → 豆包（超 30 分钟按会话分段）→ 缓存（同 `transcribe_file_cache.json` 格式，LRU + 校对变体）→ 202 任务表；LLM 滑窗校对（双上限切窗、键集合 + difflib 同算法相似度验证、反馈重试 3 次、失败窗保留原文、整体不可用降级）；系统选择文件对话框（dialog 插件）+ 原生拖放取路径 | 单测 6；真实 mp3：豆包 10.5s、二次命中缓存、校对任务修正断句 |
| **多段模式** | `audio::Segmenter`（2.2s 最短 / 10s 强切 / 0.8s 收尾门槛 + 静音阈值设置）→ 每段并发批量识别 → `recording:partial` 实时拼接（`\n\n` 合并，同旧 session merge）；收尾总上限 120s，单段失败只降级（修复旧版「最后一片失败永久卡住」） | 单测 1 |
| **ChatGPT 听写** | `stt/chatgpt.rs`：multipart + 鉴权四件套（JWT 提取 account id）+ 状态归类；单段/多段可用，流式模式明确提示不支持；健康检查同 Python 语义 | 单测 2（本机未配置令牌，未做实网） |
| **Windows 录音静音** | Core Audio `IAudioEndpointVolume`，每次取当前默认输出设备（修复旧版 pycaw 句柄固化） | 独立 crate 交叉编译通过，未实机 |
| 打包版提示词 | `bundle.resources` 带上 `prompts/`，运行时优先仓库目录、其次安装资源目录 | — |

### 仍未对齐（及原因）

| 项 | 原因 / 现状 |
| --- | --- |
| ~~本地 faster-whisper / Qwen3-ASR（录音与文件转写）~~ | **已对齐（2026-10-31）**：`crates/att-whisper`（ct2rs/CTranslate2）与 `crates/att-qwen`（sherpa-onnx ONNX）都是进程内推理，见 §10 |
| Qwen 流式（`recording_mode=streaming`） | ONNX 导出版没有流式接口；流式仍只支持豆包 |
| `/api/audio/upload`、`/api/sessions*` | 只服务于「浏览器里跑前端 + Python 后端」的旧录音路径；Tauri 版录音完全在 Rust Actor 内完成，这两组端点保持 501 |
| 本地模型服务启停（`/api/model-service/start\|stop`） | 已对齐：`start` 加载并预热、`stop` 释放（在线引擎仍无本地资源） |

## 10. 本地引擎接入：faster-whisper 与 Qwen3-ASR（2026-10-31，Windows 实机）

### 10.1 结论

| 引擎 | 实现 | 模型 | 状态 |
| --- | --- | --- | --- |
| faster-whisper | 新 crate `att-whisper`：CTranslate2 的 Rust 绑定 **ct2rs**，**直接复用旧版 `models/faster_whisper`（CT2 large-v3，2.9GB）** | 无需重下 | 单段/多段/文件转写可用，含真实分段时间戳 |
| Qwen3-ASR | 新 crate `att-qwen`：sherpa-onnx C API（运行期 `dlopen`）+ C 桥 `src/shim.c` + 官方 ONNX 导出 | `qwen3-asr-0.6B-int8`（940MB） | 批量可用（录音/文件转写），**流式仍不支持** |

两个引擎都是**进程内推理**，不依赖 Python、不依赖外部服务。模型与推理库不随仓库分发（下载清单见 04 号文档 §7）。

### 10.2 方案取舍

| 候选 | 结论 |
| --- | --- |
| **ct2rs**（CTranslate2 Rust 绑定） | ✅ Whisper 采用。复用现有 `model.bin`；CMake 静态编译约 3 分钟；提供 `generate_segments`（分段 + 词级时间戳）与 `Config{device, compute_type}` |
| whisper-rs（whisper.cpp） | ❌ 要另下 ggml 权重（large-v3 约 3GB），与现有 CT2 模型不通用 |
| Python 桥接 / 远端服务 | ❌ 与「下线 Python」冲突；流式桥接依赖 vLLM，Windows 无官方轮子 |
| transformers / vLLM 跑 Qwen（Python 路线） | ❌ Rust 侧无对应推理栈 |
| **sherpa-onnx + ONNX 导出版 Qwen3-ASR** | ✅ Qwen 采用。C API 有 `SherpaOnnxOfflineQwen3ASRModelConfig`，Windows 有预编译动态库（v2.0.1） |

### 10.3 落地的代码

| 位置 | 内容 |
| --- | --- |
| `crates/att-whisper` | 模型目录解析（含 `models/faster_whisper` 与 HF `snapshots/` 布局）、设备/精度推导（`auto` = CUDA 可编译则 GPU float16，否则 CPU int8）、CUDA 加载失败回退 CPU、能量法首尾静音裁剪、`generate_segments` 出分段时间戳、内部串行闸门 |
| `crates/att-qwen` | `build.rs` 只编译自带 C 桥；`src/shim.c` 用 `LoadLibrary/dlopen` 运行期加载 sherpa-onnx（含 ORT 依赖预加载），暴露「Qwen 识别」与「Silero VAD」两组函数；`src/lib.rs` 负责模型/推理库探测、静音切点分块（复刻 Python `_silence_cut_points`/`_plan_chunks`）、结果整形；`src/vad.rs` 是 VAD 封装 |
| `att-core/stt/local.rs` | 常驻池（懒加载、设置指纹失效、可显式卸载）、就绪性探测（只看文件不动模型）、`LocalEngine` 统一两个引擎的转写入口；**Whisper 前置 Silero VAD**（见 10.5） |
| `att-core/stt/mod.rs` | `Engine::Local` 变体；`online_from_settings`（同步，只解析在线引擎）与 `resolve`（异步，本地引擎在 blocking 线程池加载）；`transcribe_local` / `warmup_local` |
| `att-core/transcribe/mod.rs` | `resolve_engine` 恢复「显式 > auto（本地优先）」语义；本地引擎走 `transcribe_local`，分段时间戳原样进 payload；豆包路径抽成 `run_doubao` |
| `att-server` | `engine_status` 本地语义（`mode="provider"`，流式明确不支持）；`/api/model-service/start` 真正加载+预热、`/stop` 释放；payload 增加 `model_dir`/`device` |
| 前端 | 设置页去掉「本地 · 待接入」；文件转写缺省引擎改回 `whisper`（桌面端与网页端一致） |
| `src-tauri/.cargo/config.toml` | Windows MSVC 统一 `+crt-static`（见 10.5） |

### 10.4 实测（RTX 5090 / Windows 11 / large-v3 + Qwen3-ASR-0.6B-int8）

测试音频：`models/qwen3-asr-0.6B-int8/test_wavs/{noise2,raokouling}.wav`（sherpa-onnx 官方样例）。

| 场景 | 实现 | 耗时 | 实时率 |
| --- | --- | --- | --- |
| Whisper large-v3，22.8s，CPU（ct2rs 默认 `ruy` 后端、int8） | 首版 | 220.96s | 9.7× ❌ 不可用 |
| Whisper large-v3，22.8s，CPU（`dnnl` oneDNN + OpenMP、int8） | 现版 | 33.27s | 1.46× |
| Whisper large-v3，20.8s（VAD 裁到 17.7s 语音），CPU + Silero VAD | 现版 | 40.9s（含约 10s 模型加载 + VAD） | 2.0× |
| Qwen3-ASR-0.6B-int8，22.8s，CPU 2 线程 | 现版 | 4.7~11.1s（随机器负载） | 0.21~0.49× ✅ |
| 对照：Python faster-whisper large-v3，CUDA float16，20.8s | 旧版 | 3.47s | 0.17× |

文本对照（同一段 20.8s 中文绕口令，Whisper）：

| 实现 | 输出 |
| --- | --- |
| Python（CUDA float16，`vad_filter=True`） | 广西壮族自治区爱吃红鲤鱼绿鲤鱼鱼的出租车司机拉着苗族土家族自治州爱喝自治的流奶奶榴莲牛奶的骨质疏通症患者 遇见别着喇叭的哑巴 打败姚子山前44颗死色狮子树的44只狮子之后… |
| Rust（CPU int8，Silero VAD 分段） | 广西壮族自治区爱吃红鲤鱼 绿鲤鱼 绿鲤鱼的出租车司机拉着苗族土家族自治州爱喝自治的牛奶奶 榴莲牛奶的骨质疏通症患者遇见别着喇叭的哑巴 打败姚子山前44颗死色狮子树的44只狮子之后… |
| Rust（去掉 VAD，整段送模型） | **请不吝点赞 订阅 转发 打赏支持明镜与点点栏目** ❌（Whisper 经典幻觉） |

Qwen 文本对照（22.8s 电话语音）：

| 实现 | 输出 |
| --- | --- |
| Python `qwen_asr` 1.7B（CUDA） | 拨号。请再说一次，请说出你要拨打的号码。幺三五八幺八八七五五七。一三五八二八八八幺八八。纠正纠正。九六九。纠正纠正，不是九六。 |
| Rust（0.6B int8，CPU） | 拨号，请再说一次，请说出你要拨打的号码：幺三五八幺八八七五五七。一三五八二八八八幺八八。八七五五。有建筑摄像，有建筑一百公里。纠正，纠正，九六九。纠正，纠正，不是九六。 |

（0.6B 小模型在噪声段多补了半句幻觉，其余一致；要更强中文效果可换 Modelscope 上的 1.7B ONNX 导出版。）

验证命令（断言：文本非空、分段单调、分块不超上限、缓存落盘）：

```bash
ATT_TEST_WAV=<16k.wav> cargo test -p att-whisper --test real_model -- --ignored --nocapture
ATT_TEST_WAV=<16k.wav> cargo test -p att-qwen    --test real_model -- --ignored --nocapture
ATT_TEST_WAV=<任意音频> cargo test -p att-core   --test local_engine_wiring -- --ignored --nocapture
```

### 10.5 踩坑与修复（都是 Windows 实机才暴露的）

| 坑 | 现象 | 修复 |
| --- | --- | --- |
| MSVC 代码页 | `shim.c` 里的中文错误文案按 GBK 解析 → `error C2001: 常量中有换行符` | `build.rs` 给 cl 加 `/utf-8` |
| CRT 不匹配 | `LNK2038: RuntimeLibrary 值“MT_StaticRelease”不匹配“MD_DynamicRelease”`（ct2rs 固定按 /MT 编 CTranslate2） | `src-tauri/.cargo/config.toml` 对 Windows MSVC 加 `+crt-static`（ct2rs README 的建议做法） |
| 系统 ORT 抢占 | `C:\Windows\System32\onnxruntime.dll`（1.17，别的软件装的）先被解析 → `requested API version [28] is not available` + `STATUS_ACCESS_VIOLATION` | `shim.c` 加载 sherpa 前，先把推理库同目录的 `onnxruntime.dll` 按绝对路径预加载 |
| CPU 后端 | 默认 `ruy`（纯 float32、无并行）→ RTF 9.7 | 打开 ct2rs 的 `dnnl` + `openmp-runtime-comp` → RTF 1.46 |
| 缺 VAD 的幻觉 | 音乐/噪声素材上整段输出固定字幕模板文本；**用 Python `vad_filter=False` 复现同样输出**，证明不是接线问题 | 复用 sherpa-onnx 自带的 Silero VAD（`att-qwen::vad`），Whisper 按语音区间分段推理 |
| 模型目录布局 | Python 版的 `download_root` 直接就是 `models/faster_whisper` | 探测候选里补上 `faster_whisper` / `faster-whisper` |

### 10.6 已知限制（后续项）

1. **CUDA 未验证**：`att-whisper` 有 `cuda` 特性（`ct2rs/cuda` + `cudnn` + `cuda-dynamic-loading`），但本机没装 cuDNN SDK（Python 的 CT2 wheel 自带运行库、没有头文件），所以 GPU 路径没有实编译验证；CPU 下 large-v3 是 1.5 倍音频时长，实时录音可用、长文件转写偏慢。
2. **打包**：sherpa-onnx 的 DLL（`sherpa-onnx-c-api.dll` / `onnxruntime.dll`）目前只在开发态从 `models/sherpa-onnx/` 加载，`bundle` 配置未加，安装版里 Qwen 会因缺库报错（Whisper 不受影响）。
3. **Qwen 流式**：ONNX 模型没有流式接口，`recording_mode=streaming` 下仍提示不支持（豆包才是流式引擎）。
4. **Qwen 精度/模型**：默认 0.6B int8（Python 版 1.7B）；`qwen_asr_device`/`dtype`/`use_forced_aligner` 等设置项在当前实现里不生效（ONNX CPU + 无强制对齐），设置页字段保留但会被忽略。
5. **语言概率**：CTranslate2 不返回 `language_probability`，payload 里不体现；Qwen 语言由模型自判（sherpa 配置没有语言参数）。
6. **首次加载延迟**：本地模型懒加载，首次录音/转写要等 5~15 秒（large-v3 加载），之后常驻。
