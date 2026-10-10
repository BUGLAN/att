# 部署说明：Docker 化实时 ASR

对应需求文档：[需求文档-Docker化实时ASR-2026-10-01.md](需求文档-Docker化实时ASR-2026-10-01.md)

---

## 1. 架构一览

```text
Windows 宿主机
  ├── Electron 桌面端（热键 / 录音静音 / 托盘 / 悬浮窗 —— 必须留在宿主，无法容器化）
  │     └── 通过 ATT_BACKEND_PORT（默认 8000）复用容器内后端
  │
  └── Docker Desktop（WSL2 后端）
        ├── asr 容器        att-asr
        │     Qwen3-ASR 流式桥接服务，127.0.0.1:9000，独占 GPU，端口不发布到宿主
        │
        └── backend 容器    att-backend
              FastAPI 业务后端，发布 127.0.0.1:8000
              通过容器网络以 remote_bridge 方式访问 http://asr:9000

卷（全部落在仓库内，决策 D8）
  ./data   -> /app/data    业务数据 + 加密密钥
  ./models -> /app/models  模型缓存（复用宿主已下载模型）
  ./logs   -> /app/logs    应用日志 + 桥接服务日志
  ./.env   -> /app/.env    配置（只读挂载）
```

**关键机制**：后端进程不再自己拉起 ASR 子进程，而是通过 `QWEN_STREAMING_BACKEND=remote_bridge`
+ `QWEN_STREAMING_BASE_URL=http://asr:9000` 探测并预热独立的 ASR 容器
（见 `app/services/streaming_bridge_target.py`）。

---

## 2. 前置条件

| 项 | 要求 | 检查方式 |
| --- | --- | --- |
| 操作系统 | Windows 11 | `winver` |
| WSL2 | 已安装且可运行 | `wsl -l -v` |
| Docker Desktop | 使用 WSL2 后端，已启用 GPU 支持 | `docker version` |
| NVIDIA 驱动 | 支持 CUDA 13（本机 610.88 满足） | `nvidia-smi` |
| GPU | RTX 5090 等 Blackwell（sm_120）可用 | `nvidia-smi -L` |
| 模型 | `models/qwen/Qwen3-ASR-1.7B/` 存在且含 `config.json` | 见 §5 |
| 磁盘 | 镜像约 20GB + 模型约 4GB | `docker system df` |

> 基础镜像 `vllm/vllm-openai:v0.14.0` 约 9GB（压缩），解压后更大；**首次启动需要较长时间拉取**。

---

## 3. 快速开始

### 方式一：双击（推荐）

| 文件 | 作用 |
| --- | --- |
| `att-start.bat` | 一键启动容器并等待流式就绪 |
| `att-start-app.bat` | 同上，并自动打开 Electron 桌面端 |
| `att-status.bat` | 查看容器状态、健康检查、流式就绪状态与宿主卷路径 |
| `att-stop.bat` | 停止容器（**不影响**宿主卷数据） |
| `att-logs.bat` | 实时跟踪容器日志 |

### 方式二：命令行

```powershell
# 启动（首次会自动构建镜像）
pwsh -File scripts\docker\att.ps1 -Action start

# 启动并打开桌面端
pwsh -File scripts\docker\att.ps1 -Action start -WithApp

# 查看状态 / 停止 / 日志
pwsh -File scripts\docker\att.ps1 -Action status
pwsh -File scripts\docker\att.ps1 -Action stop
pwsh -File scripts\docker\att.ps1 -Action logs
```

可用参数：

| 参数 | 说明 |
| --- | --- |
| `-Rebuild` | 强制重新构建镜像 |
| `-NoBuild` | 跳过构建（镜像已存在时加快启动） |
| `-WithApp` | 启动后拉起 Electron 桌面端 |
| `-SkipSettingsFix` | 只提示、不自动修正 `data/settings.json` |
| `-ReadyTimeoutSeconds` | 等待模型就绪的超时（默认 600 秒） |

---

## 4. 一键启动做了什么

启动脚本按 8 个阶段推进，任一阶段失败即以非零退出码结束并打印下一步建议：

| 阶段 | 动作 |
| --- | --- |
| 1/8 | 检查 `.env`（缺失则从 `.env.example` 复制）、创建 `data/`、`models/`、`logs/` 卷目录、检查模型缓存 |
| 2/8 | 校验 `data/settings.json`：`asr_provider=qwen_asr` 且 `recording_mode=streaming`，不符则**备份后自动修正** |
| 3/8 | 检查 Docker 守护进程，未运行时自动拉起 Docker Desktop 并等待 |
| 4/8 | 检查后端端口占用（本项目容器占用则走复用分支） |
| 5/8 | 准备镜像（不存在则构建，可用 `-Rebuild` 强制重建） |
| 6/8 | `docker compose up -d`，等待 `att-asr` 健康检查通过 |
| 7/8 | 在 asr 容器内校验 `torch.cuda.is_available()`，失败即报错并给出排查方向 |
| 8/8 | 调用 `/api/preload` 预热模型（**含 vLLM 引擎预热**），轮询 `/api/health` 直到 `ready && loaded` |

> 第 8 步会额外花约 30 秒做**引擎预热**：vLLM 的首次推理包含图捕获与算子编译，
> 实测首个音频块会因此耗时 32 秒。若不预热，这段开销会完整落在你的第一次录音上
> （表现为「一开口就卡住半分钟」）。用 `QWEN_STREAMING_WARMUP=0` 可关闭。

**容器自身也会自动预热**（`QWEN_STREAMING_AUTO_PRELOAD=1`，默认开启）：
`asr` 容器由 `scripts/docker/asr_entrypoint.sh` 启动，在服务可用后自动调用 `/api/preload`。
因此即使不执行启动脚本（例如 Docker Desktop 开机自动拉起容器），模型也会自行加载到就绪，
避免出现「容器在跑但模型没加载」的状态。

> 第 2 步会改写 `data/settings.json` 中的两个键，改写前会生成
> `settings.json.bak-<时间戳>` 备份。不想被改写请加 `-SkipSettingsFix`。

---

## 5. 模型准备

启动脚本会检查 `models/qwen` 下是否存在含 `config.json` 的模型目录。本机已存在：

```text
models/
└── qwen/
    └── Qwen3-ASR-1.7B/     <- 容器通过 /app/models/qwen 直接挂载复用
        ├── config.json
        └── model.safetensors
```

模型缺失时：启动脚本会警告，第 8 步预热会失败并给出明确错误（不会静默开始长时间下载）。

---

## 6. 数据与备份

| 宿主路径 | 内容 | 丢失后果 |
| --- | --- | --- |
| `data/settings.json` | 用户设置、录音模式 | 需重新配置 |
| `data/.encryption_key` | **Fernet 加密密钥** | 已保存的 LLM API Key 永久无法解密 |
| `data/llm_configs.json` | LLM 提供商配置（API Key 已加密） | 需重新填写 |
| `data/records/`、`data/audio/` | 记录与录音 | 历史数据丢失 |
| `data/clipboard/`、`data/wordbook/` | 剪贴板与词库 | 历史数据丢失 |
| `models/` | 模型缓存 | 需重新下载 |
| `logs/` | 日志 | 仅影响排查 |

**完整备份 = 拷走 `data\` + `models\` + `.env` 三个位置。**

### ⚠️ 严禁 `git clean -xfd`

`data\`、`models\`、`logs\` 都处于 `.gitignore` 忽略状态（`.gitignore:67/74/78`），
因此 `git clean -xfd` 会把它们**整体删除**，且无法从 git 恢复。
排查问题时请勿使用该命令。

---

## 7. 桌面端对接

Electron 已有「复用已存在后端」的能力（`electron/python/manager.js`）：启动时先健康检查
`127.0.0.1:${ATT_BACKEND_PORT}`，通得过就直接复用，不再拉起本地 Python。

```powershell
# 容器后端在跑，直接开桌面端
$env:ATT_REQUIRE_EXISTING_BACKEND = '1'
$env:ATT_BACKEND_PORT = '8000'
npm run electron:dev:existing-backend
```

桌面专有能力（全局热键、录音静音、托盘、剪贴板悬浮窗、选区工具栏）仍在宿主 Windows 侧运行，
不受容器化影响。

---

## 8. 延迟调优

### 8.1 延迟构成

```text
说话 → [前端攒包 200ms] → [分块算法 chunk_size_sec] → [单块推理] → partial 文本
```

容器网络只占几毫秒到几十毫秒，**不是瓶颈**；决定实时性的是分块大小与单块推理速度。

### 8.2 关键参数（`.env`）

| 变量 | 默认 | 说明 |
| --- | --- | --- |
| `QWEN_STREAMING_CHUNK_SIZE_SEC` | `2.0` | 分块时长；调小可降延迟，但推理更频繁、精度可能略降 |
| `QWEN_STREAMING_UNFIXED_CHUNK_NUM` | `2` | 未固定块数（前瞻量），调小可降延迟 |
| `QWEN_STREAMING_UNFIXED_TOKEN_NUM` | `5` | 未固定 token 数 |
| `QWEN_STREAMING_GPU_MEMORY_UTILIZATION` | `0.3` | 显存占用比例；同时跑桌面/其他 GPU 任务时不要调太高 |
| `QWEN_STREAMING_MAX_MODEL_LEN` | `16384` | 最大上下文 |

修改后需重启容器：`att-stop.bat` 然后 `att-start.bat`。

### 8.3 用基准脚本量化（AC-3 / AC-15）

```powershell
# 时序与 RTF（合成音频，能测延迟但测不出识别质量）
docker compose exec asr python3 scripts/bench_streaming_latency.py

# 真实语音（推荐：把 16kHz 单声道 WAV 放到 data 卷里）
docker compose exec asr python3 scripts/bench_streaming_latency.py --wav /app/data/bench.wav
```

输出解读（判据以**总体 RTF** 和**排队延迟是否增长**为准）：

| 指标 | 含义 | 达标线 |
| --- | --- | --- |
| 总体 RTF | 推流总耗时 ÷ 音频时长 | **< 1.0 才能实时**，目标 ≤ 0.5 |
| 末段增量 | 第 4 四分位排队延迟 − 第 3 四分位 | ≤ 0.5s（持续为正变大即积压） |
| 首段文本延迟 | 开始推流到出现非空文本 | ≤ 3.0s |

> 单块耗时的 P95 天然偏高（每 2 秒窗口边界那一次请求要承担整窗计算，实测约 800ms），
> 这是正常现象，**不代表积压**。脚本已把它降级为参考值。

本机实测基线（RTX 5090，30s 音频）：**总体 RTF 0.320，末段增量 −23.8ms，单块平均 63.9ms**。

> ⚠️ 分块调小的前提是 RTF 有余量。若总体 RTF 已接近 1，调小分块只会加重积压。

---

## 9. 故障排查

| 现象 | 原因 | 处理 |
| --- | --- | --- |
| 启动卡在「检查 Docker 守护进程」 | Docker Desktop 未运行或未装好 | 手动打开 Docker Desktop，等待托盘图标变绿后重试 |
| 「asr 容器内无法访问 GPU」 | Docker Desktop 未启用 GPU、WSL2 后端未加载 NVIDIA 驱动 | 确认为 WSL2 后端；宿主执行 `nvidia-smi`；重启 Docker Desktop |
| asr 容器反复重启、日志出现 shared memory 相关报错 | `/dev/shm` 过小 | 已默认 `shm_size: 8gb`；若手工改过 compose 请恢复 |
| 日志出现 `no kernel image is available` | 镜像 CUDA 版本与 50 系显卡不匹配 | 确认使用 `vllm/vllm-openai:v0.14.0`（cu130），或更新 `.env` 中 `VLLM_IMAGE` |
| 「端口 8000 已被其他进程占用」 | 本地原生后端仍在运行 | 关掉本地后端，或改 `.env` 的 `ATT_BACKEND_PORT` |
| 「模型加载失败」+ 显存不足 | 显存被其他程序占用 | 关闭占用显存的程序，或下调 `QWEN_STREAMING_GPU_MEMORY_UTILIZATION` |
| 界面上「当前环境暂不支持流式」 | `data/settings.json` 的 provider 或录音模式不对 | 重新执行 `att-start.bat`（会自动修正并备份），或手工设为 `qwen_asr` + `streaming` |
| 模型服务显示可用但界面报不可用 | 旧版本后端仍在拉起本地子进程（C14） | 确认后端已使用 `remote_bridge`：`att-status.bat` 应显示流式就绪；容器日志无本地 9000 子进程 |
| **界面一直显示「模型加载中」，且「开启模型」按钮点不动** | 旧版本把「容器可达但模型未加载」误报成 `loading=true`，前端据此禁用了按钮（已修复） | 升级到当前版本即可。临时自救：执行 `docker compose exec asr curl -fsS -X POST http://127.0.0.1:9000/api/preload` 手动触发加载 |
| 容器在跑但模型没加载（`/api/health` 里 `loaded=false`） | 容器被 Docker Desktop 自动重启，而无人触发预热 | 已由 `QWEN_STREAMING_AUTO_PRELOAD=1` 自动处理；若关闭了该开关，手动调用 `/api/preload` 或点界面「开启模型」 |
| 后端起不来，日志显示连接 `asr:9000` 失败 | ASR 容器未就绪或已退出 | `att-logs.bat` 查看 asr 日志；`att-status.bat` 确认 att-asr 为 healthy |
| 中文日志乱码 | 控制台代码页 | 用 `att-*.bat`（已 `chcp 65001`），或 `chcp 65001` 后手动执行 |

常用诊断命令：

```powershell
docker compose ps
docker compose logs --tail 200 asr
docker compose logs --tail 200 backend
docker compose exec asr nvidia-smi
docker compose exec asr cat /app/models/qwen/Qwen3-ASR-1.7B/config.json
```

---

## 10. 与原生运行共存

容器化不影响开发期原生运行（决策 D9）：

- 原生后端：`npm run backend:dev:uv`
- 原生流式服务：`npm run qwen:streaming:uv`（监听 9000，后端 `auto` 模式会自动走本地托管）

**注意**：原生流式服务与 asr 容器都使用 9000 端口。容器内不发布该端口，因此不会与宿主冲突；
但若宿主已运行原生流式服务，容器内的 ASR 仍然独立运行，互不干扰。

---

## 11. 已知限制与实现偏差

| 项 | 说明 |
| --- | --- |
| 单镜像双服务 | 需求文档 §11 原写「两个 Dockerfile」，实现改为**一个镜像供两个容器共用**：镜像层完全共享只需构建一次，且能保证两个服务运行时版本绝对一致。构建配置只挂在 `asr` 服务上，避免两个服务并发构建同一镜像引发竞态 |
| 分块默认值 | `QWEN_STREAMING_CHUNK_SIZE_SEC` 默认保留上游 `2.0`（已知稳定），未直接改为 1.0；延迟实验请用 §8.3 脚本量化后再调 |
| 后端容器无 GPU | 后端容器**不挂载 GPU**。这是有意设计：即使配置写错，也无法在业务容器里偷偷加载模型抢显存（C14 兜底）。代价是后端容器内的单段/多段 Qwen 本地推理不可用，需走 Whisper（CPU） |
| 无 GPU 降级 | 未实现（决策 D7 默认不做），GPU 不可用时启动直接报错 |
| ASR 端口不发布 | 9000 仅容器网络内可达；基准脚本需用 `docker compose exec asr` 在容器内执行 |
| **冷启动约 200 秒** | 实测模型从 virtiofs 加载 + 引擎预热合计约 200–222s，**超出 AC-1 的 90s 目标**。容器与镜像未变化时重复启动仅 5.6s。若要缩短，按 §7 权衡说明把 `models/` 改为命名卷后复测 |
| **记录中的音频路径是容器绝对路径** | 记录 JSON 里存的是 `/app/data/audio/records/xxx.wav`。容器内回放正常（HTTP 200 已验证），但用**原生模式**打开同一份 `data/` 时这些记录无法定位音频。属待修问题 |
| 镜像体积约 47 GB | 基础镜像为 CUDA 13 的 vLLM 官方镜像，加上完整依赖集。磁盘紧张时可用 `docker system prune` 清理构建缓存 |
| buildx 偶发崩溃 | Docker Desktop 的 buildx 在构建时偶发崩溃（实测数次中出现 2 次，与项目代码无关）。启动脚本已内置自动重试一次 |
| ~~模型状态误报导致按钮锁死~~ | **已修复**：远端桥接的 `loading` 语义原先为「未就绪即加载中」，导致前端禁用「开启模型」按钮。现改为严格表示「加载进行中」，并新增容器自动预热兜底 |

---

## 12. 相关文件

| 文件 | 作用 |
| --- | --- |
| `docker-compose.yml` | 双容器编排（GPU、shm、卷、健康检查、依赖顺序） |
| `docker/Dockerfile` | 运行时镜像（基于 vLLM 官方镜像 + uv 锁文件依赖） |
| `.dockerignore` | 构建上下文排除规则（数据、模型、日志、密钥不入镜像） |
| `scripts/docker/att.ps1` | 一键启动 / 停止 / 状态 / 日志主逻辑 |
| `att-*.bat` | 根目录双击入口 |
| `scripts/bench_streaming_latency.py` | 延迟与 RTF 基准（AC-3 / AC-15） |
| `app/services/streaming_bridge_target.py` | 本地 / 远端桥接目标选择（C14 / FR-13.3 的修复） |
