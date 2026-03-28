# Codex 启动报错自动化

这个方案解决的是一个高频手工动作：

1. 启动项目
2. 发现启动报错
3. 手动复制日志给 Codex
4. 等 Codex 修复
5. 再重新启动

现在仓库里增加了一个脚本，把第 2 步到第 3 步之间的大部分动作自动化了。

## 推荐工作流

### 后端启动

```powershell
npm run codex:backend
```

脚本会自动做这些事：

1. 启动 `npm run backend:dev:uv`
2. 把 stdout 和 stderr 分别写入 `tmp/codex-startup/<时间戳>/`
3. 轮询 `http://127.0.0.1:8000/api/health`
4. 如果健康检查通过，保留服务继续运行
5. 如果启动失败或超时，自动生成一段给 Codex 的修复提示
6. 尝试把这段提示复制到系统剪贴板

失败时你只需要回到 Codex，直接粘贴即可，不用再手工翻日志。

### 前端启动

```powershell
npm run codex:frontend
```

前端预设会启动 `npm run dev`，并用 `http://127.0.0.1:5173` 作为启动成功检查。

### 自定义命令

```powershell
powershell -ExecutionPolicy Bypass -File scripts/start_dev_with_codex.ps1 `
  -Preset custom `
  -Command "python -m uvicorn app.main:app --reload" `
  -HealthUrl "http://127.0.0.1:8000/api/health"
```

如果你的进程没有健康检查接口，也可以改用成功日志匹配：

```powershell
powershell -ExecutionPolicy Bypass -File scripts/start_dev_with_codex.ps1 `
  -Preset custom `
  -Command "npm run some-service" `
  -SuccessPattern "ready|started|listening"
```

## 输出位置

每次执行都会生成一个独立目录：

```text
tmp/codex-startup/<timestamp>/
```

其中包含：

- `stdout.log`
- `stderr.log`
- `codex-prompt.md`

同时还会维护两个固定入口：

- `tmp/codex-startup/latest-codex-prompt.md`
- `tmp/codex-startup/latest-session.txt`

## 为什么不优先用 Codex 定时自动化

Codex 当前的自动化更适合“定时执行”任务，比如每天巡检、定时生成报告。你的场景是：

- 手动触发
- 需要立刻响应
- 依赖本地进程状态和本地日志
- 通常还需要看过错误后再决定是否让 Codex直接改代码

所以这个场景更适合“本地脚本驱动”，而不是“定时自动化”。

## 下一步可以继续升级的方向

如果你觉得这套流程顺手，下一步可以继续做两种升级：

1. 增加一个“重试启动”脚本，在 Codex 修复后自动再次启动对应服务。
2. 如果你本机 `codex` 命令支持非交互提问，再把 `codex-prompt.md` 直接提交给 Codex，而不是走剪贴板。

第二步我建议等你确认本机 `codex` CLI 的实际调用方式后再接，不然容易把命令接口写死。
