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

之后可以在「设置 → 关于」里检查更新；打开「自动下载更新」后，检查到新版本会自动下载安装包（只下载，由你打开安装）。

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
| 录音（按住说话） | `⌘⇧R` | `Win+Alt` |
| 剪贴板悬浮框 | `⌃⌥V` | `Ctrl+Alt+V` |
| 优化选中文本 | `⌥A` | `Alt+A` |

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
- 本地 Whisper / Qwen 的模型与推理库不进仓库，准备方法见 [`docs/遗留与下线.md` §5](docs/遗留与下线.md#5-新机器准备模型与推理库不进仓库)。
- 开发态的 macOS 权限归属启动 `tauri dev` 的终端应用。

发布：版本号写在 `package.json`、`src-tauri/Cargo.toml`、`src-tauri/tauri.conf.json`（及对应 lock 文件）；
推送 `v*` 注释标签后，`.github/workflows/release.yml` 自动打包 macOS / Windows 安装包并发布 Release，
标签注释正文即版本说明。

## 文档

| 文档 | 内容 |
| --- | --- |
| [`docs/架构.md`](docs/架构.md) | 当前实现：三条平面、模块分工、录音 / 识别 / LLM / 存储 / 桌面壳 / 前端、构建与测试、环境变量 |
| [`docs/遗留与下线.md`](docs/遗留与下线.md) | 已知限制、待办、旧栈删除留档、新机器模型准备、Windows 实机验收表 |
| [`AGENTS.md`](AGENTS.md) | 给 AI 编码代理的工作约定与常见陷阱 |
