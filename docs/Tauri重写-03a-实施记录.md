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
