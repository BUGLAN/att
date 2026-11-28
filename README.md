# 落墨 Luomo（原 Audio to Text / ATT）

[![最新版本](https://img.shields.io/github/v/release/BUGLAN/att?label=%E6%9C%80%E6%96%B0%E7%89%88%E6%9C%AC)](https://github.com/BUGLAN/att/releases/latest)
[![CI](https://github.com/BUGLAN/att/actions/workflows/ci.yml/badge.svg)](https://github.com/BUGLAN/att/actions/workflows/ci.yml)

Windows / macOS 桌面**语音输入与录音笔记**工具：按住快捷键说话 → 语音识别 →（可选）LLM 整理 → 直接输入到当前光标处，
同时落库成可搜索的记录。另带剪贴板历史、选中文本一键改写、本地文件转写。

数据全部存在本机（JSON 文件），可在「设置 → 诊断」一键导出备份或从备份恢复；只有在线识别与 LLM 优化会把音频 / 文本发给你自己配置的服务。
技术栈：**Tauri 2 + Rust 内核 + Vue 3**，单进程原生应用，运行时不依赖 Python / Node。

## 下载安装

到 [Releases](https://github.com/BUGLAN/att/releases/latest) 下载最新版：

| 平台 | 安装包 | 说明 |
| --- | --- | --- |
| macOS（Apple Silicon） | `.dmg` | 安装包未签名：首次打开若提示「无法验证开发者」，到「系统设置 → 隐私与安全性」点「仍要打开」 |
| Windows（x64） | `-setup.exe`（推荐）或 `.msi` | 安装包未签名：SmartScreen 提示时选「更多信息 → 仍要运行」 |

macOS 需要授予三项权限：「麦克风」（录音）、「输入监控」（全局快捷键）、「辅助功能」（自动粘贴、优化或语音改写选中文本）。

之后可以在「设置 → 关于」里检查更新；打开「自动下载更新」后，检查到新版本会在后台下载更新包，下好后点「重启并更新」即可（不会自己重启打断录音）。

## 功能

| 能力 | 说明 |
| --- | --- |
| 录音 | 单段 / 多段（静音自动切段，边录边出字）/ 流式（豆包，边说边出字）；按住快捷键录音、松开结束，录音悬浮条实时显示字幕 |
| 语音识别 | 火山豆包大模型（流式 + 批量）、豆包网页语音（匿名在线批量）、ChatGPT 听写、DeepSeek 语音识别（在线批量）、微信输入法语音（Windows 本机注入）、本地 faster-whisper（CT2 large-v3）、本地 Qwen3-ASR（ONNX）、本地 SenseVoice（中粤英日韩 ONNX）；个人词表（豆包热词 + LLM 纠错） |
| LLM 优化 | 内置服务商目录（DeepSeek / OpenAI / Anthropic / Gemini / 通义 / 豆包方舟 / Kimi / 智谱 / Ollama / 任意 OpenAI 兼容 …），流式输出；可开启「录音后自动优化并输入」 |
| 提示词预设 | 内置多套预设（通用优化、程序员模式、会议记录、Agent 提示词等，只读），也可以新增、编辑、删除自己的预设 |
| 优化选中文本 | 在任意应用里选中文字按快捷键，用所选预设改写后流式替换原文 |
| 语音改写选中文本 | 选中文字后按住快捷键说出要求（如「改成更正式的语气」「翻译成英文」「压缩成三条」），识别后按固定模板改写并流式替换原文 |
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
| 语音改写选中文本 | 默认为空 | 默认为空 |

按住录音与点按录音可以设成不同的键同时使用；想用同一个键，先清空其中一个，同一个键只能启用一种录音方式。

本地识别模型不随安装包分发；Qwen3-ASR / SenseVoice 所需的 sherpa-onnx / ONNX Runtime 推理库已包含在安装包里。
用本地 Whisper / Qwen / SenseVoice 时在「设置 → 识别引擎」里直接下载模型（带进度、取消、断点续传、SHA-256 校验与源站固定版本，国内可切 hf-mirror.com 镜像），装好后可查看占用并删除；Whisper 会一并安装 Silero VAD。
本地 Whisper 默认用 CPU + int8（安装包也是）；要在这台机器上用 CUDA 加速，按「开发」一节的 `ATT_CUDA` 自建即可，没有 GPU 的机器会自动回退 CPU。
豆包网页语音使用网页匿名会话，无需密钥；识别时音频会发往字节跳动服务器，属于非官方链路，服务端协议变化时可能暂时失效。
微信输入法语音仅支持 Windows：ATT 直接连接本机正在运行的微信输入法，不另起桥进程；首次构建会下载并静态链接 Frida Core（无需 libclang）。该链路不是官方 API，输入法升级后可能失效，音频会经输入法网络层发往腾讯。

## 开发

```bash
npm ci                   # 按 package-lock.json 安装依赖（与 CI、发布一致）
npm run tauri:dev        # 开发运行：Vite(8090) + Tauri 壳 + 内嵌数据面，数据目录 = 仓库 data/
npm run tauri:build      # 本地打包
npm run prepare:sherpa   # 单独下载并校验当前平台的 sherpa-onnx 推理库（tauri:build 会自动执行）
npm test                 # = test:node（前端纯逻辑用例）+ test:rust（cargo test --workspace）
```

- 前端统一用 npm（`package.json` 的 `packageManager`），不要用 pnpm / yarn 安装：它们不读 `package-lock.json`，解析出的版本会和 CI、发布产物不一致；加减依赖用 `npm install <包>` 并一起提交 `package-lock.json`。
- `cargo` 命令要在 `src-tauri/` 目录内执行（Windows 的 `+crt-static` 配置只在该目录下生效）。
- 首次构建会编译 CTranslate2（约 10~20 分钟），macOS 需先 `brew install cmake`。
- 本地 Whisper / Qwen / SenseVoice 的模型不随仓库分发；选择对应引擎后在「设置 → 识别引擎」里应用内下载（Whisper 会自动装 Silero VAD，Qwen / SenseVoice 走 GitHub Release），模型目录默认 `models/`，可在设置页更改并重启应用生效，也可用启动前环境变量 `ATT_MODELS_DIR` 指定；两者同时存在时环境变量优先。开发态 Qwen / SenseVoice 推理库默认放 `models/sherpa-onnx/`，也可用 `ATT_SHERPA_ONNX_DIR` 覆盖；安装包使用随包的 `resources/sherpa-onnx/`。
- 开发态的 macOS 权限归属启动 `tauri dev` 的终端应用。
- 更多开发约定与常见陷阱见 [`AGENTS.md`](AGENTS.md)。

### 本地 Whisper CUDA 加速

构建机**检测到 CUDA Toolkit（CUDA_PATH / CUDA_HOME）就默认启用 CUDA 构建**（Windows / Linux）；运行时再由
「设置 → 识别引擎 → 运行设备」在 CPU / GPU 间切换，**不需要命令行开关**。没装 CUDA Toolkit 的构建机自动保持
CPU + int8；macOS 固定走 Accelerate，不支持 CUDA。

- 强制开启（即使没检测到）：ATT_CUDA=1 npm run tauri:dev / tauri:build
- 强制关闭（即使装了 CUDA Toolkit，例如出纯 CPU 包）：ATT_CUDA=0 或 ATT_CPU=1
- npm run tauri:dev、tauri:build、tauri dev/build 都走同一个启动脚本，行为一致。

- 启动脚本会把 `--features cuda` 传给 Tauri，再经 `att-core` 传到 `att-whisper`（ct2rs 0.11.3 /
  CTranslate2 4.9.1 的 `cuda` + `cuda-dynamic-loading`）。编译需要 CUDA Toolkit（实测 13.0，最低 11.0）；
  cuBLAS 在运行期按需加载，模型加载前会先检查它是否就位。
- CUDA 构建的 `whisper_device=auto` 会先尝试 GPU（float16），健康面板 / 日志显示真实生效设备：
  `cuda:float16`；没有可用 GPU、或只有驱动没有 CUDA 运行库时自动回退 `cpu:int8`，进程不会因缺 CUDA 退出，
  也不会卡在第一次转写上。
- 需要 cuDNN 卷积加速时再加 `ATT_CUDNN=1`（或直接 `ATT_CUDA=cudnn`）。该变体编译与运行都需要 cuDNN 9
  （实测 9.28.1.4；Windows 为 `cudnn64_9.dll`），且二进制启动时就会加载 cuDNN——没有安装 cuDNN 的机器无法运行，
  只给确定有 GPU + cuDNN 的机器用。
- 默认（未显式关闭）在 Windows / Linux 上按“检测到 CUDA Toolkit 即启用”处理；ATT_CUDA=0 / ATT_CPU=1 可强制回到纯 CPU。macOS 固定走 Accelerate，不支持 CUDA。
- 同时保留 CPU 与 CUDA 两套产物时建议用不同的 `CARGO_TARGET_DIR`；切换 feature 会重新编译
  CTranslate2（首次约 20–40 分钟）。RTX 50 系（sm_120）可在 NVIDIA 新卡上设 `CUDA_ARCH_LIST=12.0`
  生成原生代码，不设时按 CT2 的常见架构 + PTX 由驱动 JIT。
- Windows 报 `No CUDA toolset found` 时，说明 CUDA Toolkit 没装 Visual Studio 集成：重跑 CUDA 安装器勾选
  Visual Studio Integration，或改用 Ninja 生成器（`CMAKE_GENERATOR=Ninja`，并保证 C/C++ 与 Rust 都用静态 CRT）。

可选环境变量（应用不读 `.env` 文件，需在启动前设置）：

| 变量 | 作用 |
| --- | --- |
| `ATT_DATA_DIR` | 数据目录（默认：开发态为仓库 `data/`，安装版为系统应用数据目录） |
| `ATT_MODELS_DIR` / `ATT_SHERPA_ONNX_DIR` | 本地模型根目录（优先于设置页，重启生效） / sherpa-onnx 动态库目录 |
| `ATT_SILERO_VAD` | Silero VAD 模型文件（默认在模型目录里找 `silero_vad.onnx`） |
| `ATT_CUDA` | 构建时开关：默认在 Windows/Linux 检测到 CUDA Toolkit 即启用；`1` 强制启用，`0`/`off` 强制关闭；`cudnn` = CUDA + cuDNN |
| `ATT_CUDNN` | 构建时开关：`1` 在 CUDA 构建上追加 cuDNN（运行机器需安装 cuDNN 9） |
| `ATT_CPU` | 构建时开关：`1` 强制纯 CPU 构建（忽略默认的 CUDA 自动启用） |
| `ATT_WHISPER_COMPUTE_TYPE` | Whisper 计算精度（默认 `auto`：CUDA 构建用 float16，CPU 用 int8） |
| `ATT_QWEN_THREADS` | Qwen 推理线程数（默认 2） |
| `ATT_FFMPEG_DIR` | ffmpeg 所在目录（文件转写；默认查 PATH 与 Homebrew 常见路径） |
| `ATT_PROMPTS_DIR` | 内置提示词目录 |
| `ATT_BACKEND_PORT` | 数据面优先端口（默认 17890；被占用时依次 +1 重试，上限内都不行才随机） |
| `ATT_UPDATE_URL` | 检查更新的地址 |
| `ATT_FORCE_MIGRATION=1` | 重跑一次性数据迁移 |
| `DOUBAO_ASR_API_KEY` | 豆包密钥兜底（设置页填写的优先） |

数据面令牌由应用每次启动随机生成并注入前端；除 `/api/health` 外所有接口都要求 `X-ATT-Token`，
录音回放 / 图片 / 导出等 `<audio>`、`<img>` 无法带请求头的地址改用 `?token=`。`/api/transcribe`
以令牌作为唯一授权：携带有效令牌即可转写任意可读的本地音视频文件，不再要求文件在界面里登记过
或位于配置的目录内；令牌缺失或错误仍返回 401。不带令牌时 `/api/health` 只返回探活字段；
带有效令牌时才返回引擎、模型路径等完整运行态。界面里的文件白名单只用于防误操作，不是 HTTP 门槛。

外部进程（例如 SourceFlow）可以在「设置 → 诊断 → 外部 API」开启**持久访问令牌**：首次开启时生成
一个随机令牌并加密保存在数据目录，应用重启后保持不变；界面默认脱敏，点「显示 / 复制」才会读取明文，
令牌不会写进日志或 `GET /api/settings`。持久令牌仅可调用 `POST /api/transcribe`、
`GET /api/transcribe/jobs/{id}` 和 `POST /api/transcribe/jobs/{id}/cancel`，访问记录、设置或 LLM
配置等其他接口会返回 403。诊断页可用「重新生成」立即吊销旧令牌并生成新令牌。该能力只对回环地址开放，
非回环请求仍返回 403。外部调用流程：
`POST /api/transcribe`（`X-ATT-Token` 用持久令牌，`path` 为任意可读的本地文件，`want_segments: true`
时每个 segment 带秒为单位的 `start` / `end`）拿到 `job_id` 后轮询 `GET /api/transcribe/jobs/{id}`。

发布：`scripts/prepare-sherpa-onnx.mjs` 会按目标平台下载 sherpa-onnx v2.0.1、校验固定 SHA-256 后放进安装包资源；
版本号写在 `package.json`、`src-tauri/Cargo.toml`、`src-tauri/tauri.conf.json`（及对应 lock 文件），
用 `npm run version:bump -- 2.8.0` 一次改齐（`node scripts/bump-version.mjs --check` 只校验，Node 测试里也会校验）；
推送 `v*` 注释标签后，`.github/workflows/release.yml` 自动打包 macOS / Windows 安装包并发布 Release，
标签注释正文即版本说明。

应用内更新：发布时用仓库 Secrets 里的 `TAURI_SIGNING_PRIVATE_KEY` / `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`
给更新包签名，并生成 `latest.json`；已安装的应用据此下载、校验签名后「重启并更新」。公钥在 `tauri.conf.json`
的 `plugins.updater.pubkey`，换密钥后旧版本将无法校验新版本的更新包，只能手动下载安装包更新。
本地 `npm run tauri build` 同样需要这两个环境变量；不需要更新包时加
`--config '{"bundle":{"createUpdaterArtifacts":false}}'`。
