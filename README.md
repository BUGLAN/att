# Audio to Text（ATT）

一款 Windows / macOS 桌面**录音笔记**工具：**录音 → 语音识别 → （可选）LLM 优化 → 落库/导出/复制**，
并附带剪贴板历史。产品定位是「简单的录音笔记」：记录每一次录音，保存音频与文字。

- 语音识别支持**本地离线**（faster-whisper、Qwen3-ASR）与**在线流式**（火山引擎豆包双向流式 ASR）
- 文本优化走可配置的 LLM（DeepSeek / Claude / OpenAI / 豆包 / 通义千问 / 任意 OpenAI 兼容服务）
- 数据全部落在本地 `data/` 目录的 JSON 文件里，不上传云端（在线 ASR 除外，只上传音频流）

> 本文档基于代码重新编写（2026-10）。历史文档已删除，可从 git 历史找回。

---

## 功能一览

| 能力 | 说明 |
| --- | --- |
| 单段录音 | 录完整段后统一上传转写 |
| 多段准实时 | 按静音自动切段，边录边转写并持续累积文本 |
| 流式录音 | WebSocket 实时推流，边说边出字（仅 `qwen_asr` / `doubao_asr` 支持） |
| 语音识别 | `whisper`（faster-whisper）、`qwen_asr`（Qwen3-ASR）、`doubao_asr`（火山豆包在线流式） |
| 文本优化 | 多 LLM 配置管理，支持预设提示词；可开启「转写后自动优化」 |
| 记录库 | 搜索、分页、编辑、收藏、导出（TXT / JSON）、音频回放 |
| 剪贴板历史 | 自动记录复制的文本与图片，支持收藏 / 置顶 / 复制 / 图片预览 |
| 提示词预设 | 内置 5 个预设（会议记录 / 小说创作 / 文案优化 / 程序员模式 / 通用优化） |
| 桌面集成 | 全局热键、录音时暂停系统音频、托盘菜单、悬浮窗实时字幕 |
| 数据安全 | JSON 原子写 + 单写者锁；API Key 加密落盘（Fernet） |

---

## 技术栈

| 层 | 技术 |
| --- | --- |
| 桌面外壳 | Electron 29（主进程 `electron/`） |
| 渲染进程 | Vue 3 + Pinia + Vite 5（`src/`） |
| 后端 | FastAPI + Pydantic v2 + uvicorn（`app/`） |
| 语音识别 | faster-whisper / Qwen3-ASR（vLLM）/ 火山豆包 SAUC 协议 |
| 存储 | 本地 JSON 文件（`data/`），原子写 + 跨进程单写者锁 |
| 依赖管理 | Python：`uv` + `pyproject.toml`；前端：`npm` + `package.json` |

---

## 架构

```text
Electron 主进程 (electron/)
  ├─ python/manager.js        拉起并监控 FastAPI 后端（动态分配端口）
  ├─ desktopAgent/            本地 Python 代理：全局热键 + 录音静音
  ├─ tray/                    托盘菜单（记录列表 / 设置 / 退出）
  ├─ floatingWindow/          录音悬浮窗（实时字幕）
  └─ clipboardMonitor/        剪贴板轮询与落库

Vue 渲染进程 (src/)
  ├─ stores/                  Pinia：app / settings / records
  ├─ composables/             录音编排（useRecorder）+ recorder/*.mjs 纯逻辑
  └─ components/              页面与基础原语（common/ → 页面组件）

FastAPI 后端 (app/)
  ├─ api/routers/             HTTP 与 WebSocket 端点（只做解析与组装）
  ├─ services/                编排：settings / records / transcription / model_service
  ├─ repositories/            JSON 持久化：原子写、单写者锁、索引、路径安全
  ├─ stt/                     ASR provider（批量接口 + 流式接口 + 工厂）
  ├─ llm/                     LLM 配置管理、网关、优化器、多 provider 实现
  ├─ records/ clipboard/ prompts/  业务域
  └─ config.py                环境变量 Settings + data/settings.json 用户设置
```

一次录音的完整链路：

```text
采集 → （单段）上传 /（多段）按静音切段上传 /（流式）WS 二进制帧
     → 音频校验与转 WAV（16kHz 单声道）
     → ASR provider 转写
     → （可选）LLM 优化
     → 记录落库（data/records/*.json）
     → WebSocket 广播状态给渲染进程与悬浮窗
```

---

## 快速开始

### 1. 环境准备

```bash
# Python 依赖（要求 Python 3.12）
uv sync

# 前端依赖
npm install

# 环境变量（首次）
copy .env.example .env      # PowerShell: Copy-Item .env.example .env
```

### 2. 启动

```bash
# 推荐：Electron + Vite（Electron 会自动拉起后端，端口从 8000 起自动挑选）
npm run electron:dev

# 只跑后端（调试 API）
npm run backend:dev:uv

# 复用已启动的后端
npm run electron:dev:existing-backend
```

### 3. 首次配置

打开「设置」页完成三件事：

1. **语音识别**：选择 ASR Provider。本地识别需在首页点「开启模型」预热；选豆包在线识别只需填 Access Token，不需要预热。
2. **LLM 服务**：新增一个 LLM 配置（默认 DeepSeek），否则「自动优化」不可用。
3. **提示词预设**：选择默认预设（内置 5 个，来自 `prompts/*.txt`）。

### 4. Docker（可选）

实时流式 ASR 的桥接服务可以跑在容器里（`docker-compose.yml`）：

```powershell
.\att-start.bat          # 启动容器化后端
.\att-start-app.bat      # 启动容器 + Electron 桌面端
.\att-status.bat         # 查看状态
.\att-stop.bat           # 停止
```

---

## 目录结构

```text
app/                     FastAPI 后端（见 docs/架构说明.md）
electron/                Electron 主进程与本地 Python 代理
src/                     Vue 渲染进程
tests/                   pytest（后端）+ Node 测试（前端纯逻辑）
scripts/                 打包、Docker、数据迁移、清理、探针脚本
prompts/                 提示词预设（*.txt，运行时唯一真相源）
docs/                    项目文档（见下方索引）
data/                    运行时数据（不进版本库）：settings.json、records/、audio/、clipboard/…
models/                  本地模型缓存（不进版本库）
```

---

## 测试与质量门禁

```bash
uv run python -m pytest tests -q    # 后端：281 个用例
node scripts/run-node-tests.mjs     # 前端纯逻辑：15 个文件
npm run build                       # 渲染进程构建
```

- CI 配置见 `.github/workflows/ci.yml`：前端 job 为阻塞门禁，后端 lint 目前是 `continue-on-error` 的非阻塞基线。
- 后端 lint/format 用 `ruff.toml`，前端用 `.prettierrc` + `.editorconfig`。
- 测试不得读取本机 `data/settings.json`（`tests/conftest.py` 有 autouse 隔离 fixture）。

---

## 打包

```bash
node scripts/package-electron.js --win
```

打包配置在 `electron-builder.yml`：代码进 `app.asar`，`python-dist` / `app` / `prompts` 解包到 `resources/`。
`scripts/package-electron.js` 内置 `verifyPackagedResources()` 做产物自检。

> ⚠️ 当前状态：`win.target` 同时声明了 `nsis` 与 `portable`，而实测 `nsis` 阶段会失败
> （`release/` 下只有 `win-unpacked`、没有安装包；见 `docs/已知问题.md`）。
> 需要安装包时请先按该文档处理，或改用 portable 产物。

---

## 文档索引

| 文档 | 内容 |
| --- | --- |
| [docs/使用说明.md](docs/使用说明.md) | 面向用户：三种录音模式、热键、记录库、剪贴板、设置项逐条说明、FAQ |
| [docs/架构说明.md](docs/架构说明.md) | 三层架构、后端分层、数据流、ASR 抽象、持久化与并发约束、Electron 模块 |
| [docs/配置说明.md](docs/配置说明.md) | 环境变量全表（含「写了不生效」的坑）、`settings.json` 全量键、优先级、密钥与加密 |
| [docs/API.md](docs/API.md) | 全部 HTTP 端点 + 两个 WebSocket 的帧格式与状态码约定 |
| [docs/ASR接入.md](docs/ASR接入.md) | 三个 ASR provider 的接入方式、豆包 SAUC 协议要点、探针验证、如何新增 provider |
| [docs/开发与运维.md](docs/开发与运维.md) | 环境、命令表、测试、lint、打包、Docker、数据目录与清理、排障手册 |
| [docs/已知问题.md](docs/已知问题.md) | 当前确认的问题清单（按严重度），带证据路径与建议动作 |
| [AGENTS.md](AGENTS.md) | 给 AI 编码代理的工作指引（架构约定、易错点、改动前必读） |

---

## 已知问题（摘要）

完整清单、证据与「会导致什么」见 [docs/已知问题.md](docs/已知问题.md)。最需要知道的三条：

1. **`--win` 打包在 nsis 阶段失败** → 产不出安装包（只有 `win-unpacked` 免安装目录）；
   且 `extraResources` 未包含后端会 spawn 的 `scripts/`，**打包版里 Qwen 流式录音起不来**。
2. **多段录音的最后一片上传失败会卡住** → 界面永久停在"正在转写"、后续录音无法开始，
   只能重启应用，而且**这段录音实际丢失**（临时分片与后端会话泄漏）。
3. **LLM provider 没有请求超时**（SDK 默认 600s）→ 网络异常时优化挂最多 10 分钟，
   界面一直转圈，记录长时间停在 `optimizing`。
