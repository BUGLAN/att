# 落墨 Luomo（原 Audio to Text / ATT）

Windows / macOS 桌面**录音笔记**工具：**录音 → 语音识别 →（可选）LLM 优化 → 落库 / 复制粘贴**，附带剪贴板历史与本地文件转写。
数据全部落在本地 `data/` 目录的 JSON 文件里（在线识别与 LLM 只上传音频 / 文本）。

技术栈：**Tauri 2 + Rust 内核 + Vue 3**，单进程原生应用，运行时不依赖 Python / Node。

| 文档 | 内容 |
| --- | --- |
| [`docs/架构.md`](docs/架构.md) | 当前实现：三条平面、模块分工、录音/识别/LLM/存储/桌面壳、构建与测试 |
| [`docs/遗留与下线.md`](docs/遗留与下线.md) | 已知限制、待办、旧代码下线清单、模型准备、Windows 验收表 |

## 功能

| 能力 | 说明 |
| --- | --- |
| 录音 | 单段 / 多段（静音自动切段，边录边出字）/ 流式（豆包，边说边出字）；全局热键按住录音、松开结束 |
| 语音识别 | 火山豆包大模型（流式 + 批量）、ChatGPT 听写（批量）、本地 faster-whisper（CT2 large-v3）、本地 Qwen3-ASR（ONNX） |
| LLM 优化 | 内置服务商目录（DeepSeek / OpenAI / Anthropic / Gemini / 通义 / 豆包方舟 / Kimi / 智谱 / Ollama / 任意 OpenAI 兼容 …），提示词预设，流式优化，可开启「录音后自动优化并粘贴」 |
| 文件转写 | 音视频文件（需系统安装 ffmpeg）转文字，可选 LLM 滑窗校对，结果缓存 |
| 记录库 | 搜索、分页、编辑、收藏、导出 TXT / JSON、音频回放 |
| 剪贴板 | 自动记录复制的文本与图片，收藏 / 置顶 / 图片预览；悬浮框（全局快捷键唤起，选中即粘贴） |
| 桌面集成 | 托盘（最近记录一键粘贴）、录音悬浮窗实时字幕、录音时系统静音、自动粘贴到焦点窗口 |

## 开发

```bash
npm install
npm run tauri:dev        # 开发运行（数据目录 = 仓库 data/）
npm run tauri:build      # 打包
npm test                 # = test:node（前端纯逻辑用例）+ test:rust（cargo test --workspace）
```

本地 Whisper / Qwen 需要先准备模型与推理库（不随仓库分发），见 [`docs/遗留与下线.md` §5](docs/遗留与下线.md#5-新机器准备模型与推理库不进仓库)。

macOS 首次运行需授予「麦克风」「输入监控」（全局热键）「辅助功能」（自动粘贴）权限；开发态权限归属启动 `tauri dev` 的终端。
