# Audio to Text（ATT）

Windows / macOS 桌面**录音笔记**工具：**录音 → 语音识别 → （可选）LLM 优化 → 落库 / 复制粘贴**，附带剪贴板历史与本地文件转写。
数据全部落在本地 `data/` 目录的 JSON 文件里（在线识别与 LLM 只上传音频 / 文本）。

> 架构已从 Electron + Python 重写为 **Tauri 2 + Rust**。设计与进度见 `docs/Tauri重写-*`，
> 剩余工作（本地 Whisper / Qwen 引擎、旧代码下线）见 `docs/Tauri重写-04-交接-剩余移植与Python下线.md`。

## 功能

| 能力 | 说明 |
| --- | --- |
| 录音 | 单段 / 多段（静音自动切段，边录边出字）/ 流式（豆包，边说边出字）；全局热键按住录音、松开结束 |
| 语音识别 | 火山豆包大模型（流式 + 批量）、ChatGPT 听写（批量）；本地 faster-whisper / Qwen3-ASR 待移植 |
| LLM 优化 | 14 家服务商（DeepSeek / OpenAI / Anthropic / Gemini / 通义 / 豆包方舟 / Kimi / 智谱 / Ollama / 任意 OpenAI 兼容 …），提示词预设，流式优化，可开启「录音后自动优化并粘贴」 |
| 文件转写 | 音视频文件（需系统安装 ffmpeg）转文字，可选 LLM 滑窗校对，结果缓存 |
| 记录库 | 搜索、分页、编辑、收藏、导出 TXT / JSON、音频回放 |
| 剪贴板历史 | 自动记录复制的文本与图片，收藏 / 置顶 / 图片预览 |
| 桌面集成 | 托盘（最近记录一键粘贴）、悬浮窗实时字幕、录音时系统静音、自动粘贴到焦点窗口 |

## 开发

```bash
npm install
npm run tauri:dev        # 开发运行（数据目录 = 仓库 data/）
npm run tauri:build      # 打包
cargo test --workspace --manifest-path src-tauri/Cargo.toml
node scripts/run-node-tests.mjs
```

macOS 首次运行需授予「麦克风」「输入监控」（全局热键）「辅助功能」（自动粘贴）权限；开发态权限归属启动 `tauri dev` 的终端。
