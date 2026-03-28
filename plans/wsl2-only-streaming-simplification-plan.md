# WSL2-Only Backend, No Automatic Downgrade

## Summary

将项目收敛为一条明确路径：

- Windows 上的桌面端只连接你手动在 WSL2 启动的 FastAPI。
- 不再支持 Electron 在 Windows 上自启本地 Python 后端。
- 保留 `single / multi / streaming` 三种录音模式，但 `streaming` 失败时直接报错，不再自动回退到 `single`。
- 如果后端环境不对，或 WSL2 内缺少 `qwen-asr[vllm]` / vLLM，主应用直接提示错误，停止当前流式启动。

这个方案的目标是减少“容错分支”和“双路径逻辑”，把开发和运行模型固定下来。

## Key Changes

### 1. Electron 只连现成的 WSL2 FastAPI

- 将桌面端后端策略改为“必须连接现成后端”，默认目标仍是 `127.0.0.1:8000`。
- 删除或停用 `electron/python/manager.js` 中“端口不可用时自动启动本地 Python”的分支。
- 启动时只做健康检查：
  - `/api/health` 可达则继续启动主应用。
  - 不可达则主应用直接报错退出或弹出明确错误页，不再尝试本地兜底。
- `npm run electron:dev` 直接变为“需要现成后端”的模式，不再额外保留 `existing-backend` 作为另一套命令。

### 2. 后端只做明确报错，不做降级

- 保留后端环境识别：
  - `runtime_environment`
  - `qwen_streaming_available`
  - `qwen_streaming_error`
- 但这层只用于“报告事实”，不再承担降级决策。
- `streaming` 启动时，如果当前 FastAPI 不是 WSL2/Linux 可用环境，或缺少 `qwen-asr[vllm]` / CUDA / vLLM，直接返回错误。
- 不再保留“Windows 后端可继续让 streaming 自动退回 single”的业务逻辑。

### 3. 前端保留三种录音模式，但 streaming 失败直接中断

- 保留设置页中的 `single / multi / streaming` 选择，不删模式。
- 删除 `useRecorder.js` 中“检测失败后自动调用 `startSingleRecording()`”的逻辑。
- 改为：
  - 进入 `streaming` 前可先做一次轻量探测。
  - 探测失败时只提示错误，不启动录音，不切模式，不自动回退。
  - 若已开始连接流式 WebSocket，服务端返回 `stream_error` 时直接结束本次启动并提示原因。
- 设置页可以保留一行简单状态文案，但只做展示，不做模式切换或自动修复。

### 4. 简化开发/运行约定

- Windows 开发约定固定为：
  1. 先在 WSL2 中启动 FastAPI。
  2. 再在 Windows 中运行 `npm run electron:dev`。
- 推荐补一个 WSL 启动脚本，例如：
  - 固定使用独立 Linux 虚拟环境
  - 固定 `uv run ... uvicorn app.main:app --host 0.0.0.0 --port 8000`
- 不再围绕 PowerShell 本地后端建立额外兼容分支。
- 当前仓库中的 Windows `.venv` 与 WSL2 Linux 环境分离使用，避免继续复用同一个 `.venv`。

## Public Interfaces / Behavior

- `/api/health?probe_runtime=true` 继续保留，并作为桌面端唯一运行前检查接口。
- 桌面端启动行为变更：
  - 旧行为：无后端时尝试自启本地 Python。
  - 新行为：无后端时直接报错。
- `streaming` 行为变更：
  - 旧行为：环境不满足时自动降级到 `single`。
  - 新行为：环境不满足时直接报错，用户自行处理后端环境。

## Test Plan

### Backend / Runtime

- WSL2 中启动 FastAPI，`/api/health?probe_runtime=true` 返回：
  - `runtime_environment = "wsl2"`
  - `qwen_streaming_available = true/false` 与实际依赖状态一致
- Windows 本机误启动 FastAPI 时：
  - `runtime_environment = "windows"`
  - `qwen_streaming_available = false`
  - `qwen_streaming_error` 为明确错误信息

### Electron / Desktop

- 未启动 WSL2 后端时，`npm run electron:dev` 直接报后端不可用，不再尝试自启 Python。
- 已启动 WSL2 后端时，Electron 正常进入应用并连接 API / WebSocket。
- `streaming` 模式在后端缺少 vLLM 依赖时：
  - 主应用提示错误
  - 不开始录音
  - 不自动切回 `single`
- `single` 和 `multi` 模式仍能按原样工作，不受这次简化影响。

### User-Facing Scenarios

- WSL2 后端正确、依赖完整：`streaming` 可启动。
- WSL2 后端正确、但缺 `qwen-asr[vllm]`：直接报错。
- Windows 后端误占用 `8000`：桌面端显示环境错误，不走流式降级。
- 设置页切到 `streaming` 后，再次录音失败时，模式保持不变，只提示错误。

## Assumptions

- 当前目标是简化 Windows + WSL2 开发/使用路径，不处理“Windows 本地单独跑 FastAPI”这一条链路。
- 当前不删除 `single` 与 `multi` 功能，只删除 `streaming -> single` 的自动回退。
- 当前不规划打包后的离线一体化部署策略；若后续要支持打包自带后端，再单独设计。
- 默认桌面端连接地址继续使用 `127.0.0.1:8000`，不额外引入复杂的后端发现机制。
