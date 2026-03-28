# Qwen 真流式部署说明

## 结论

当前仓库里的前端录音链路和 FastAPI 后端已经支持 `streaming` 模式。

Qwen 官方目前明确说明：`Qwen3-ASR` 的流式推理只支持 `vLLM backend`。因此要在 Windows 上跑真流式，关键不是改主应用架构，而是给 Qwen 准备一个可用的 vLLM 运行时。

这件事现在有两条可行路径：

1. `remote_bridge`：推荐。把 Qwen 流式服务作为独立进程运行，再让主应用通过 HTTP 桥接过去。
2. `local_vllm`：可选。让主应用后端直接在当前 Python 环境里加载 Qwen + vLLM。

如果你准备用 [SystemPanic/vllm-windows](https://github.com/SystemPanic/vllm-windows)，建议优先走 `remote_bridge`，即使 Qwen 服务和主应用都在同一台 Windows 机器上也一样。这样依赖更好隔离，排障也更简单。

## 1. Windows 环境前提

当前这条 Windows 路线按以下版本基线收敛：

- Python 3.12
- CUDA 12
- GPU 版 `torch==2.11`
- `vllm-windows / vllm==0.17.0`

如果你的机器环境不是这组版本，要么自行调整 wheel/索引并重新验证，要么改走 Linux / WSL2。

推荐做法：

1. 使用 `uv` 创建独立的 Qwen 流式虚拟环境。
2. 先安装 GPU 版 `torch`，不要使用 CPU 默认包。
3. 安装 `vllm-windows 0.17.0`。
4. 安装项目依赖并校验 `torch.cuda.is_available()`。

仓库里已经提供了安装脚本：

```powershell
powershell -ExecutionPolicy Bypass -File .\scripts\setup_qwen_streaming_windows_uv.ps1
```

脚本默认会：

- 创建 `.venv`
- 使用 `uv` 安装 Python 3.12 环境
- 根据 [pyproject.toml](/E:/repo/att/pyproject.toml) 中的 `tool.uv.sources` / `tool.uv.index` 配置，从 CUDA 12 的 PyTorch GPU 索引安装 `torch==2.11`
- 安装 `vllm==0.17.0`
- 安装项目依赖并打印 CUDA 可用性

如果你手里已经有 `vllm-windows` 的本地 wheel，也可以传入：

```powershell
powershell -ExecutionPolicy Bypass -File .\scripts\setup_qwen_streaming_windows_uv.ps1 `
  -VllmWheel "C:\path\to\vllm-0.17.0-*.whl"
```

## 2. 推荐方案：Windows 上用 `remote_bridge`

这种方式最适合当前仓库，因为主应用和 Qwen 真流式服务是分开的：

- 主应用继续跑自己的 FastAPI / Electron
- Qwen 服务单独跑 `scripts/qwen_streaming_service.py`
- 主应用通过 `QWEN_STREAMING_BASE_URL` 去桥接

即使两者都部署在同一台 Windows 机器上，这仍然是“进程隔离”，不是“跨机器部署”。

### 2.1 启动独立 Qwen 流式服务

直接用仓库里的 UV 启动脚本：

```powershell
powershell -ExecutionPolicy Bypass -File .\scripts\start_qwen_streaming_uv.ps1
```

自定义参数示例：

```powershell
powershell -ExecutionPolicy Bypass -File .\scripts\start_qwen_streaming_uv.ps1 `
-EnvDir ".venv" `
  -Port 9000 `
  -Model "Qwen/Qwen3-ASR-1.7B" `
  -CacheDir "models/qwen"
```

这个脚本会固定使用 `uv run --python <虚拟环境>` 启动，不再使用默认 `python scripts/qwen_streaming_service.py`。

### 2.2 配置主应用桥接这个服务

在主应用的 `.env` 中配置：

```env
ASR_PROVIDER=qwen_asr
QWEN_STREAMING_BACKEND=remote_bridge
QWEN_STREAMING_BASE_URL=http://127.0.0.1:9000
QWEN_STREAMING_TIMEOUT_SECONDS=30
```

然后照常启动主应用。

这样一来，录音模式切到 `streaming` 时，就会走独立的 Qwen 真流式服务。

## 3. 可选方案：Windows 上用 `local_vllm`

如果你已经把 `qwen-asr` 和 `vllm-windows` 都装进了主应用当前 Python 环境，也可以直接让主应用本地加载：

```env
ASR_PROVIDER=qwen_asr
QWEN_STREAMING_BACKEND=local_vllm
QWEN_STREAMING_BASE_URL=
QWEN_STREAMING_TIMEOUT_SECONDS=30
```

这种方式少一个桥接进程，但有两个代价：

- 主应用环境会更重
- Qwen / vLLM 初始化失败时，会直接影响主应用后端启动或流式录音链路

所以除非你已经把本地环境打磨得很稳定，否则仍然更建议 `remote_bridge`。

## 3.1 开发环境后端启动也改成 `uv`

当前仓库的 Electron 开发态后端启动已经切到 `uv run`，不再默认走系统 `python`。

可直接使用：

```bash
npm run backend:dev:uv
```

以及：

```bash
npm run qwen:streaming:uv
```

## 4. Linux / WSL2 仍然是兼容兜底方案

如果 Windows + `vllm-windows` 版本组合暂时不稳定，可以保留同样的架构，只把 `scripts/qwen_streaming_service.py` 放到 Linux / WSL2 里运行。

这时主应用侧配置完全不需要变，仍然是：

```env
QWEN_STREAMING_BACKEND=remote_bridge
QWEN_STREAMING_BASE_URL=http://127.0.0.1:9000
```

区别只在于服务端运行环境从 Windows 换成了 Linux / WSL2。

## 5. 健康检查

主应用的 `/api/health` 会返回：

- `qwen_streaming_backend`
- `qwen_streaming_available`
- `qwen_streaming_error`

如果桥接或本地 vLLM 初始化成功，`recording_mode=streaming` 就会走真 Qwen 流式链路。
