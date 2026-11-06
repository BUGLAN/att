# 落墨 Luomo（原 Audio to Text / ATT）

[![最新版本](https://img.shields.io/github/v/release/BUGLAN/att?label=%E6%9C%80%E6%96%B0%E7%89%88%E6%9C%AC)](https://github.com/BUGLAN/att/releases/latest)
[![CI](https://github.com/BUGLAN/att/actions/workflows/ci.yml/badge.svg)](https://github.com/BUGLAN/att/actions/workflows/ci.yml)

Windows / macOS 桌面**语音输入与录音笔记**工具：按住快捷键说话 → 语音识别 →（可选）LLM 整理 → 直接输入到当前光标处，
同时落库成可搜索的记录。另带剪贴板历史、选中文本一键改写、本地文件转写。

数据全部存在本机（JSON 文件）；只有在线识别与 LLM 优化会把音频 / 文本发给你自己配置的服务。
技术栈：**Tauri 2 + Rust 内核 + Vue 3**，单进程原生应用，运行时不依赖 Python / Node。

## 下载安装

到 [Releases](https://github.com/BUGLAN/att/releases/latest) 下载最新版：

| 平台 | 安装包 | 说明 |
| --- | --- | --- |
| macOS（Apple Silicon） | `.dmg` | 安装包未签名：首次打开若提示「无法验证开发者」，到「系统设置 → 隐私与安全性」点「仍要打开」 |
| Windows（x64） | `-setup.exe`（推荐）或 `.msi` | 安装包未签名：SmartScreen 提示时选「更多信息 → 仍要运行」 |

macOS 需要授予三项权限：「麦克风」（录音）、「输入监控」（全局快捷键）、「辅助功能」（自动粘贴、优化选中文本）。

之后可以在「设置 → 关于」里检查更新；打开「自动下载更新」后，检查到新版本会在后台下载更新包，下好后点「重启并更新」即可（不会自己重启打断录音）。

## 功能

| 能力 | 说明 |
| --- | --- |
| 录音 | 单段 / 多段（静音自动切段，边录边出字）/ 流式（豆包，边说边出字）；按住快捷键录音、松开结束，录音悬浮条实时显示字幕 |
| 语音识别 | 火山豆包大模型（流式 + 批量）、ChatGPT 听写、本地 faster-whisper（CT2 large-v3）、本地 Qwen3-ASR（ONNX）；个人词表（豆包热词 + LLM 纠错） |
| LLM 优化 | 内置服务商目录（DeepSeek / OpenAI / Anthropic / Gemini / 通义 / 豆包方舟 / Kimi / 智谱 / Ollama / 任意 OpenAI 兼容 …），流式输出；可开启「录音后自动优化并输入」 |
| 提示词预设 | 内置多套预设（通用优化、程序员模式、会议记录、Agent 提示词等，只读），也可以新增、编辑、删除自己的预设 |
| 优化选中文本 | 在任意应用里选中文字按快捷键，用所选预设改写后流式替换原文 |
| 剪贴板 | 自动记录复制的文本与图片，收藏 / 置顶 / 图片预览；悬浮框快捷键唤起，选中即粘贴 |
| 文件转写 | 音视频文件转文字（需系统安装 ffmpeg），可选 LLM 滑窗校对，结果缓存 |
| 记录库 | 搜索、分页、编辑、收藏、导出 TXT / JSON、音频回放 |
| 桌面集成 | 托盘（最近记录一键粘贴）、录音时暂停其他音频、自动粘贴到焦点窗口、检查更新 |

默认快捷键（「设置 → 快捷键」可改）：

| 动作 | macOS | Windows |
| --- | --- | --- |
| 按住录音（按住说话，松开结束） | `⌘⇧R` | `Win+Alt` |
| 点按录音（按一下开始，再按一下结束） | 默认为空 | 默认为空 |
| 剪贴板悬浮框 | `⌃⌥V` | `Ctrl+Alt+V` |
| 优化选中文本 | `⌥A` | `Alt+A` |

按住录音与点按录音可以设成不同的键同时使用；想用同一个键，先清空其中一个，同一个键只能启用一种录音方式。

本地识别模型不随安装包分发；用本地 Whisper / Qwen 时按「设置 → 识别引擎」的提示下载。

## 开发

```bash
npm install
npm run tauri:dev        # 开发运行：Vite(8090) + Tauri 壳 + 内嵌数据面，数据目录 = 仓库 data/
npm run tauri:build      # 本地打包
npm test                 # = test:node（前端纯逻辑用例）+ test:rust（cargo test --workspace）
```

- `cargo` 命令要在 `src-tauri/` 目录内执行（Windows 的 `+crt-static` 配置只在该目录下生效）。
- 首次构建会编译 CTranslate2（约 10~20 分钟），macOS 需先 `brew install cmake`。
- 本地 Whisper / Qwen 的模型与推理库不随仓库分发；选择对应引擎后按「设置 → 识别引擎」的提示下载，模型目录默认 `models/`，可用 `ATT_MODELS_DIR` / `ATT_SHERPA_ONNX_DIR` 改位置。
- 开发态的 macOS 权限归属启动 `tauri dev` 的终端应用。
- 更多开发约定与常见陷阱见 [`AGENTS.md`](AGENTS.md)。

可选环境变量（应用不读 `.env` 文件，需在启动前设置）：

| 变量 | 作用 |
| --- | --- |
| `ATT_DATA_DIR` | 数据目录（默认：开发态为仓库 `data/`，安装版为系统应用数据目录） |
| `ATT_MODELS_DIR` / `ATT_SHERPA_ONNX_DIR` | 本地模型根目录 / sherpa-onnx 动态库目录 |
| `ATT_SILERO_VAD` | Silero VAD 模型文件（默认在模型目录里找 `silero_vad.onnx`） |
| `ATT_WHISPER_COMPUTE_TYPE` | Whisper 计算精度（默认 `auto`：CUDA 用 float16，CPU 用 int8） |
| `ATT_QWEN_THREADS` | Qwen 推理线程数（默认 2） |
| `ATT_FFMPEG_DIR` | ffmpeg 所在目录（文件转写；默认查 PATH 与 Homebrew 常见路径） |
| `ATT_PROMPTS_DIR` | 内置提示词目录 |
| `ATT_BACKEND_PORT` | 数据面优先端口（默认 17890，被占用时随机） |
| `ATT_UPDATE_URL` | 检查更新的地址 |
| `ATT_FORCE_MIGRATION=1` | 重跑一次性数据迁移 |
| `DOUBAO_ASR_API_KEY` | 豆包密钥兜底（设置页填写的优先） |
| `TRANSCRIBE_FILE_API_TOKEN` | 非空时文件转写接口要求请求头 `X-ATT-Token` |

发布：版本号写在 `package.json`、`src-tauri/Cargo.toml`、`src-tauri/tauri.conf.json`（及对应 lock 文件），
用 `npm run version:bump -- 2.8.0` 一次改齐（`node scripts/bump-version.mjs --check` 只校验，Node 测试里也会校验）；
推送 `v*` 注释标签后，`.github/workflows/release.yml` 自动打包 macOS / Windows 安装包并发布 Release，
标签注释正文即版本说明。

应用内更新：发布时用仓库 Secrets 里的 `TAURI_SIGNING_PRIVATE_KEY` / `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`
给更新包签名，并生成 `latest.json`；已安装的应用据此下载、校验签名后「重启并更新」。公钥在 `tauri.conf.json`
的 `plugins.updater.pubkey`，换密钥后旧版本将无法校验新版本的更新包，只能手动下载安装包更新。
本地 `npm run tauri build` 同样需要这两个环境变量；不需要更新包时加
`--config '{"bundle":{"createUpdaterArtifacts":false}}'`。
