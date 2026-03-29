# Windows / WSL2 / macOS 最小迁移计划书

## 目标

将项目尽快迁回 `Windows PowerShell` 作为主开发与主运行环境，同时继续保留 `WSL2` 和 `macOS` 支持。

当前已知唯一未稳定支持的是 `Qwen3-ASR` 真流式链路；其余能力按可用处理。

## 现状结论

- `Electron + Vite + 主界面`：可在 Windows 运行
- `FastAPI + Whisper + Qwen3-ASR single/multi`：可迁移到 Windows 运行
- `Qwen3-ASR streaming`：暂不作为 Windows 本地稳定能力承诺
- `模型服务托管`：Windows 必须支持，不能因为 streaming 不稳定就整体禁用
- Python 依赖入口统一使用 `pyproject.toml` + `uv`
- `requirements.txt` 不再作为正式依赖入口

## 设计原则

1. 将“模型服务托管能力”和“流式识别能力”拆开
2. Windows 支持模型服务托管
3. Windows 下 streaming 是否可用单独做能力判断
4. 主应用跨平台保持同一套 UI / 状态机
5. `Qwen streaming bridge` 继续作为独立服务抽象，不和主 FastAPI 业务逻辑耦死

## 目标能力矩阵

### Windows

- Electron：支持
- FastAPI：支持
- Whisper single/multi：支持
- Qwen single/multi：支持
- 模型服务托管：支持
- Qwen streaming：暂不承诺本地稳定可用，按能力探测决定

### WSL2 / Linux

- 全部支持

### macOS

- Electron：支持
- FastAPI：支持
- Whisper single/multi：支持
- Qwen single/multi：按运行环境支持
- 模型服务托管：支持
- Qwen streaming：按能力探测决定

## 改造范围

### 一、依赖与环境管理

目标：统一依赖入口，删除双轨维护。

修改项：

- 删除 `requirements.txt`
- 清理所有 `pip install -r requirements.txt` 引用
- 文档统一改成 `uv sync` / `uv run`

验收标准：

- 新开发者只看 `pyproject.toml` + `uv` 就能完成环境初始化
- 文档中不再出现 `requirements.txt` 作为正式入口

### 二、能力模型拆分

目标：将“托管”和“流式”分成两个状态。

建议状态字段：

- `managed`
- `running`
- `loading`
- `ready`
- `streaming_available`
- `streaming_error`

含义：

- Windows 上允许出现 `managed = true` 且 `streaming_available = false`
- UI 要能准确表达“模型已经开启，但当前不支持流式录音”

验收标准：

- “模型服务按钮”和“录音按钮”逻辑不再混淆托管和流式能力
- single/multi 不会被 streaming 限制误伤

### 三、模型服务托管跨平台化

目标：Windows / WSL2 / macOS 都能托管模型服务。

当前问题：

- `pkill`
- `os.killpg`
- `SIGTERM / SIGKILL`
- `start_new_session=True`

这些实现是 Unix 导向，不能直接在 Windows 复用。

改造方案：

- 抽一层进程管理接口
- 按平台分别实现

建议抽象：

- `start_process()`
- `stop_process_tree()`
- `cleanup_stale_processes()`

验收标准：

- 模型服务在 Windows 能被主应用启动、停止、清理
- 后端退出时不会留下孤儿进程
- 多次启停不会残留旧 bridge 进程

### 四、Windows 下 streaming 采用“托管支持、能力降级”

目标：先把产品行为做对，不强行承诺 Windows 本地真流式。

策略：

- Windows 可以启动 bridge 服务并完成 preload
- bridge health 明确区分：
  - 服务在线
  - 模型已加载
  - streaming session 是否可创建
- 若 streaming 不可用：
  - “模型已开启”仍然可以亮
  - 流式录音按钮保持禁用
  - 给出明确错误原因

验收标准：

- Windows 上模型服务按钮能正常工作
- 切到流式模式时，如果 streaming 不可用，用户看到明确状态，而不是假加载或假可用

### 五、统一模型服务入口

目标：所有模型都走同一套“开启模型 / 模型已开启 / 加载中”的逻辑。

统一规则：

- 用户切模型、切模式后，当前模型服务自动卸载
- 用户点击“开启模型”后，加载当前模式所需运行时
- 流式仅在 `streaming_available` 时允许开始录音

验收标准：

- 切换 `Whisper -> Qwen` 或 `single -> streaming` 后，不再复用旧状态
- UI 状态和实际加载模型一致

### 六、Windows 主运行方案

目标：让 Windows 成为默认主运行环境。

建议工作区方案：

- 前端 / Electron：Windows 本地目录
- Python 后端：优先 Windows 本地运行
- 若 streaming 仍不稳：
  - bridge 可先放 WSL2
  - 主应用仍然运行在 Windows

短期支持两种拓扑：

1. Windows 主应用 + Windows 后端 + WSL2 bridge
2. Windows 主应用 + Windows 后端 + Windows bridge（实验性）

验收标准：

- Windows 上 single/multi 全链路可跑
- 不依赖 WSL 才能开发普通功能
- streaming 允许外接 bridge

## 分阶段计划

### 阶段 1：清理依赖入口

- 删除 `requirements.txt`
- 更新文档和脚本说明
- 统一使用 `uv`

### 阶段 2：托管能力与流式能力拆分

- 调整健康检查和前端状态字段
- 让 UI 能区分“模型服务已开启”和“流式不可用”

### 阶段 3：模型服务进程管理跨平台

- 抽象进程管理
- 支持 Windows 进程树启动与清理

### 阶段 4：Windows 主环境验收

验证以下场景：

- Whisper single
- Whisper multi
- Qwen single
- Qwen multi
- 模型切换自动卸载
- 模型服务启停
- Electron + 后端联动

### 阶段 5：Qwen streaming 渐进支持

- 先支持 bridge 托管和状态展示
- 再逐步验证 Windows 本地 streaming session
- 不稳定时继续允许桥接到 WSL2 / Linux

## 风险

- `vllm-windows` 稳定性仍有不确定性
- Windows 上 `ffmpeg/ffprobe` 需要明确安装路径
- Electron 与 Python 本地环境路径要统一
- Windows 与 WSL 不能共用 `node_modules` 或 `.venv`

## 建议的最小落地顺序

1. 删除 `requirements.txt` 并清理引用
2. 拆分 `managed` 和 `streaming_available`
3. 做 Windows 模型服务托管
4. 验收 Windows single/multi
5. 最后推进 Windows streaming

## 一句话方案

短期将 `Windows` 做成主运行环境，保证 `Whisper / Qwen` 的 single/multi 和模型托管都可用；`Qwen streaming` 先作为可降级能力，通过 bridge 和能力探测接入，而不是阻塞整体迁移。
