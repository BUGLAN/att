# 需求文档：Docker 化实时 ASR（一键启动 + 卷分离持久化）

| 项 | 内容 |
| --- | --- |
| 文档状态 | **v1.0：已实现并验证**（决策全部冻结，代码已落地，验证记录见 §12） |
| 创建日期 | 2026-10-01 |
| 版本记录 | v0.1 初稿；v0.2 锁定 D2/D3/D6/D8；v1.0 冻结 D1/D4/D5/D7/D9/D10 并完成实现 |
| 需求提出人 | 项目所有者 |
| 需求一句话描述 | 把实时流式 ASR 后端 Docker 化，做到一键启动、稳定输出实时 ASR，运行期产物与镜像分离并落盘到本地 |
| 影响模块 | `app/`（FastAPI 后端）、`scripts/qwen_streaming_service.py`（流式桥接服务）、`electron/python/`（后端拉起与复用）、根目录运行期目录 `data/`、`models/`、`logs/` |
| 当前实现状态 | 仓库内**无任何 Dockerfile / compose / .dockerignore**，本需求为全新建设 |

---

## 1. 背景与问题

当前应用形态是「Electron 桌面壳 + Vue 渲染层 + FastAPI 后端 + 本地 Python 进程」的一体化桌面程序，后端的生命周期由 Electron 管理（`electron/python/manager.js` 通过 `spawn` 拉起本机 Python）。

实时流式 ASR 依赖 Qwen3-ASR + vLLM。当前存在三个具体问题：

1. **Windows 原生被代码显式禁止本地 vLLM 真流式**：`app/config.py:67` 的 `supports_local_vllm_streaming()` 返回 `not is_windows_host()`，Windows 宿主上直接抛错，只能退化为 9000 端口的桥接子进程方案。
2. **运行时依赖与宿主强耦合**：CUDA / torch / vLLM / 驱动版本必须与显卡匹配；换机、重装、分发都要重新踩一遍环境。
3. **运行期产物与应用生命周期强绑定**：录音、记录、会话、配置、加密密钥、模型缓存都写在程序工作目录内，重装应用、迁移目录或清理工作区即丢数据；且这些产物目前完全不受「容器可写层」这类边界保护。

本需求要解决的是「**部署形态**」与「**数据归属**」两件事，**不涉及实时流式算法本身的改造**。

---

## 2. 目标与非目标

### 2.1 目标

| 编号 | 目标 | 说明 |
| --- | --- | --- |
| G1 | **一键启动** | 用户执行单一入口（脚本或命令），自动完成前置检查、镜像准备、容器启动、模型预热，最终到达「可直接录音」状态，无需手工敲多条命令、无需手工改配置 |
| G2 | **稳定实时** | 流式 partial 文本持续输出，端到端延迟可控且有量化上限；连续多次录音不崩、不丢会话、不积压（RTF 小于 1） |
| G3 | **卷分离** | 一切运行期产生的文件（数据、模型、日志、密钥）都不留在容器可写层，全部映射到宿主本地目录或命名卷，容器删除/重建/升级后数据零丢失 |
| G4 | **可迁移** | 拷贝卷目录即等价于完整备份；换机器时只需重建镜像 + 挂回卷 |

### 2.2 非目标（本期明确不做）

- 不改动实时流式的分块算法与模型选型（仍为 Qwen chunk 流式）。
- 不把 Electron 桌面端容器化：全局热键、录音静音（`electron/python/desktop_agent.py`，依赖 Windows WASAPI）、托盘、悬浮窗必须留在宿主机。
- 不做公网暴露、多用户、鉴权、反向代理。
- 不提供 macOS / Linux 宿主的一键启动（本期只针对 Windows 宿主 + WSL2）。
- 不改前端交互与 UI 样式。
- 不做容器编排集群化（K8s 等）。

---

## 3. 术语定义（可量化，避免歧义）

| 术语 | 定义 |
| --- | --- |
| **一键启动** | 用户仅执行一个入口命令/双击一个脚本，中途除首次模型下载外无任何交互，最终 `/api/health` 返回流式可用 |
| **冷启动** | 宿主重启后，Docker 守护进程与容器均未运行的状态下发起一键启动；模型已缓存 |
| **首次启动** | 模型缓存为空，需要下载模型的启动 |
| **就绪（ready）** | `/api/health` 中流式字段为可用，且流式桥接服务 `/api/health` 返回 `ready=true && loaded=true` |
| **实时** | 以「说话时刻 → partial 文本出现」的端到端延迟度量，见 §6.1 延迟预算 |
| **稳定** | 见 §8 验收标准 AC-4：连续多次流式录音成功率 100%，无容器重启、无会话泄漏、无未捕获异常 |
| **RTF** | 单个音频块推理耗时 ÷ 该块的音频时长；RTF < 1 才可能不积压 |
| **卷分离** | 容器可写层不含任何业务数据；所有运行期产物通过 volume / bind mount 落到宿主 |
| **partial / final** | `stream_partial` 为中间态文本（可被修正），`stream_finished` 为本次录音最终文本（不可再变） |

---

## 4. 现状约束（技术事实，需求必须遵守）

| 编号 | 约束 | 代码依据 | 对需求的影响 |
| --- | --- | --- | --- |
| C1 | Windows 宿主禁止进程内 local_vllm 流式 | `app/config.py:67-69` | Linux 容器内 `is_windows_host()` 为 false，该禁令自动解除；**Docker 化反而是开启本地真流式的路径** |
| C2 | 桥接服务监听的 host/port 为硬编码 `127.0.0.1:9000` | `app/services/qwen_streaming_service_manager.py:25-27` | 若拆成两个容器，**不能**用「托管子进程」模式，必须走 `remote_bridge`（`QWEN_STREAMING_BASE_URL`） |
| C3 | 桥接服务通过 `sys.executable -m uvicorn scripts.qwen_streaming_service:app` 以子进程方式拉起 | 同上，`:162-172` | 单容器方案下该逻辑可原样工作；容器内走 POSIX 分支（killpg），Windows 专有的 PowerShell / taskkill 清理分支不会命中 |
| C4 | 桥接服务读取 `QWEN_STREAMING_CHUNK_SIZE_SEC`、`QWEN_STREAMING_UNFIXED_*`、`QWEN_STREAMING_CACHE_DIR` 等环境变量 | `scripts/qwen_streaming_service.py:33-45` | 分块与延迟可通过环境变量调优，**但只在桥接路径生效** |
| C5 | 主进程内嵌 local_vllm 路径**未接**分块配置：`StreamingSessionManager` 只透传 model/device/max_new_tokens/backend/base_url/timeout | `app/services/streaming_session_manager.py:64-72`、`app/stt/factory.py:50-68` | 内嵌路径锁死默认 `chunk_size_sec=2.0 / unfixed_chunk_num=2`；要压延迟需走容器化的 remote_bridge，或补齐配置透传 |
| C6 | 前端以 16kHz 单声道 PCM、每 3200 样本（200ms）一包经 WebSocket 推送 | `src/composables/useRecorder.js:12-13` | 容器不需要声卡/音频设备，音频采集留在渲染进程 |
| C7 | Electron 启动后端前会先健康检查 `127.0.0.1:${ATT_BACKEND_PORT\|PORT\|8000}`，健康则复用为 `externalBackend` | `electron/python/manager.js:31-48, 240-259` | 容器化后端只需发布端口 + 设置 `ATT_REQUIRE_EXISTING_BACKEND=1`，Electron 侧几乎零改动 |
| C8 | 流式会话音频以 WAV 落盘并注册为记录 | `app/services/streaming_session_manager.py:91-92, 170-175` | `data/` 卷必须可写且权限正确，否则录音成功但入库失败 |
| C9 | LLM API Key 使用 Fernet 加密，密钥文件默认 `data/.encryption_key` | `app/utils/encryption.py:27-31` | **`.encryption_key` 必须持久化**，丢失则已保存的 API Key 无法解密 |
| C10 | 运行期目录固定为相对工作目录的 `data/`、`models/`、`logs/` | `app/config.py:276-356`、`app/utils/logging.py:18`、`app/services/qwen_streaming_service_manager.py:35` | 容器内统一以固定工作目录（建议 `/app`）为根，卷挂载点据此确定 |
| C11 | 模型缓存分为 `models/qwen`（流式/ASR）与 `models/faster_whisper`（Whisper） | `app/config.py:331-338`、`app/stt/factory.py:48,54` | 卷设计需同时覆盖两条 ASR 链路 |
| C12 | 宿主为 RTX 5090（32GB）+ 驱动 610.88，属 Blackwell sm_120 | 实测 `nvidia-smi` | 镜像必须使用 CUDA 12.8+ 的 torch/vLLM（cu128）；旧镜像会因缺少 sm_120 kernel 而失败 |
| C13 | 宿主已安装 WSL2 与 Ubuntu 发行版，`docker-desktop` 发行版存在但**当前为 Stopped** | 实测 `wsl -l -v`、`docker version` | 一键启动必须先自愈「Docker 未运行」这一前置状态 |
| C14 | **模型服务状态探测固定指向本地 `127.0.0.1:9000`**：只要录音模式为 streaming，运行能力恒为 `managed_streaming_bridge`，模型服务管理器会去拉起**本地** 9000 子进程，健康状态也只探测本地 | `app/stt/providers/qwen_asr_provider.py:136-157`、`app/services/model_service_manager.py:60-72`、`app/services/model_service_manager.py:101-118` | **双容器方案的硬冲突**：后端容器会额外拉起一个无用的本地 ASR 子进程（重复抢显存/加载失败），且远端 ASR 即使健康也会被误报为「不可用」。这是 D2 选定后必须一并处理的改造点（见 FR-13） |
| C15 | `.gitignore` 已忽略 `data/`、`models/`、`logs/`、`tmp/`、`.env` | `.gitignore:67`、`:74`、`:78` | 沿用仓库内卷**不会误提交**；但这些目录处于 ignored 状态，`git clean -xfd` 会把它们整体删除，误删风险真实存在（见 R12） |

---

## 5. 功能需求（FR）

### FR-1 镜像与构建物

- FR-1.1 提供后端镜像定义（Dockerfile）与忽略规则（`.dockerignore`），构建上下文不得包含 `.venv`、`node_modules`、`data/`、`models/`、`logs/`、`dist/`、`.worktrees/`、`.git/`。
- FR-1.2 镜像需内置 Python 运行时、FastAPI 后端依赖，以及 GPU 版 torch/vLLM（CUDA 12.8+，支持 sm_120）。
- FR-1.3 镜像构建产物可复用（分层缓存），重复构建在依赖未变时不应重新下载全部依赖。
- FR-1.4 容器内工作目录固定（建议 `/app`），使 `data`、`models`、`logs` 的相对路径可预测。

### FR-2 一键启动入口

- FR-2.1 提供**根目录 `.bat` / `.ps1` 双击入口**（D6 已定），执行后完成：前置检查 → 启动 Docker 守护 → 构建/复用镜像 → 拉起两个容器（后端 + ASR 服务）→ 等待就绪 → 预热模型 →（可选）启动 Electron。内部复用 `docker compose`，但用户不需要手敲 docker 命令。
- FR-2.2 启动过程必须有**分段进度输出**（如 `[1/6] 检查 Docker…`、`[5/6] 等待模型预热…`），不得静默长时间无输出。
- FR-2.3 入口需具备**幂等性**：重复执行不产生重复容器、不报端口占用、第二次起走「复用已有实例」分支。
- FR-2.4 任一环节失败时，入口以非零退出码结束，并打印**可执行的下一步建议**（如「Docker Desktop 未启动，请先启动或允许脚本自动启动」）。
- FR-2.5 默认启动路径不得要求用户手工执行 `docker` 相关命令。

### FR-3 前置检查与自愈

- FR-3.1 检查 Docker 守护进程可达；不可达时按 D1 选定的策略自动启动或明确提示。
- FR-3.2 检查 GPU 可用（`nvidia-smi` 与容器内 `--gpus all` 均可见），不可用时给出明确错误，不得静默降级。
- FR-3.3 检查宿主端口占用（后端默认 8000，桥接 9000）；被非本项目进程占用时，自动改选可用端口并把实际端口传递给 Electron 与前端。
- FR-3.4 检查宿主可用内存与 WSL2 内存上限是否满足模型加载需求，不满足时给出提示。
- FR-3.5 检查并清理历史遗留进程/容器（含 vLLM `EngineCore` 残留），避免显存被旧实例占满。

### FR-4 就绪判定与模型预热

- FR-4.1 容器启动后，入口必须轮询 `/api/health` 直至流式字段为可用，而不是「容器起来了」就算成功。
- FR-4.2 预热应主动触发模型加载（等价于桥接服务 `/api/preload`），使**首次录音不再承担模型加载延迟**。
- FR-4.3 就绪超时需可配置，且需覆盖**模型加载耗时**（D3 下不再有下载环节，但 virtiofs 加载可能偏慢，见 §7 权衡说明）；失败时输出已等待时长与最后一帧健康信息。
- FR-4.4 就绪状态需在 UI/日志中可见（加载中 / 已就绪 / 失败原因）。

### FR-5 停止、状态与日志

- FR-5.1 提供一键停止入口，停止后容器退出且无残留子进程、无残留 PID 文件、显存释放。
- FR-5.2 提供状态查询（容器状态、健康状态、模型是否已加载、当前监听端口）。
- FR-5.3 提供日志查看路径：应用日志与桥接服务日志（`logs/qwen_streaming_service.log`）都能从宿主直接读取。

### FR-6 客户端对接

- FR-6.1 Electron 能够连接容器内后端而不自行再拉起本地 Python 后端（复用 C7 的既有能力）。
- FR-6.2 后端实际端口变化时，Electron 与渲染层拿到的端口必须同步更新（不得写死 8000）。
- FR-6.3 桌面专有能力（全局热键、录音静音、托盘、剪贴板悬浮窗、选区工具栏）继续在宿主 Windows 侧工作，功能不退化。
- FR-6.4 流式录音链路（`WS /api/asr/stream`）需端到端可用：`stream_start` → 二进制 PCM 流 → `stream_partial` → `stream_finish` → `stream_finished` 并生成记录。

### FR-7 卷分离与持久化（本需求核心）

- FR-7.1 容器可写层不得存放业务数据；以下内容必须外置：业务数据、模型缓存、日志、密钥、配置。
- FR-7.2 挂载清单见 §7，每一项需明确持久化级别（必须 / 建议 / 不需要）。
- FR-7.3 卷目录的物理位置（**D8 已定：仓库内 `data/`、`models/`、`logs/`**）需在启动时以绝对路径打印，保证可发现、可读写、可备份。
- FR-7.4 容器内进程对挂载目录需具备写权限（非 root 运行时的 UID/GID 映射需明确），避免「能启动但写不进 `data/`」。
- FR-7.5 容器重建、镜像升级、`compose down` 之后重新启动，业务数据与加密密钥必须完整保留。
- FR-7.6 不得为了「干净」而在启动脚本中无差别清空 `data/`；任何清理动作必须显式、可确认。

### FR-8 首次初始化与模型获取

- FR-8.1 **模型来源（D3 已定）**：直接挂载宿主仓库内已有的 `models/qwen` 缓存，不重新下载、不烘焙进镜像。
- FR-8.2 需验证容器内可正确解析宿主挂载来的本地模型目录（存在 `config.json` 即视为可用），并验证 ASR 容器对该目录**有读权限**。
- FR-8.3 若挂载目录中模型缺失，启动必须失败并给出明确提示（预期位置、缺失文件）；**是否提供自动下载作为兜底由 D3 的后续确认决定**，不得静默开始长时间下载。
- FR-8.4 首次启动与常规启动走同一入口，不要求用户执行额外命令。

### FR-9 配置注入

- FR-9.1 关键配置（provider、模型名、显存占用比例、最大上下文、分块参数、超时）通过 `.env` 注入，不硬编码进镜像。
- FR-9.2 `.env` 以只读方式挂载进容器；宿主 `.env` 需可继续被 Electron 侧读取。
- FR-9.3 容器化后默认 provider 应为 Qwen 流式链路，而非 `.env.example` 当前的 `whisper`（默认值需在实现阶段显式确认）。
- FR-9.4 提供 `.env.example` 的补充项：`QWEN_STREAMING_CHUNK_SIZE_SEC`、`QWEN_STREAMING_UNFIXED_CHUNK_NUM`、`QWEN_STREAMING_UNFIXED_TOKEN_NUM`、`QWEN_STREAMING_CACHE_DIR`。

### FR-10 升级与回滚

- FR-10.1 镜像升级后数据无需迁移（依赖卷分离）。
- FR-10.2 提供回滚到上一可用镜像版本的方式（保留旧镜像标签或记录版本）。
- FR-10.3 升级前后 `/api/health` 与一次端到端流式录音均需可验证。

### FR-11 失败可诊断

- FR-11.1 所有失败场景（Docker 未启动、GPU 不可见、`/dev/shm` 过小、模型加载失败、端口占用、显存不足）需有**专属错误文案**，不得共用同一句泛化报错。
- FR-11.2 桥接子进程崩溃时，宿主侧日志需包含其 stdout/stderr 落盘内容。
- FR-11.3 不允许出现「容器显示 running 但流式不可用」的静默状态——就绪判定必须依据 FR-4.1。

### FR-12 降级路径（可选，按 D7 决定）

- FR-12.1 若约定支持无 GPU 环境，需明确降级方案（CPU 流式不可用 → 退回 `single`/`multi` 模式，或走云端 ASR），且启动入口需主动检测并告知用户当前处于降级模式。
- FR-12.2 降级不得表现为「假装流式」的静默行为。

### FR-13 双容器编排与状态打通（D2 选定后的新增必做项）

> D2 选择「双容器：后端容器 + ASR 服务容器」后，C14 描述的硬冲突必须一并解决，否则会出现「远端 ASR 健康但 UI 报不可用」以及「后端容器偷偷再加载一份模型抢显存」两个问题。

- FR-13.1 编排需包含两个服务：`backend`（FastAPI，发布端口到宿主）与 `asr`（Qwen 流式服务，**不发布端口**，仅容器网络内可达）。
- FR-13.2 后端必须以 `QWEN_STREAMING_BACKEND=remote_bridge` + `QWEN_STREAMING_BASE_URL=http://asr:9000` 指向 ASR 容器（C2：桥接服务的 `127.0.0.1:9000` 为硬编码，双容器下只能走 remote_bridge）。
- FR-13.3 **禁止后端容器再拉起本地 ASR 子进程**：需改造模型服务的启动与状态探测，使其在 remote_bridge 模式下探测远端健康、且不执行本地 `qwen_streaming_service_manager.start()`（对应 C14）。
- FR-13.4 流式可用性判定需以**远端 ASR 容器**的健康为准（`ready && loaded`），不得以本地 9000 端口为准。
- FR-13.5 ASR 容器需启动依赖顺序与健康检查：后端启动时若 ASR 尚未就绪，应表现为「加载中」而非「不可用」；ASR 就绪后状态需自动转为可用。
- FR-13.6 ASR 容器不可用时，后端需给出明确错误（含容器名与排查提示），不得静默等待或误报成功。
- FR-13.7 两个容器共享同一份 `models/` 挂载（ASR 必需；后端在 single/multi 模式下走 Whisper 时同样需要）；`data/` 与 `logs/` 仅后端需要挂载。

---

## 6. 非功能需求（NFR）

### 6.1 性能与延迟预算

| 环节 | 目标值 | 说明 |
| --- | --- | --- |
| 前端攒包 | 200ms（固定） | 3200 样本 @16k，属于既有实现，本期不改 |
| 分块算法延迟 | 目标 ≤ 2s（可调至 1s，按 D5） | 由 `chunk_size_sec` + `unfixed_chunk_num` 决定 |
| 单块推理（RTF） | **RTF ≤ 0.5**（硬指标 RTF < 1） | 低于 1 才能保证不积压 |
| 端到端 partial 延迟 | **P95 ≤ 3.0s**（验收口径见 AC-3） | 「说完一句」到 partial 文本可见 |
| 容器网络额外开销 | ≤ 50ms | Docker 端口转发 + 网络栈，属噪声级 |
| 冷启动到就绪 | **≤ 90s**（模型已缓存） | 见 AC-1 |
| 首次启动到就绪 | ≤ 5 分钟（仅 D3 兜底下载时才放宽到 30 分钟） | D3 已定挂载宿主缓存，正常无下载环节 |

> 说明：Docker 化本身不构成实时性风险；延迟主要由分块参数与单块推理速度决定。因此 FR 中必须保证分块参数**可配置**（C4/C5）。

### 6.2 稳定性

- NFR-2.1 连续 30 次流式录音（每次 15–60s）成功率 100%。
- NFR-2.2 连续运行 8 小时无容器重启、无 OOM、无显存增长趋势。
- NFR-2.3 WebSocket 连接在录音过程中不得非预期断开；断开时有明确错误上报而非静默卡住。
- NFR-2.4 会话资源必须释放：结束时 WAV 写入句柄关闭、临时音频清理、`data/sessions` 无残留 zombie 会话。
- NFR-2.5 单次录音失败不得导致容器退出或影响后续录音。

### 6.3 资源占用

- NFR-3.1 显存占用可控且可配置（`QWEN_STREAMING_GPU_MEMORY_UTILIZATION`），且需与宿主其他 GPU 使用者并存（宿主桌面本身占用显存）。
- NFR-3.2 空闲时不得持续占用 CPU（不得空转轮询）。
- NFR-3.3 容器内存上限需显式声明，避免 WSL2 内存被逐步吃满。

### 6.4 兼容性

- NFR-4.1 仅承诺 Windows 11 + WSL2 + Docker Desktop 组合（按 D1 确认）。
- NFR-4.2 必须支持 RTX 5090（sm_120）：CUDA 12.8+、torch/vLLM 对应版本。
- NFR-4.3 容器内 `platform.system()` 为 Linux，`supports_local_vllm_streaming()` 需返回 True（C1 生效）。
- NFR-4.4 不得依赖容器内的 Windows 专有命令（`taskkill`、`powershell.exe`）。

### 6.5 安全与隐私

- NFR-5.1 `data/.encryption_key` 必须持久化且不得进入镜像层（C9）；一旦丢失，已保存的 LLM API Key 全部不可用。
- NFR-5.2 后端与桥接端口只监听 127.0.0.1，不对外网暴露；桥接端口 9000 不发布到宿主（若拆容器则仅容器网络内可达）。
- NFR-5.3 `.env` 与密钥文件不得被 `COPY` 进镜像；`.dockerignore` 必须覆盖。
- NFR-5.4 录音音频属敏感数据。D8 选择沿用仓库内目录后，隔离责任由 `.gitignore` 承担：**已核验** `data/`、`models/`、`logs/` 当前**零个被跟踪文件**（`git ls-files` 结果仅含 `prompts/*.txt`），误提交风险已消除；但仍必须防护 `git clean -xfd` 之类会删除 ignored 目录的操作（见 FR-7-V3 与 R12）。

### 6.6 可观测性

- NFR-6.1 应用日志与桥接服务日志写入 `logs/`（已外置为卷），宿主机可直接查看。
- NFR-6.2 健康接口需反映：模型是否加载、当前 provider、流式 backend 类型、桥接服务状态。
- NFR-6.3 关键指标（就绪耗时、单块推理耗时、partial 延迟）需可获取，用于验证 RTF 与 P95。

### 6.7 幂等性与可重复性

- NFR-7.1 启动/停止/重建脚本可反复执行，结果一致。
- NFR-7.2 全流程不依赖「先手动做一步」的隐含前置条件。

---

## 7. 卷与持久化清单（核心交付约定）

容器内统一以 `/app` 为工作目录，路径与代码中的相对路径一一对应（C10）。**卷物理位置（D8 已定）：沿用仓库内目录**，即 `E:\repo\att\data`、`E:\repo\att\models`、`E:\repo\att\logs`。

| 宿主路径（D8 已定） | 容器路径 | 内容 | 持久化级别 | 挂载方式 | 备注 |
| --- | --- | --- | --- | --- | --- |
| `E:\repo\att\data` | `/app/data` | `settings.json`、`llm_configs.json`、`.encryption_key`、`records/`、`sessions/`、`audio/{temp,records,permanent}`、`clipboard/`、`wordbook/`、`prompts/` | **必须** | bind mount | 含加密密钥，丢失代价最高；容器进程必须可写 |
| `E:\repo\att\models` | `/app/models` | `qwen/`（Qwen3-ASR 与流式缓存）、`faster_whisper/` | **必须** | bind mount | 宿主已有缓存，直接复用（D3）；**注意 virtiofs 加载变慢**，见下方权衡说明 |
| `E:\repo\att\logs` | `/app/logs` | `app.log`、`qwen_streaming_service.log` | 建议 | bind mount | 便于宿主直接排查 |
| `E:\repo\att\.env` | `/app/.env` | 运行配置 | **必须** | bind mount，**只读** | 不 `COPY` 进镜像 |
| `E:\repo\att\prompts` | `/app/prompts` | 内置提示词模板（只读） | 不需要（镜像内容） | 不挂载 | 属程序资源，非运行期数据 |
| `E:\repo\att\tmp\uv-cache` | — | uv 缓存 | 不需要 | 不挂载 | 仅开发期 |
| — | `/dev/shm` | vLLM 共享内存 | **必须扩容** | `--shm-size=8g` 或 `--ipc=host` | 默认 64MB 会导致 vLLM 崩溃 |

> **D8 的已接受权衡（必须记录在案）**：模型目录位于 Windows 盘并通过 bind mount 进入容器，读写走 virtiofs，**模型加载会明显慢于原生或命名卷**。这会直接冲击 AC-1 的「≤ 90s 冷启动到就绪」目标。
> 处理约定：实现阶段必须实测该耗时；若超过 90s，启用备选方案——`models/` 改用命名卷，首次通过一次性导入脚本从仓库目录灌入，其余卷仍按 D8 落在仓库内。**不因此改变 D8 对其他目录的决定。**

补充约定：

- FR-7-V1（D8 已定）卷物理位置为仓库内目录；必须保证「拷走 `data` + `models` + `.env` = 完整备份」。启动脚本需在启动时**打印实际挂载的宿主绝对路径**，避免用户找不到数据在哪。
- FR-7-V2 启动脚本需在首次运行时**创建**卷目录结构，而不是让容器以 root 创建后造成宿主权限错乱。
- FR-7-V3 文档中需明确警示：`data/`、`models/` 均处于 `.gitignore` 忽略状态（C15），因此 **`git clean -xfd` 会直接删除全部业务数据与模型缓存**，排查问题时禁止使用该命令。
- FR-7-V4 需明确「数据目录已存在但格式不符」时的处理策略：拒绝启动并提示，不做静默覆盖。

---

## 8. 验收标准（逐条可执行）

| 编号 | 验收项 | 判定方式 |
| --- | --- | --- |
| AC-1 | 冷启动一键到达就绪 | 宿主重启后执行单一入口，**≤ 90s** 内 `/api/health` 显示流式可用；中途无手工命令 |
| AC-2 | 启动幂等 | 连续执行启动入口 3 次，无重复容器、无端口冲突，第 2、3 次走复用分支并成功 |
| AC-3 | 实时性 | 连续朗读 60s 文本，partial 文本相对说话时刻的延迟 **P95 ≤ 3.0s**；实测 RTF ≤ 0.5；音频样本数守恒（前端发送样本数 = 后端 `sample_count`） |
| AC-4 | 稳定性 | 连续 30 次流式录音（15–60s）成功率 100%，容器无重启，WS 无异常断开，无残留会话与临时音频 |
| AC-5 | 数据持久化 | `compose down` 后重新启动：记录、剪贴板、设置、词库全部可见；已保存的 LLM API Key 仍可正常调用（验证 `.encryption_key` 保留） |
| AC-6 | 卷分离验证 | 删除容器与镜像后重建并启动，业务数据与模型缓存仍在，无需重新下载模型 |
| AC-7 | 失败可诊断 | 分别模拟「Docker 未启动」「GPU 不可见」「`/dev/shm` 过小」「端口被占用」，四种场景均在 ≤ 60s 内给出**专属**错误文案与下一步建议，且退出码非零 |
| AC-8 | 无残留 | 执行停止入口后，无遗留容器、无 vLLM `EngineCore` 残留进程、显存回落到启动前水平 |
| AC-9 | 桌面能力不退化 | 全局热键录音、录音静音、托盘、剪贴板悬浮窗、选区工具栏在容器化后端下全部正常工作 |
| AC-10 | 全链路可用 | 一次完整流程：按热键 → 流式 partial 输出 → 结束 → 生成记录 → 音频可回放（`GET /api/records/{id}/audio`） |
| AC-11 | 模型缺失场景可诊断 | 临时移走 `models/qwen` 后一键启动，必须 ≤ 60s 内失败并打印预期路径与缺失文件，**不得**静默开始长时间下载；若启用兜底下载，则需有进度输出且可重试 |
| AC-12 | 升级保留数据 | 重新构建镜像并启动后，AC-5 数据完整、AC-10 全链路通过 |
| AC-13 | 双容器状态正确性（对应 FR-13） | ASR 容器健康时接口与 UI 显示流式可用；手动停止 ASR 容器后 ≤ 30s 内转为「不可用」并给出明确原因；重新启动后自动恢复可用。同时验证后端容器内**不存在**监听 9000 的本地子进程（证明 C14 已解决） |
| AC-14 | 卷路径可见与防护 | 启动输出打印 `data`、`models`、`logs` 三个宿主绝对路径；一次完整流程后 `git status` 无新增未跟踪的业务数据文件；启动脚本与文档中含 `git clean -xfd` 警示 |
| AC-15 | D8 权衡实测 | 记录模型加载耗时；若冷启动到就绪 > 90s，说明已按 §7 权衡说明启用命名卷备选方案 |

> AC-3 的测量口径需在实现阶段固化为一个可重复执行的基准步骤（含播报内容、计时方式、采样方式），以便回归对比。

---

## 9. 决策记录

### 9.1 已确认决策（2026-10-01，需求方拍板）

| 编号 | 决策点 | **已确认结论** | 连带影响 |
| --- | --- | --- | --- |
| **D2** | 镜像边界 | **双容器：后端容器 + ASR 服务容器**（`remote_bridge`） | 触发 **C14 硬冲突**，新增 FR-13 必做项；分块参数从此走 env 可调，规避 C5 |
| **D3** | 模型来源 | **挂载宿主仓库内已有的 `models/qwen`** | 不烘焙、不下载；镜像体积小；但引入 virtiofs 加载耗时问题（见 §7 权衡说明） |
| **D6** | 一键启动入口 | **根目录 `.bat` / `.ps1` 双击脚本** | 内部复用 `docker compose`；需处理执行策略、控制台输出与错误码 |
| **D8** | 卷物理位置 | **沿用仓库内 `data/` 与 `models/`** | 迁移成本为零；已核验无 git 跟踪文件；但需防护 `git clean -xfd`（R12） |

### 9.2 已冻结决策（2026-10-01，按建议默认值执行）

| 编号 | 决策点 | **已确认结论** | 说明 |
| --- | --- | --- | --- |
| D1 | Docker 运行形态 | **Docker Desktop（WSL2 后端）** | 启动脚本可自动拉起 Docker Desktop 并等待就绪（C13） |
| D4 | 模型档位 | **Qwen3-ASR-1.7B** | 5090 32GB 余量充足 |
| D5 | 分块参数 | **保留上游默认 `chunk_size_sec=2.0`** | 与建议的「1.0 起步实测」有偏差，理由见 §12 偏差 2：未经验证直接调小存在质量回归风险，已提供基准脚本支持量化后再调 |
| D7 | 无 GPU 降级 | **不需要** | GPU 不可用时启动直接失败并给出明确排查建议 |
| D9 | 容器外原生运行 | **保留** | 原生 `npm run backend:dev:uv` 与 `npm run qwen:streaming:uv` 不受影响 |
| D10 | 分发形态 | **暂按仅自用** | 后续如需分发，再考虑烘焙模型与免 Docker 方案 |

---

## 10. 风险清单

| 编号 | 风险 | 影响 | 缓解方向 |
| --- | --- | --- | --- |
| R1 | RTX 5090（sm_120）与镜像内 torch/vLLM 版本不匹配 | 容器起来但推理报 kernel 不可用 | 锁定 CUDA 12.8+ 基础镜像并在 AC 中显式验证 |
| R2 | `/dev/shm` 默认 64MB | vLLM 启动即崩 | 强制 `--shm-size`，写入启动脚本默认值 |
| R3 | 模型缓存走 Windows 盘 bind mount（virtiofs） | 加载慢，可能撞就绪超时 | **D8 已接受该权衡**：AC-15 实测加载耗时；超标则仅把 `models/` 单独改命名卷（§7 权衡说明） |
| R4 | 宿主 8000/9000 端口被占 | 启动失败或连错后端 | FR-3.3 自动改端口并同步给 Electron |
| R5 | `.encryption_key` 丢失 | 已保存 API Key 永久不可用 | 强制持久化 + 启动时检测告警 |
| R6 | 桥接服务硬编码 `127.0.0.1:9000` | 双容器方案无法复用托管模式 | **已确认要走 `remote_bridge`（D2）**，对应 FR-13.2；或在实现阶段把 host 参数化 |
| R7 | 分块配置未透传到内嵌路径（C5） | 延迟无法下调 | 采用双容器 + `remote_bridge` 后由 env 控制（D5），或补齐配置透传 |
| R8 | WSL2 内存/显存被旧实例占用 | 加载失败 | FR-3.5 启动前清理残留 `EngineCore` |
| R9 | 容器 root 写入宿主卷导致权限错乱 | 宿主无法读写数据 | 明确 UID/GID 与非 root 运行策略（FR-7.4） |
| R10 | 镜像体积膨胀 | 构建/分发慢 | `.dockerignore` + 多阶段构建 + 模型外置 |
| R11 | 容器化后端与现有打包流程冲突 | 分发形态需重构 | D10 先定分发目标，再决定是否改打包 |
| R12 | `git clean -xfd` 或清理脚本删除 ignored 目录 | 因 D8 落在仓库内，`data/` 与 `models/` 会被整体删除 | FR-7-V3 显式警示；文档说明数据物理位置；建议定期把 `data/` 拷出仓库做备份 |
| R13 | D8 下模型经 virtiofs 加载变慢 | 冷启动可能超过 90s，被误判为启动失败 | AC-15 实测加载耗时；超标则 `models/` 单独改命名卷 + 一次性导入（§7 权衡说明） |
| R14 | 双容器下后端仍会拉起本地 ASR 子进程（C14） | 重复加载模型抢显存；远端健康却被报不可用 | FR-13.3 列为必做改造项，AC-13 验证后端容器内无 9000 端口子进程 |

---

## 11. 后续实现阶段交付物（本期不产出）

> 以下仅为范围界定，本文件不包含实现。

1. `Dockerfile`（后端）与 `Dockerfile`（ASR 服务，含 CUDA 12.8+ / sm_120 支持的 torch 与 vLLM）、`.dockerignore`
2. `docker-compose.yml`（**双容器，D2 已定**：`backend` + `asr`，含 GPU 声明、`/dev/shm` 扩容、卷挂载、健康检查与依赖顺序）
3. 根目录一键启动/停止/状态 `.bat` 或 `.ps1` 脚本（D6 已定）
4. **`app/` 内改造：remote_bridge 模式下的模型服务启动与状态探测**（FR-13.3 / C14，本期核心改造点）
5. `.env.example` 补充项（FR-9.4）
6. 卷目录初始化脚本与「宿主路径打印」输出（D8 / FR-7-V1、FR-7-V2）
7. 端到端延迟与 RTF 基准步骤（AC-3、AC-15 固化）
8. 部署说明文档（含 `git clean -xfd` 警示与故障排查表，对应 AC-7、AC-14）

---

## 12. 实现与验证记录（2026-10-01）

### 12.1 交付物与需求对应

| 交付物 | 对应需求 |
| --- | --- |
| `docker/Dockerfile` | FR-1.1 ~ FR-1.4（基于 vLLM 官方镜像 + uv 锁文件导出依赖） |
| `.dockerignore` | FR-1.1、NFR-5.3（数据/模型/日志/密钥不入镜像） |
| `docker-compose.yml` | FR-13.1 ~ FR-13.7（双容器、GPU、shm、卷、健康检查、依赖顺序） |
| `scripts/docker/att.ps1` | FR-2、FR-3、FR-4、FR-5（8 阶段启动流程 + 前置自愈 + 就绪轮询 + 停止/状态） |
| `att-start.bat` / `att-start-app.bat` / `att-stop.bat` / `att-status.bat` / `att-logs.bat` | FR-2.1（根目录双击入口，D6） |
| `app/services/streaming_bridge_target.py` | **FR-13.3（C14 修复）**：远端模式下不拉起本地子进程、探测远端健康 |
| `app/services/model_service_manager.py`（改造） | 同上，改为面向桥接目标编程 |
| `tests/test_streaming_bridge_target.py` | C14/FR-13.3 的回归测试（7 项） |
| `.env.example`（补充） | FR-9.4（分块参数、镜像版本、端口） |
| `scripts/bench_streaming_latency.py` | AC-3（RTF 与首段延迟）、AC-15（加载耗时实测） |
| `docs/部署说明-Docker化实时ASR.md` | FR-11.1（故障排查表）、AC-7、AC-14 |

### 12.2 已完成的自动化验证

| 验证项 | 命令 | 结果 |
| --- | --- | --- |
| 单元测试（含 C14 回归） | `uv run python -m pytest tests -q` | **24 passed**（原 17 项 + 新增 7 项） |
| 编排文件合法性 | `docker compose config --quiet` | 通过（`gpus`、`shm_size=8GB`、`service_healthy`、`remote_bridge` 均已正确解析） |
| 脚本语法（PS 5.1 解析器） | `Parser::ParseFile` | 通过 |
| 状态命令冒烟 | `att.ps1 -Action status` | 通过：无容器时优雅降级，且正确打印宿主卷绝对路径（FR-7-V1） |
| 桥接目标选择逻辑 | 单元测试 | 本地/远端两条分支均正确，配置变更后重建缓存 |
| pyproject 单一依赖来源 | `uv export --frozen` | Linux 侧解析为 `vllm==0.14.0 + torch==2.9.1 + qwen-asr==0.0.6`，全部来自 PyPI |

### 12.3 真实环境实测结果（RTX 5090 + Docker Desktop/WSL2）

> 以下全部为**实际跑出的数字**，不是估算。

#### 实时性（AC-3）：达成

命令：`docker compose exec asr python3 scripts/bench_streaming_latency.py --seconds 30`

| 指标 | 实测值 | 判定 |
| --- | --- | --- |
| 总体 RTF（推流总耗时 ÷ 音频时长） | **0.320**（30s 音频 9.60s 处理完） | ✅ 远优于 < 1.0，且满足 ≤ 0.5 |
| 单块平均推理耗时 | 63.9 ms / 200ms 块 | ✅ |
| 单块最大耗时 | 936 ms（每 2s 窗口边界的整窗计算） | ✅ 正常现象，不积压 |
| 实时节奏模拟：末段排队延迟增量 | **−23.8 ms** | ✅ 无积压 |
| 首段文本延迟 | 0.02 s（合成音频） | ✅ |

#### 端到端链路（AC-10）：通过

走后端 WebSocket `/api/asr/stream` 的完整链路：

- 收到 **30 条 `stream_partial`**，`stream_finished` 正常
- 生成记录 `record_id=7886a2f1-...`，duration=6.00s
- 6s 音频端到端耗时 **1.34s（4.5x 实时）**
- 音频回放 `GET /api/records/{id}/audio` → **HTTP 200，192044 bytes**
- 记录 JSON、WAV、`.encryption_key` 均已落盘到宿主卷

#### C14 / 双容器状态（AC-13）：通过

| 检查项 | 实测值 |
| --- | --- |
| `streaming_backend` | `remote_bridge` ✅ |
| `streaming_available` | `True` ✅（修复前此处会是 False） |
| `qwen_streaming_service_managed` | `False`（如实反映由编排托管） |
| 后端容器内本地 9000 子进程数 | **0** ✅（FR-13.3 达成） |
| 后端容器 GPU | 不可见（设计兜底，nvidia-smi not found）✅ |

#### 启动与幂等（AC-1 / AC-2）

| 场景 | 实测耗时 | 判定 |
| --- | --- | --- |
| 冷启动（新容器：模型从 virtiofs 加载 + 引擎预热） | **约 200–222 s** | ❌ **未达成 AC-1 的 ≤ 90s** |
| 重复启动（容器与镜像均未变化） | **5.6 s** | ✅ 走复用分支，无重复容器、无端口冲突 |

#### 引擎预热（本次发现并修复的关键缺陷）

初次实测发现：`/api/preload` **只加载了模型权重，没有触发 vLLM 引擎编译**，导致
**首个音频块耗时 32201 ms**，平均 RTF 1.879 —— 也就是用户每次重启后的第一次录音会卡住半分钟。

修复方式（`scripts/qwen_streaming_service.py` 的 `_warmup_engine`）：预热时用 0.5s 静音
跑一次完整流式推理，把编译开销提前吃掉。修复后同一指标：

| 指标 | 修复前 | 修复后 |
| --- | --- | --- |
| 首块耗时 | 32 201 ms | < 1 000 ms |
| 单块平均 | 375.8 ms | 63.9 ms |
| 总体 RTF | 1.879（不可实时） | **0.320** |

可用 `QWEN_STREAMING_WARMUP=0` 关闭（不建议，关闭后首次录音会重新承受该开销）。

### 12.4 本次实测发现的遗留问题

| 编号 | 问题 | 影响 | 建议 |
| --- | --- | --- | --- |
| I1 | 记录里的 `audio_path` 存成了容器绝对路径 `/app/data/audio/records/xxx.wav` | 容器内回放正常（HTTP 200），但用**原生模式**打开同一份 `data/` 时，这些记录的音频将无法定位 | 后续把记录中的音频路径改为相对路径，读取时按 `DATA_DIR` 解析 |
| **I5** | **（真实使用中发现并已修复）** 远端桥接把「容器可达但模型未加载」误报为 `loading=true`，而前端据 `model_service_loading` **禁用「开启模型」按钮**并显示「模型加载中」 | 用户陷入「流式用不了、加载按钮又点不动」的死局；容器被自动重启后必然触发 | 已修两处：① `RemoteStreamingBridgeTarget.status()` 的 `loading` 改为严格表示「加载进行中」；② 新增 `scripts/docker/asr_entrypoint.sh`，容器启动即自动预热（`QWEN_STREAMING_AUTO_PRELOAD=1`）。新增 2 项回归测试，并复现故障态验证前端不再锁死 |
| I2 | 冷启动约 200s，超出 AC-1 目标（对应风险 R13） | 首次启动等待较久 | 按 §7 权衡说明把 `models/` 改为命名卷后复测；或把 AC-1 目标据实放宽 |
| I3 | Docker Desktop 的 buildx 偶发崩溃（实测数次构建中出现 2 次，与代码无关） | 首次构建可能失败 | 启动脚本已内置**自动重试一次**；仍失败重跑即可 |
| I4 | 镜像体积约 47 GB（基础镜像 cu130 + 完整依赖集） | 构建/占用磁盘较大 | 如需分发再考虑精简（R10） |

### 12.5 仍未实测的验收项

| 验收项 | 状态 |
| --- | --- |
| AC-4 稳定性（连续 30 次流式录音） | 待实测（本轮仅验证单次与链路正确性） |
| AC-6 卷分离（删容器重建后数据仍在） | 待实测 |
| AC-7 失败场景文案（4 种故障模拟） | 部分覆盖（GPU 不可用、端口占用路径已实现），完整场景待逐个模拟 |
| AC-9 桌面能力不退化（热键/静音/托盘/悬浮窗） | 待人工验证 |

### 12.4 与需求文档的实现偏差（3 条）

| 编号 | 偏差 | 理由 | 影响 |
| --- | --- | --- | --- |
| 偏差 1 | 原计划「两个 Dockerfile」，实现为**一个镜像供两个容器共用** | 镜像层完全共享只需构建一次；且两服务运行时版本绝对一致，避免版本漂移 | 仅影响镜像分层策略，不影响 FR-13.1 的「两个服务」要求 |
| 偏差 2 | 分块参数默认**保留上游 2.0**，未直接改为 D5 建议的 1.0 | 未在真实语音上验证过的降延迟调参存在识别质量回归风险；已提供基准脚本，改成「先量化再调」 | AC-3 的延迟目标需在实测后再决定是否调小 |
| 偏差 3 | **后端容器不挂载 GPU** | 作为 C14 的设计兜底：即使代码路径遗漏，业务容器也无法加载模型抢显存 | 后端容器内单段/多段 Qwen 本地推理不可用，需走 Whisper（CPU）；流式链路不受影响 |

---

## 附：本文档依据的代码位置索引

| 用途 | 位置 |
| --- | --- |
| Windows 禁止本地 vLLM 流式 | [app/config.py:67](app/config.py#L67) |
| 运行期目录定义（data / models / logs） | [app/config.py:276](app/config.py#L276)、[app/config.py:322](app/config.py#L322) |
| 桥接服务进程管理器（硬编码 9000、子进程拉起） | [app/services/qwen_streaming_service_manager.py:25](app/services/qwen_streaming_service_manager.py#L25)、[:162](app/services/qwen_streaming_service_manager.py#L162) |
| 流式桥接服务（env 可调分块） | [scripts/qwen_streaming_service.py:33](scripts/qwen_streaming_service.py#L33) |
| 流式会话与落盘 | [app/services/streaming_session_manager.py:64](app/services/streaming_session_manager.py#L64)、[:91](app/services/streaming_session_manager.py#L91) |
| 本地 vLLM provider 与设备校验 | [app/stt/providers/qwen_streaming_runtime_provider.py:120](app/stt/providers/qwen_streaming_runtime_provider.py#L120) |
| 流式 WebSocket 协议 | [app/api/routes.py:856](app/api/routes.py#L856) |
| 前端 PCM 分包（16k / 3200 样本） | [src/composables/useRecorder.js:12](src/composables/useRecorder.js#L12) |
| Electron 后端复用与端口选择 | [electron/python/manager.js:31](electron/python/manager.js#L31)、[:240](electron/python/manager.js#L240) |
| API Key 加密与密钥文件 | [app/utils/encryption.py:27](app/utils/encryption.py#L27) |
| 日志目录 | [app/utils/logging.py:18](app/utils/logging.py#L18) |
| 流式运行能力判定（恒为 `managed_streaming_bridge`） | [app/stt/providers/qwen_asr_provider.py:136](app/stt/providers/qwen_asr_provider.py#L136) |
| 模型服务启动与状态探测（本地 9000 依赖，C14） | [app/services/model_service_manager.py:60](app/services/model_service_manager.py#L60)、[:101](app/services/model_service_manager.py#L101) |
| 忽略规则（data / models / logs 已忽略，C15） | [.gitignore:67](.gitignore#L67)、[:74](.gitignore#L74)、[:78](.gitignore#L78) |
