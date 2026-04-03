# ![ATT Logo](electron/icons/icon.png) Audio-to-Text Desktop

一个面向中文语音输入场景的桌面转写工具，使用 `Electron + Vue + FastAPI` 组合实现。项目同时覆盖“桌面录音入口”“本地/半本地 ASR 转写”“LLM 文本优化”“剪贴板历史与悬浮窗”几条链路，既可以当作日常语音输入工具，也适合作为二次开发的桌面 AI 应用骨架。

> [!IMPORTANT]
> 当前生产路径以 `electron/ + src/ + app/` 为主，旧的 Python 托盘与静态前端链路已经移除，新增功能应优先沿这条路径扩展。

## 功能概览

- 全局快捷键录音，支持按下开始、松开结束，兼容 `Win+Alt` 这类纯修饰键组合。
- 支持三种录音模式：
  - 单段录音：一次录完，一次上传，一次出结果。
  - 多段录音：基于静音阈值自动切段，边说边分段转写，最后合并成完整记录。
  - 流式录音：通过 `/api/asr/stream` WebSocket 持续推送 PCM 数据，实时返回中间文本。
- 支持 `Whisper / faster-whisper` 与 `Qwen3-ASR` 两类 ASR Provider。
- 转写完成后可自动调用默认 LLM 做文本优化。
- 支持剪贴板历史、收藏、置顶、悬浮窗唤起和回填。
- 支持提示词预设、LLM 配置管理、录音设备选择、录音期间系统输出静音与恢复。
- 所有记录、会话、提示词和剪贴板数据默认存放在本地 `data/` 目录，便于调试和迁移。

## 适用场景

### 对使用者

- 把语音快速转成可编辑文本。
- 录制会议片段，再合并为完整文本。
- 在聊天框、文档编辑器、表单输入框中，完成“说完即贴上”的输入流程。
- 把常用片段存进剪贴板悬浮窗，减少重复输入。

### 对开发者

- 作为 Electron 桌面 AI 工具的基础工程。
- 复用现有录音、ASR、LLM、剪贴板和 WebSocket 通信链路。
- 在现有 API 和数据模型上扩展更多桌面自动化能力。

## 界面与核心能力

### 录音链路

- 主界面提供录音状态、记录列表和当前处理结果。
- 全局热键与录音期间静音由 Electron 主进程拉起的本地 Python desktop agent 执行，Renderer 只负责展示和状态同步。
- 录音时可通知桌面代理将系统输出静音，结束后恢复到录音前状态。
- 浮窗会在录音、转写、优化等阶段同步显示状态。

### 剪贴板链路

- 支持监听剪贴板变更并写入本地历史。
- 可通过鼠标中键或左右键同时按下唤起剪贴板悬浮窗。
- 条目支持搜索、收藏、置顶、复制、复制后自动回填。

### 文本优化链路

- 转写后的原始文本可手动或自动触发优化。
- 优化逻辑依赖默认 LLM 配置和默认提示词预设。
- 提示词预设直接来源于 `prompts/` 目录和自定义配置。

## 快速开始

### 1. 环境要求

- Node.js 20+。
- Python `3.12.x`。
- Windows 为当前主要目标平台。
- 如果使用 `Qwen3-ASR` 或 GPU 推理，需要本机具备对应 CUDA / 显卡环境。

### 2. 安装依赖

```bash
uv sync
npm install
```

### 3. 启动方式

仅启动后端：

```bash
npm run backend:dev:uv
```

启动 Electron + Vite Renderer：

```bash
npm run electron:dev
```

构建前端：

```bash
npm run build
```

打包 Electron：

```bash
node scripts/package-electron.js --win
```

运行测试：

```bash
uv run python -m pytest tests -q
```

## 使用说明

### 基本使用流程

1. 启动桌面应用。
2. 在“设置”里选择 ASR Provider、录音模式、麦克风和默认优化预设。
3. 按下全局快捷键开始录音，松开后结束；若按压时间过短，会提示“录音时间过短”并恢复为空闲状态。
4. 等待转写完成。
5. 若开启“自动 AI 优化”，系统会继续生成优化文本。
6. 若开启“自动复制到剪贴板”，结果会自动复制，并尝试粘贴到当前焦点输入框。

### 三种录音模式说明

#### 单段录音

- 最适合短句输入和一次性语音转文字。
- 录音结束后直接走 `/api/audio/upload`。
- 后端异步处理，前端通过 WebSocket 和轮询补偿获知完成状态。

#### 多段录音

- 适合较长表达或讲话有停顿的场景。
- 前端根据静音阈值自动切段。
- 每个片段通过 `/api/sessions/{id}/segments` 上传并单独转写。
- 最后调用 `/api/sessions/{id}/merge` 合并为完整记录。

#### 流式录音

- 适合追求低延迟反馈的场景。
- 通过 `/api/asr/stream` WebSocket 推送音频数据并接收中间结果。
- 当前实现更依赖 `Qwen3-ASR` 运行时与流式服务可用性。

## 已知问题

### 桌面热键依赖本地桌面代理

- 当前桌面端全局热键与录音期间系统静音由 Electron 主进程拉起的本地 Python desktop agent 提供。
- Windows 默认热键仍支持 `Win+Alt` 这类仅由修饰键组成的组合，行为语义为“按下开始、松开结束”。
- macOS 首次使用全局热键前，需要在系统设置中授予输入监听相关权限。

## 项目结构

```text
electron/
  main.js                     Electron 主进程入口
  desktop/                    本地桌面设置持久化
  desktopAgent/               本地桌面代理管理
  audio/controller.js         录音期间的桌面音频控制（代理不可用时的后备路径）
  clipboardOverlay/           剪贴板悬浮窗窗口管理
  floatingWindow/             录音状态浮窗
  python/desktop_agent.py     本地桌面代理
  python/manager.js           Python 后端启动与守护

src/
  components/                Vue 组件
  composables/useRecorder.js 录音主工作流
  composables/useWebSocket.js WebSocket 状态同步
  composables/useSettings.js 设置页请求封装
  api/client.js              前端 HTTP / WebSocket 客户端

app/
  main.py                    FastAPI 入口与生命周期管理
  api/routes.py              主 REST / WebSocket 接口
  audio/processor.py         音频校验、转码、落盘
  stt/                       Whisper / Qwen ASR 集成
  llm/                       LLM 网关、配置、优化逻辑
  records/                   记录与会话管理
  clipboard/                 剪贴板存储与接口

data/
  settings.json              用户设置
  records/                   记录元数据
  audio/records/             WAV 音频文件
  clipboard/                 剪贴板条目
```

## 架构说明

```text
Electron Main
  -> 启动 Python FastAPI
  -> 拉起本地 Python desktop agent（热键、静音）
  -> 管理托盘、浮窗、剪贴板悬浮窗

Vue Renderer
  -> 通过 HTTP / WebSocket 调 FastAPI
  -> 通过 preload API 调用桌面专属能力
  -> 不直接与 desktop agent 通信

FastAPI Backend
  -> 校验与转码音频
  -> 调用 ASR provider 转写
  -> 调用 LLM 优化文本
  -> 管理 records / sessions / prompts / clipboard / settings
```

### 关键模块

- `electron/main.js`
  - 启动后端。
  - 启动并管理本地 desktop agent。
  - 注册 IPC。
  - 创建主窗口、托盘、浮窗、剪贴板悬浮窗。
- `electron/desktop/settings.js`
  - 持久化本地桌面配置。
  - 管理热键和录音静音开关的默认值与归一化。
- `electron/desktopAgent/manager.js`
  - 通过 stdio JSON-RPC 管理 Python desktop agent 生命周期。
  - 转发热键按下、松开、权限与错误事件。
- `src/composables/useRecorder.js`
  - 统一处理单段、多段、流式三套录音模式。
  - 消费 desktop agent 的按下/松开事件并驱动录音状态机。
  - 负责上传、状态同步、自动复制与 UI 提示。
- `app/main.py`
  - 初始化音频处理器、ASR Provider、LLM 优化器、记录管理器、剪贴板管理器。
- `app/api/routes.py`
  - 暴露主要 REST API、状态 WebSocket 和流式 ASR WebSocket。

## 配置说明

### 环境变量

环境变量定义见 `.env.example`，常用项如下：

- `ASR_PROVIDER`
- `ASR_LANGUAGE`
- `ASR_PRELOAD_ON_STARTUP`
- `WHISPER_MODEL`
- `WHISPER_DEVICE`
- `WHISPER_COMPUTE_TYPE`
- `QWEN_ASR_MODEL`
- `QWEN_ASR_DEVICE`
- `QWEN_ASR_DTYPE`
- `QWEN_ASR_MAX_NEW_TOKENS`
- `QWEN_ASR_RETURN_TIMESTAMPS`
- `QWEN_ASR_USE_FORCED_ALIGNER`
- `QWEN_STREAMING_BACKEND`
- `QWEN_STREAMING_BASE_URL`

> [!NOTE]
> LLM 服务配置不依赖 `.env` 中的固定单一 Provider，而是通过设置页中的 “LLM 配置” 进行管理。

### 用户设置

业务用户设置保存在 `data/settings.json`，主要包括：

- `asr_provider`
- `recording_mode`
- `pause_audio_on_recording`
- `auto_optimize`
- `auto_copy_to_clipboard`
- `hotkey`
- `audio_input_device`
- `silence_threshold`
- `silence_level`
- `clipboard_overlay_*`
- `default_preset_id`

桌面本地设置由 Electron 单独持久化，默认位于 Electron `userData` 目录下的 `desktop-settings.json`，主要包括：

- `hotkey`
- `pause_audio_on_recording`

## 后端 API

以下为当前代码中的主要接口分组。

### 健康检查与运行时

| 方法 | 路径 | 说明 |
| --- | --- | --- |
| `GET` | `/api/health` | 健康检查，可通过 `probe_runtime=true` 探测运行时状态 |
| `GET` | `/api/model-service` | 查询模型服务状态 |
| `POST` | `/api/model-service/start` | 启动模型服务 |
| `POST` | `/api/model-service/stop` | 停止模型服务 |
| `GET` | `/api/streaming/service` | 查询流式服务状态 |
| `POST` | `/api/streaming/service/start` | 启动流式服务 |
| `POST` | `/api/streaming/service/stop` | 停止流式服务 |

### 录音与记录

| 方法 | 路径 | 说明 |
| --- | --- | --- |
| `POST` | `/api/audio/upload` | 上传单段录音并异步转写 |
| `GET` | `/api/records` | 获取记录列表 |
| `GET` | `/api/records/search` | 搜索记录 |
| `GET` | `/api/records/{id}` | 获取记录详情 |
| `GET` | `/api/records/{id}/status` | 查询转写状态 |
| `PUT` | `/api/records/{id}` | 更新文本、收藏等字段 |
| `DELETE` | `/api/records/{id}` | 删除记录 |
| `GET` | `/api/records/{id}/audio` | 获取录音文件 |
| `GET` | `/api/records/{id}/export` | 导出文本或 JSON |
| `POST` | `/api/records/{id}/optimize` | 手动触发文本优化 |

### 多段会话

| 方法 | 路径 | 说明 |
| --- | --- | --- |
| `POST` | `/api/sessions` | 创建多段录音会话 |
| `GET` | `/api/sessions/{id}` | 获取会话详情 |
| `POST` | `/api/sessions/{id}/segments` | 上传一个录音片段 |
| `POST` | `/api/sessions/{id}/merge` | 合并多段结果并生成记录 |
| `DELETE` | `/api/sessions/{id}` | 删除会话 |

### 设置

| 方法 | 路径 | 说明 |
| --- | --- | --- |
| `GET` | `/api/settings` | 获取用户设置 |
| `PUT` | `/api/settings` | 更新用户设置 |

### 提示词预设

| 方法 | 路径 | 说明 |
| --- | --- | --- |
| `GET` | `/api/prompts` | 获取全部提示词预设 |
| `PUT` | `/api/prompts/default` | 设置默认提示词 |
| `GET` | `/api/prompts/{id}` | 获取单个预设 |
| `POST` | `/api/prompts` | 创建预设 |
| `PUT` | `/api/prompts/{id}` | 更新预设 |
| `DELETE` | `/api/prompts/{id}` | 删除预设 |
| `POST` | `/api/prompts/{id}/preview` | 预览渲染后的提示词 |

### LLM 配置

| 方法 | 路径 | 说明 |
| --- | --- | --- |
| `GET` | `/api/llm/configs` | 获取 LLM 配置列表 |
| `GET` | `/api/llm/configs/{id}` | 获取单个配置 |
| `POST` | `/api/llm/configs` | 新建配置 |
| `PUT` | `/api/llm/configs/{id}` | 更新配置 |
| `DELETE` | `/api/llm/configs/{id}` | 删除配置 |
| `PUT` | `/api/llm/configs/{id}/default` | 设置默认配置 |
| `POST` | `/api/llm/configs/{id}/test` | 测试连接 |
| `GET` | `/api/llm/providers` | 获取支持的 Provider 列表 |
| `GET` | `/api/llm/providers/{provider}/models` | 获取某 Provider 的模型列表 |

### 剪贴板

| 方法 | 路径 | 说明 |
| --- | --- | --- |
| `GET` | `/api/clipboard/items` | 获取剪贴板条目列表 |
| `GET` | `/api/clipboard/stats` | 获取统计信息 |
| `POST` | `/api/clipboard/items` | 新建条目 |
| `GET` | `/api/clipboard/items/{id}` | 获取单个条目 |
| `PUT` | `/api/clipboard/items/{id}` | 更新条目 |
| `DELETE` | `/api/clipboard/items/{id}` | 删除条目 |

### WebSocket

| 方法 | 路径 | 说明 |
| --- | --- | --- |
| `WS` | `/api/ws` | 通用状态同步，广播转写开始、完成、优化完成等事件 |
| `WS` | `/api/asr/stream` | 流式 ASR 会话，支持 `stream_start / stream_finish / stream_cancel / ping` |

## 数据落盘

项目默认使用 JSON 文件做本地持久化，便于桌面应用调试和快速迭代。

- `data/settings.json`
  - 用户设置。
- `data/records/`
  - 转写记录元数据。
- `data/audio/records/`
  - 持久化后的 WAV 文件。
- `data/clipboard/`
  - 剪贴板历史数据。
- `prompts/`
  - 内置或外置提示词文本。

## 开发建议

### 推荐阅读顺序

1. `AGENTS.md`
2. `electron/main.js`
3. `src/composables/useRecorder.js`
4. `src/components/settings/SettingsPage.vue`
5. `app/main.py`
6. `app/api/routes.py`

### 二次开发切入点

- 新增桌面能力：优先放在 `electron/`，通过 preload / IPC 暴露给前端。
- 新增业务接口：优先放在 `app/api/routes.py` 或拆分子路由。
- 新增转写后处理：沿 `app/llm/` 和 `prompts/` 扩展。
- 新增录音模式：先看 `src/composables/useRecorder.js` 的状态机和 WebSocket 协调方式。

## 常见问题

### 1. 启动后只能看到前端，录音不可用

通常是 Python 后端未正常启动，或者 Electron 未拿到后端端口。优先检查：

- `npm run backend:dev:uv` 能否单独启动。
- Python 依赖是否已通过 `uv sync` 安装完成。
- `electron/python/manager.js` 启动日志和 `logs/` 下输出。

### 2. 快捷键按下没有反应

快捷键监听由本地 Python desktop agent 负责，不是浏览器页面自己监听。优先检查：

- 设置页中的快捷键是否已成功保存。
- 当前快捷键是否与系统级快捷键冲突。
- Windows / macOS 本机是否允许当前组合键。
- desktop agent 依赖是否已通过 `uv sync` 安装完成。
- macOS 是否已授予输入监听权限。

### 3. 录音完成后没有自动粘贴

“自动复制到剪贴板”依赖两步：

- 先写系统剪贴板。
- 再调用桌面侧粘贴辅助脚本回填到当前焦点窗口。

如果只复制成功、没有粘贴成功，通常是系统焦点切换、权限限制或粘贴辅助脚本执行失败。

### 4. Qwen3-ASR 真流式不可用

真流式模式依赖运行时环境和模型服务状态。当前实现里：

- 只有在流式服务可用时，前端才会进入 `/api/asr/stream` 链路。
- 若运行时不支持或服务未就绪，设置页会给出“不可用”提示。
- 在部分环境下会自动退回更保守的录音链路，而不是直接进入真流式。

### 5. 多段录音切段不理想

优先调整以下参数：

- `silence_threshold`
- `silence_level`
- 麦克风输入设备

多段录音是前端静音检测驱动的，环境噪音、麦克风增益和说话停顿方式都会影响切段效果。

### 6. 转写完成了，但记录列表状态不对

当前前端采用 “WebSocket 事件 + 轮询补偿” 双保险机制。如果状态偶尔不同步，优先检查：

- `/api/ws` 是否已连上。
- `/api/records/{id}/status` 是否返回了正确状态。
- 前端是否因刷新页面丢失了当前会话状态。

## 已知限制

- 数据层当前以本地 JSON 文件为主，不适合多用户并发共享。
- 流式识别依赖运行时能力，跨平台一致性仍需要继续打磨。
- 大模型与 ASR 模型启动时间可能较长，首次使用有明显预热成本。
- 当前 README 以代码现状为准，若后续拆分路由或更换存储层，需要同步更新。
