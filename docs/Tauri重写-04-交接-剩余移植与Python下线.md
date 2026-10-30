# 交接：剩余移植与 Python / Electron 下线（在 Windows 环境执行）

> 前置阅读：[Tauri重写-02-架构与模块设计.md](Tauri重写-02-架构与模块设计.md)（架构）、
> [Tauri重写-03a-实施记录.md](Tauri重写-03a-实施记录.md) §9（已对齐清单）。
> 本文是**可执行任务单**：逐项做完、逐项验收，最后一步才删除 Python 与 Electron。
> 写于 2026-10-30（macOS 开发机）；执行环境：有 Python/CUDA/本地模型的 Windows 机器。

## 0. 现状（2026-10-30 核对结果）

逐条对照了 Python 路由全集、Electron IPC 全集与 `UserSettings` 键集合：

- **HTTP**：除下面 §1–§3 列出的项外全部已在 `att-server` 实现，契约与 Python 一致。
  设计上有意移除的：`/api/ws`、`/api/asr/stream` 两个 WebSocket（D4：事件改走 Tauri event）、
  `/api/streaming/service*` 别名（前端只用 `/api/model-service*`）。
- **桌面能力**：Electron preload 的 31 个方法全部有 Tauri 命令或 Rust 侧等价实现。
- **设置项**：键集合一致（旧 `hotkey` 由迁移收敛为 `hotkeys[]`，属设计内变化）。

**Rust 版仍缺、只有 Python 版才有的能力**（删除 Python 前必须逐项决定「移植」或「放弃」）：

| # | 能力 | Python 位置 | 状态 |
| --- | --- | --- | --- |
| 1 | 本地 faster-whisper 引擎（录音单段/多段 + 文件转写） | `app/stt/providers/whisper_provider.py` | 未移植，选中时报错提示 |
| 2 | 本地 Qwen3-ASR 引擎（批量 + 流式；本地 vLLM 或远端桥接） | `app/stt/providers/qwen_*`、`scripts/qwen_streaming_service.py` | 未移植 |
| 3 | 本地模型服务启停 / 预热（`/api/model-service/start|stop` 的本地语义） | `app/services/model_service*.py` | Rust 只有在线引擎语义 |
| 4 | 浏览器网页模式 + Docker 部署（`/api/audio/upload`、`/api/sessions*`、WS） | `app/api/routers/records.py|sessions.py`、`docker/` | 501 |
| 5 | 数据维护 CLI：孤儿/残留清理、卡死状态收敛、数据目录迁移 | `scripts/housekeeping.py`、`scripts/migrate_data.py`、`app/repositories/housekeeping.py` | 未移植 |

## 1. 本地 faster-whisper 引擎（建议移植）

### 1.1 必须复刻的 Python 行为

- 配置：`whisper_model`（tiny/base/small/medium/large-v3，用户当前为 large-v3）、`whisper_device`（auto/cpu/cuda）、
  `whisper_compute_type`、模型目录 `models/faster_whisper`（`MODELS_DIR`），并兼容 HF 缓存目录
  `models--Systran--faster-whisper-<size>/snapshots/<hash>`（见 `_resolve_model_source`）。
- 识别参数（`CHINESE_CONFIG`）：`beam_size=5, best_of=5, temperature=0, condition_on_previous_text=False,
  word_timestamps=True, vad_filter=True, vad_parameters={min_silence_duration_ms:500, speech_pad_ms:200}`。
- 结果：分段 `[{start, end, text}]`（文件转写的 `want_segments` 与 LLM 校对窗口都依赖真实分段时间戳）；
  `info.duration / language / language_probability`。
- CUDA 加载失败回退 CPU + int8（`_load_model`）。
- 模型常驻、`warmup()` 预热、`unload_model()` 释放；健康字段 `model_service_*` 的 loading/ready/error 要真实反映。

### 1.2 方案选择（先做 1 小时调研再定）

| 方案 | 优点 | 代价 |
| --- | --- | --- |
| **ct2rs**（CTranslate2 的 Rust 绑定，支持 Whisper） | 直接复用现有 faster-whisper（CTranslate2 格式）模型，无需重新下载；行为最接近 Python | 需本机编译 CTranslate2（CMake + MSVC，CUDA 可选）；确认其是否提供 VAD 与词级时间戳 |
| **whisper-rs**（whisper.cpp） | 社区成熟、CPU 性能好、CUDA/Vulkan 可选 | 需要 ggml 模型（large-v3 约 3GB，与现有模型不通用）；VAD 需另接 Silero |

无论哪种：新增 crate `att-whisper`（或 `att-core/stt/whisper`），实现与 `stt::Engine` 同形的
`transcribe_pcm(language, samples) -> 分段结果`，并：

1. `stt::Engine` 加 `Whisper` 变体；`unsupported_provider_message("whisper")` 改为 `None`；
2. 录音 Actor 的单段/多段走 `Engine::transcribe_pcm`（已就绪，无需改 Actor）；流式模式提示不支持；
3. `transcribe::resolve_engine` 的 `whisper` / `auto` 分支接上（Python 语义：`auto` = 本地优先）；
   文件转写的分段时间戳原样进入 payload；
4. `att-server` 的 `engine_status` / `model_service_*` 返回本地语义（`mode="provider"`、loading/ready）；
   `/api/model-service/start` 触发预热、`/stop` 释放模型；
5. 前端 `useFileTranscribe.js` 桌面端缺省引擎改回 `whisper`（目前因未移植缺省为豆包）。

### 1.3 验收

- `cargo test --workspace` 新增：模型目录解析（含 HF snapshots 布局）、分段结构、CPU 回退。
- 实机：单段录音 / 多段录音 / 文件转写（mp4、mp3）各一次，结果与 Python 版同一文件对比（文本基本一致、分段时间戳合理）。
- 冷启动预热耗时、显存占用记入实施记录。

## 2. 本地 Qwen3-ASR（需要先决策）

Python 版依赖 `transformers`/vLLM 推理，**Rust 侧没有成熟等价方案**。可选：

- **A. 放弃**：设置页去掉 Qwen 选项（`src/components/settings/SettingsPage.vue` 的 provider 下拉与 Qwen 配置组、
  `src/composables/transcribe/fileTranscribeModel.mjs` 的引擎卡片），并删除相关设置键与迁移说明；
- **B. 保留远端桥接**：Rust 实现 `QwenRemoteBridge` 客户端（协议见 `qwen_streaming_runtime_provider.py` 的
  `QwenRemoteStreamingSession`：`POST /api/start` → `POST /api/chunk/{sid}`（application/octet-stream PCM16）
  → `POST /api/finish/{sid}`，`DELETE /api/session/{sid}`，`GET /api/health`），桥接服务作为**独立部署的外部服务**保留
  （`scripts/qwen_streaming_service.py` 移出本仓库或单独目录维护）。

## 3. 网页模式 / Docker（建议放弃）

Tauri 版录音完全在 Rust Actor 内完成，网页模式只服务旧架构。放弃时一并清理：

- 前端 `useRecorder.js` 中 `!isDesktop` 的旧录音路径（MediaRecorder 上传、多段 session、流式 WS、`useWebSocket` 的 WS 分支），
  以及 `src/api/client.js` 的 `uploadAudio / createSession / uploadSessionSegment / mergeSession / deleteSession /
  getWebSocketUrl / getStreamingWebSocketUrl`；相关 Node 用例同步删除或改写；
- `att-server` 的 `ny_upload` / `ny_sessions` 路由与 `未接端点返回501` 用例；
- `docker/`、`docker-compose.yml`、`scripts/docker/`。

## 4. 数据维护（建议移植精简版）

- 启动期只读体检（Python `run_startup_housekeeping`）：统计孤儿音频/图片、长期卡在 `transcribing`/`optimizing` 的记录，
  写日志即可；**卡在 optimizing 的记录在 Rust 版启动时应直接复位为 transcribed**（崩溃恢复，Python 版也存在此风险）。
- 显式清理（`housekeeping.py --apply`）可做成 `att-app` 的隐藏命令行参数或设置页「诊断」按钮；
  保留「默认只读、目录包含性校验、隔离不删除」三条安全约定。
- `migrate_data.py`（平铺/分片布局迁移）：Rust 版已有 `att-core/migration`，确认无需再保留后放弃。

## 5. Windows 实机验收清单（macOS 上无法验证的部分）

| 项 | 方法 |
| --- | --- |
| 修饰键不再卡住 | Win+L 锁屏再解锁后，单按 Alt / Alt+Tab 不应触发录音；默认 Win+Alt 按住录音正常 |
| 录音静音 | 开启「录音时暂停其他音频」，录音中切换耳机/扬声器后结束录音，静音状态正确恢复 |
| 托盘 | 左键唤起主窗口、右键菜单最近 10 条点击即粘贴、四态图标随录音切换 |
| 关窗 | 关闭主窗口即退出进程（任务管理器中无残留） |
| 自动粘贴 | SendInput 粘贴到记事本 / 浏览器输入框 |
| 文件转写 | 未把 ffmpeg 加入 PATH 时提示可读；`ATT_FFMPEG_DIR` 指定后可用；转写时不弹控制台窗口 |
| 打包 | `npm run tauri:build`：检查 NSIS/MSI 产物可安装、安装版能读到随包 `prompts/`、数据目录落在 `%APPDATA%` |

## 6. 下线（以上完成并验收后执行）

先 `git commit`，再按清单删除（全部是版本库内文件，可由 git 恢复）：

- Python 后端：`app/`、`tests/*.py`（含 `conftest.py`；Rust 的 Fernet 互操作样本在
  `src-tauri/crates/att-core/tests/fixtures/`，不受影响）、`pyproject.toml`、`uv.lock`、
  `.python-version`、`ruff.toml`、`scripts/*.py`、`att-*.bat`；
- Electron：`electron/`、`electron-builder.yml`、`scripts/package-electron.js`、
  Node 用例 `test_desktop_agent_integration.js`、`test_desktop_agent_manager.js`、`test_desktop_agent_python.js`、
  `test_desktop_settings_store.js`、`test_electron_dev_startup.js`、`test_tray_icon_assets.js`；
- `package.json`：删除 `main`、`backend:dev:uv`、`qwen:streaming:uv`、`codex:*`、`electron:*`、`test:python` 脚本，
  `test` 改为 `npm run test:node && npm run test:rust`；删除依赖 `electron`、`electron-builder`、`concurrently`、`wait-on`、
  `cross-env`（确认无其他引用后）；同步 `package-lock.json` / `pnpm-lock.yaml`；
- `.gitignore`：清理 `.venv/`、`release/`、Python 缓存等条目；
- `AGENTS.md` / `CLAUDE.md`：删掉「Python 后端仍在仓库」的说明与 pytest 命令。

验收：`node scripts/run-node-tests.mjs`、`cargo test --workspace`、`npm run build`、`npm run tauri:build` 全部通过；
`git grep -n "python\|electron"` 只剩历史文档与迁移代码中的合理引用。
