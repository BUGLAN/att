# AGENTS.md

给 AI 编码代理（Codex / Claude Code 等）的工作指引。项目：Audio to Text（ATT），**Tauri 2 + Rust + Vue 3** 桌面语音转文字应用。

详细设计与现状以 `docs/` 下的 Tauri 重写系列为准：

| 文档 | 内容 |
| --- | --- |
| `docs/Tauri重写-01-功能梳理清单.md` / `01a-前端审计附录.md` | 功能对齐基线（用户可见功能全清单） |
| `docs/Tauri重写-02-架构与模块设计.md` | 架构、模块划分、关键决策 |
| `docs/Tauri重写-03a-实施记录.md` | 已落地内容、踩坑与修复、§9 与旧版的功能对齐表 |
| `docs/Tauri重写-04-交接-剩余移植与Python下线.md` | **剩余移植任务**（本地 Whisper/Qwen 等）与 Python/Electron 下线清单 |

> 仓库里的 `app/`（Python 后端）、`electron/` 是旧架构，Tauri 版运行时不再使用；按 04 号文档完成剩余移植后删除。
> 改动请落在 `src-tauri/` 与 `src/`，不要再往旧代码里加功能。

## 1. 命令

```bash
npm install
npm run tauri:dev                      # Vite(8090) + Tauri 壳 + 内嵌 axum 数据面（127.0.0.1:17890）
npm run build                          # 前端构建
cargo test --workspace --manifest-path src-tauri/Cargo.toml   # Rust 测试
node scripts/run-node-tests.mjs        # 前端纯逻辑测试（必须走这个脚本，见下）

# 本地引擎的真实模型测试（默认 #[ignore]，需要模型与音频就位）
ATT_TEST_WAV=/abs/path/audio.wav cargo test -p att-whisper --test real_model -- --ignored --nocapture
ATT_TEST_WAV=/abs/path/audio.wav cargo test -p att-qwen    --test real_model -- --ignored --nocapture
ATT_TEST_WAV=/abs/path/audio.wav cargo test -p att-core    --test local_engine_wiring -- --ignored --nocapture
```

`tests/` 里有手写 `main()` 的 Node 用例会 `process.exit(0)`，直接 `node --test` 会截断汇总。

## 2. 代码布局

```text
src-tauri/
  src/                 Tauri 壳：recording.rs（录音 Actor）、tray / floating / clipboard（监控）/ desktop（粘贴/静音）/ commands
  .cargo/config.toml   Windows MSVC 必须 `+crt-static`：CTranslate2 静态库按 /MT 编译，否则 LNK2038
  crates/att-core/     领域内核：settings / records / llm（注册表·存储·网关·优化）/ stt（豆包·ChatGPT·local 常驻池）/
                       transcribe（文件转写·校对）/ clipboard / prompts / storage（原子写·Fernet）/ migration
  crates/att-whisper/  本地 faster-whisper：ct2rs（CTranslate2）绑定，复用 models/faster_whisper 的 CT2 模型
  crates/att-qwen/     本地 Qwen3-ASR：sherpa-onnx C API（运行期 dlopen）+ ONNX 模型；C 桥见 src/shim.c
  crates/att-server/   axum 数据面（仅回环），HTTP 契约与旧 Python 版一致
  crates/att-audio/    cpal 麦克风采集
  crates/att-hotkey/   全局热键（macOS CGEventTap / Windows WH_KEYBOARD_LL）
src/                   Vue 前端；桌面能力只经 composables/useElectron.js，HTTP 只经 api/client.js
prompts/*.txt          提示词预设唯一真相源（打包时作为资源随包）
data/                  用户数据（开发态即仓库 data/），不入库
models/                本地模型（不入库）：faster_whisper/（CT2 large-v3）、qwen3-asr-0.6B-int8/（sherpa ONNX）、
                       sherpa-onnx/（sherpa-onnx 动态库 + onnxruntime）；可用 ATT_MODELS_DIR / ATT_SHERPA_ONNX_DIR 改位置
```

## 3. 硬约束

1. 注释与文档用中文。
2. JSON 写入一律走 `att_core::storage::json_store`（同目录 tmp + rename），禁止原地截断写。
3. 密钥不落明文：豆包 / ChatGPT 令牌与 LLM 密钥只存 Fernet 密文（`data/.encryption_key`，与旧 Python 密文互认）；
   任何响应、日志、错误文案不得出现密钥（LLM 错误文案统一过 `llm::errors::redact_secrets`）。
4. 数据文件格式保持与旧版一致（`settings.json`、`records/`、`llm_v2.json`、`clipboard/`、`transcribe_file_cache.json`），
   旧数据必须能直接读；模型要有缺字段兜底。
5. 前端访问外部能力只走两个包装层：HTTP → `src/api/client.js`，桌面 → `src/composables/useElectron.js`。
6. 录音编排的纯逻辑放 `src/composables/recorder/*.mjs` 并在 `tests/test_recorder_*.mjs` 补用例；
   Rust 侧录音状态机只在 `src-tauri/src/recording.rs` 的 Actor 内推进。
7. 前端文案改动先查 `tests/test_recorder_*.mjs` 是否被锁死。

## 4. 常见陷阱

| 陷阱 | 说明 |
| --- | --- |
| `data/settings.json` 优先于环境变量 | 引擎、录音模式等以设置页 / `settings.json` 为准 |
| 运行中的 `tauri dev` 会自动重编译 | 改 Rust 代码后正在跑的应用会重启并用新代码读写真实 `data/`（迁移类改动尤其注意，先备份） |
| macOS 权限归属 | 开发态「输入监控」「辅助功能」「麦克风」权限归属启动 `tauri dev` 的终端应用 |
| 思考型模型 | 性能参数（思考强度）只经 `ModelParams.reasoning_effort` → 网关映射；上游不认时网关会自动去掉重试 |
| 确定性失败不要重试 | 思考 token 吃满预算（`output_truncated`）不得回退重跑，否则耗时翻倍 |
| 文件转写依赖 ffmpeg | 未在 PATH 时查 Homebrew 常见路径，或用 `ATT_FFMPEG_DIR` 指定 |
| 本地引擎首次构建慢 | `att-core` 依赖 `att-whisper`，首次会 CMake 编译 CTranslate2 + oneDNN（约 10~20 分钟）；之后走缓存 |
| Qwen 引擎与系统 ORT 冲突 | Windows 上 `C:\Windows\System32\onnxruntime.dll`（旧版）会抢占依赖解析导致崩溃；`shim.c` 已按绝对路径预加载 `models/sherpa-onnx/` 里的 ORT，改动加载逻辑时别丢掉这一步 |
| 本地引擎没有模型时 | 只给 409 + 可照做的下载命令，不 panic；模型不入库，见 `docs/Tauri重写-04-…` §7 的下载清单 |
