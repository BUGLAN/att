# AGENTS.md

给 AI 编码代理（Codex / Claude Code 等）的工作指引。项目：Audio to Text（ATT），**Tauri 2 + Rust + Vue 3** 桌面语音转文字应用。

> 旧 Electron / Python 栈已于 **2026-11-01** 从仓库删除，运行时只依赖 `src-tauri/` 与 `src/`。
> 不要从 git 历史恢复旧代码再改；需要参考旧实现时只读不写。
> 不要再新增「重写阶段 / 交接」类过程文档：现状以代码与 README 为准。

## 1. 命令

```bash
npm install
npm run tauri:dev                      # Vite(8090) + Tauri 壳 + 内嵌 axum 数据面（127.0.0.1:17890）
npm run build                          # 前端构建
npm run test:rust                      # = cd src-tauri && cargo test --workspace
node scripts/run-node-tests.mjs        # 前端纯逻辑测试（必须走这个脚本，见下）

# 本地引擎的真实模型测试（默认 #[ignore]，需要模型与音频就位）
cd src-tauri
ATT_TEST_WAV=/abs/path/audio.wav cargo test -p att-whisper --test real_model -- --ignored --nocapture
ATT_TEST_WAV=/abs/path/audio.wav cargo test -p att-qwen    --test real_model -- --ignored --nocapture
ATT_TEST_WAV=/abs/path/audio.wav cargo test -p att-core    --test local_engine_wiring -- --ignored --nocapture
```

> `cargo` 必须在 `src-tauri/` 目录内执行：`.cargo/config.toml` 的 `+crt-static` 只在工作目录位于其下时生效，
> 从仓库根加 `--manifest-path` 跑会撞 `LNK2038`（CTranslate2 是 /MT，Rust 默认 /MD）。

`tests/` 里有手写 `main()` 的 Node 用例会 `process.exit(0)`，直接 `node --test` 会截断汇总。

## 2. 代码布局

```text
src-tauri/
  src/                 Tauri 壳：recording.rs（录音 Actor）、tray / floating / clipboard（监控）/ desktop（粘贴/静音）/ commands
  .cargo/config.toml   Windows MSVC 必须 `+crt-static`：CTranslate2 静态库按 /MT 编译，否则 LNK2038
  crates/att-core/     领域内核：settings / records / llm（注册表·存储·网关·优化）/ stt（豆包·ChatGPT·local 常驻池）/
                       transcribe（文件转写·校对）/ clipboard / prompts（内置 + 自定义预设）/ update（检查更新）/
                       storage（原子写·Fernet）/ migration
  crates/att-whisper/  本地 faster-whisper：ct2rs（CTranslate2）绑定，复用 models/faster_whisper 的 CT2 模型
  crates/att-qwen/     本地 Qwen3-ASR：sherpa-onnx C API（运行期 dlopen）+ ONNX 模型；C 桥见 src/shim.c
  crates/att-server/   axum 数据面（仅回环），HTTP 契约与旧 Python 版一致
  crates/att-audio/    cpal 麦克风采集
  crates/att-hotkey/   全局热键（macOS CGEventTap / Windows RegisterHotKey + WH_KEYBOARD_LL）
src/                   Vue 前端；桌面能力只经 composables/useElectron.js，HTTP 只经 api/client.js
prompts/*.txt          内置提示词预设（只读，打包时作为资源随包）；用户自定义预设在 data/custom_prompts.json
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
| macOS 构建本地引擎 | 需要 `brew install cmake`；macOS 上 ct2rs 走 Accelerate + ruy（Apple Clang 无 OpenMP，`openmp-runtime-comp` 会去链不存在的 `gomp`），见 `att-whisper/Cargo.toml` |
| dev 下 Whisper 慢 9 倍 | cmake crate 在 opt-level=0 时按 Debug 编 CTranslate2；根 `Cargo.toml` 用 `[profile.dev.package.ct2rs]` 等强制优化，别删 |
| 本地引擎首次构建慢 | `att-core` 依赖 `att-whisper`，首次会 CMake 编译 CTranslate2 + oneDNN（约 10~20 分钟）；之后走缓存 |
| Qwen 引擎与系统 ORT 冲突 | Windows 上 `C:\Windows\System32\onnxruntime.dll`（旧版）会抢占依赖解析导致崩溃；`shim.c` 已按绝对路径预加载 `models/sherpa-onnx/` 里的 ORT，改动加载逻辑时别丢掉这一步 |
| 本地引擎没有模型时 | 只给 409 + 可照做的下载命令，不 panic；模型不入库，路径见上文「代码布局」 |
| 图片写剪贴板 | Windows 上必须自己写 `CF_DIB`(BI_RGB) + `CF_DIBV5`(BI_RGB) + `PNG`（`src-tauri/src/clipboard_image.rs`）；换回 arboard 的 BI_BITFIELDS 写法会让 Chromium 系应用粘不出图 |
| 新增模拟按键 | 自动输入 / 粘贴等自己发出的键盘事件必须打 `att_hotkey::SYNTHETIC_EVENT_TAG`（macOS `EVENT_SOURCE_USER_DATA`、Windows `dwExtraInfo`），否则热键监听会把它清零的修饰键当成用户松开，按住录音时被打断又自动开新一段 |
| 剪贴板偶发写失败 | 剪贴板同一时刻只允许一个线程打开，写入要重试；失败发生在 `EmptyClipboard` 之后会把用户剪贴板清空 |
| 录音文件格式 | 新录音落盘为 FLAC（`audio::write_recording_16k`），旧录音仍是 WAV；读录音一律走 `audio::read_audio_16k_mono`（按扩展名分派），回放端点按扩展名给 Content-Type |
| 记录 / 剪贴板列表有内存索引 | `RecordRepo` / `ClipboardRepo` 的列表、搜索、去重走 `storage::dir_index`，靠目录修改时间发现外部增删；别绕过仓库原地改写 `data/records/`、`data/clipboard/` 里的文件（原地改内容不会被发现，需重启） |
