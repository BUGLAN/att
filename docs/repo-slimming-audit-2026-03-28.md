# 仓库精简审计

日期：2026-03-28

## 结论摘要

当前仓库已经完成了“Electron 承接桌面能力”的主迁移，但仓库里仍同时保留了：

- 新主线：`electron/` + `src/` + `app/`
- 旧主线：`tray/` + `run_tray.py` + `static/`
- 历史脚本与设计资产：`scripts/*.ps1`、`plans/`、`stitch-design/`
- 运行产物：`dist/`、`logs/`、`__pycache__/`、`.pyc`

从代码引用关系看，当前真正仍在跑的核心链路是：

1. Electron 主进程从 [electron/main.js](/home/buglan/repo/att/electron/main.js) 启动。
2. Electron 主进程通过 [electron/python/manager.js](/home/buglan/repo/att/electron/python/manager.js) 拉起 FastAPI 后端。
3. Electron 窗口加载 `src/` 经过 Vite 构建后的 `dist/index.html`，而不是 `static/index.html`。
4. Vue 前端经由 [src/api/client.js](/home/buglan/repo/att/src/api/client.js)、[src/composables/useRecorder.js](/home/buglan/repo/att/src/composables/useRecorder.js)、[src/composables/useWebSocket.js](/home/buglan/repo/att/src/composables/useWebSocket.js) 与后端通信。

也就是说，Python tray 和旧版 `static/` 前端都已经不是当前桌面主流程的必需组成。

## 我这次实际验证了什么

- `python -m pytest tests -q`
  - 结果：`9 passed, 1 failed`
  - 失败点：Qwen 流式 provider 的中文错误信息出现编码异常，不是“精简导致”的失败。
- `bash -lc "npm run build"`
  - 结果：通过，Vite 成功构建到 `dist/`
- 代码引用检查结论：
  - [electron/main.js](/home/buglan/repo/att/electron/main.js) 只加载 `dist/index.html`
  - [app/main.py](/home/buglan/repo/att/app/main.py) 仍保留 `static/` 挂载和回退逻辑
  - [app/services/hotkey_listener.py](/home/buglan/repo/att/app/services/hotkey_listener.py) 当前没有任何调用点
  - [tray/](/home/buglan/repo/att/tray) 仍被 Git 跟踪，但当前主链路未引用
  - [static/](/home/buglan/repo/att/static) 仍被 Git 跟踪，但 Electron 不再加载它

## 高把握可删项

这部分从当前代码关系看，已经明显属于历史遗留。

### 1. 旧 Python Tray 实现

候选：

- [tray/](/home/buglan/repo/att/tray)
- [run_tray.py](/home/buglan/repo/att/run_tray.py)
- [scripts/build_app.py](/home/buglan/repo/att/scripts/build_app.py)

证据：

- 当前桌面入口是 [electron/main.js](/home/buglan/repo/att/electron/main.js)，并且内部已经使用 [electron/tray/index.js](/home/buglan/repo/att/electron/tray/index.js) 实现托盘。
- 检索中没有发现 `app/`、`electron/`、`src/` 对 `tray/` 或 `run_tray.py` 的运行时引用。
- [scripts/build_app.py](/home/buglan/repo/att/scripts/build_app.py) 仍然在构建 `python -m tray.main` 的 macOS `.app`，已经是旧路线。

建议：

- 优先整体移除 `tray/`、`run_tray.py`、`scripts/build_app.py`
- 同步清理相关说明文档和安装说明

### 2. 旧版静态前端

候选：

- [static/](/home/buglan/repo/att/static)

证据：

- [electron/main.js](/home/buglan/repo/att/electron/main.js) 生产态固定加载 `../dist/index.html`
- [vite.config.js](/home/buglan/repo/att/vite.config.js) 的构建输入来自 `src/`
- [static/index.html](/home/buglan/repo/att/static/index.html) 自带整套旧页面、旧脚本和旧 WebSocket 交互
- 旧静态页仍在发送 `type: "recording"`，对应发送端只在 [static/js/websocket.js](/home/buglan/repo/att/static/js/websocket.js) 和 [tray/websocket_client.py](/home/buglan/repo/att/tray/websocket_client.py) 出现，说明它和旧 tray 是一套历史链路

建议：

- 如果你已经不再支持“直接浏览器访问 FastAPI 页面”的旧模式，可以整体删除 `static/`
- 删除后再简化 [app/main.py](/home/buglan/repo/att/app/main.py) 中的 `/static` 挂载和 `static_index` 回退

### 3. 后端未接线的旧热键监听

候选：

- [app/services/hotkey_listener.py](/home/buglan/repo/att/app/services/hotkey_listener.py)

证据：

- 当前仓库里没有任何地方实例化 `PynputHotkeyListener` 或该文件里的管理器
- 现在热键入口已经迁移到 [electron/input/listener.js](/home/buglan/repo/att/electron/input/listener.js)

建议：

- 可以删除该文件
- 同步清理与后端 `pynput` 热键相关的依赖说明

## 高概率可删，但建议先做一次小确认

### 4. WebSocket 中专门服务旧 tray 的消息分支

候选：

- [app/services/websocket.py](/home/buglan/repo/att/app/services/websocket.py) 中的 `"tray"` client 类型说明
- [app/api/routes.py](/home/buglan/repo/att/app/api/routes.py) 中 `recording`、`web_recording`、`settings_update` 里专门广播给 `"tray"` 的逻辑
- [src/composables/useWebSocket.js](/home/buglan/repo/att/src/composables/useWebSocket.js) 里的 `sendSettingsUpdate`
- [src/composables/useRecorder.js](/home/buglan/repo/att/src/composables/useRecorder.js) 中对 `recording_command` 的兼容处理

证据：

- 当前仓库里没有新的 Electron 代码连接 `/api/ws/tray`
- `recording` 消息发送方只出现在旧静态页和旧 Python tray
- `sendSettingsUpdate` 在 `src/` 内没有调用点
- 新桌面能力已经通过 Electron IPC 直接完成，例如：
  - `hotkey:update`
  - `desktop:sync-input-settings`
  - `desktop:sync-settings`
  - `desktop:recording-state`

建议：

- 如果你确认不会再独立运行旧 tray，也不再支持旧网页前端，可以删除这些“兼容旧 tray”的 WebSocket 分支
- 这一步建议放在删除 `tray/` 与 `static/` 之后做

### 5. `requirements.txt` 作为第二份依赖清单

候选：

- [requirements.txt](/home/buglan/repo/att/requirements.txt)

证据：

- 当前开发脚本和打包脚本已经优先使用 [pyproject.toml](/home/buglan/repo/att/pyproject.toml)
- [scripts/package-electron.js](/home/buglan/repo/att/scripts/package-electron.js) 明确写了“存在 `pyproject.toml` 就不用 `requirements.txt`”
- 仓库说明却还保留 `pip install -r requirements.txt`
- `requirements.txt` 里还有明显偏旧的内容，比如：
  - `PySide6`
  - `pyinstaller`
  - 旧 tray 依赖
  - `ffmpeg-python`

建议：

- 选择一个单一事实源
- 如果现在以 `uv` + `pyproject.toml` 为准，建议删除 `requirements.txt`
- 如果暂时不删，至少要把它改成由 `pyproject.toml` 派生，避免双份清单漂移

### 6. 旧依赖项

优先关注这些依赖是否还能删：

- Python 侧：
  - `PySide6`
  - `Pillow`
  - `keyboard`
  - `pyinstaller`
  - `ffmpeg-python`
- 可疑但要再看平台：
  - `pyobjc`
  - `pycaw`

证据：

- `PySide6`、`keyboard`、`pyinstaller` 明显对应旧 Python tray 路线
- `ffmpeg-python` 当前代码检索未发现有效调用
- `pycaw` 仍被 [electron/python/audio_helper.py](/home/buglan/repo/att/electron/python/audio_helper.py) 使用，不能直接删
- `pyobjc` 不在当前 Electron 主链直接出现，但如果 [electron/python/paste_helper.py](/home/buglan/repo/att/electron/python/paste_helper.py) 或桌面辅助脚本还依赖 macOS 辅助能力，需要再确认

建议：

- 删 tray 后，先把 `PySide6`、`keyboard`、`pyinstaller` 从依赖中剔除
- `Pillow` 是否保留，要看 Electron 图标链路是否仍需要 Python 侧产图
- `pycaw` 暂时保留

## 不一定要删代码，但强烈建议移出版本库

### 7. 运行产物和本地环境

候选：

- [dist/](/home/buglan/repo/att/dist)
- [logs/](/home/buglan/repo/att/logs)
- [tmp/](/home/buglan/repo/att/tmp)
- [node_modules/](/home/buglan/repo/att/node_modules)
- [.venv/](/home/buglan/repo/att/.venv)
- 各类 `__pycache__/`、`.pyc`
- [npminstall-debug.log](/home/buglan/repo/att/npminstall-debug.log)

说明：

- 这些目录大多已经被 [.gitignore](/home/buglan/repo/att/.gitignore) 忽略
- 但仓库里实际仍存在不少运行产物，说明之前可能已经提交过，或者本地长期累积没有清理

建议：

- 保持它们不进入版本库
- 如果 Git 历史里已经跟踪过，做一次统一 `git rm --cached` 清理会很值

## 文档和配置漂移

### 8. 仓库说明仍然偏旧

候选：

- [AGENTS.md](/home/buglan/repo/att/AGENTS.md)
- [CLAUDE.md](/home/buglan/repo/att/CLAUDE.md)
- [.env.example](/home/buglan/repo/att/.env.example)

证据：

- 说明里仍强调 `pip install -r requirements.txt`
- `AGENTS.md` 仍把 `tray/` 作为主要模块介绍
- `.env.example` 里还有 `DATABASE_URL=sqlite+aiosqlite:///data/fragments.db`，但当前代码已经是 JSON 文件存储，并无对应使用点

建议：

- 文档与配置示例要跟“Electron + pyproject + uv + JSON storage”的现状对齐
- 这一步不删代码，但能显著减少以后继续引入遗留物

## 可能转为归档而不是删除的目录

这些不是运行时依赖，但如果你想把仓库做得更轻，也值得单独处理。

### 9. 设计稿与计划文档

候选：

- [stitch-design/](/home/buglan/repo/att/stitch-design)
- [plans/](/home/buglan/repo/att/plans)

证据：

- 当前代码内未发现对 `stitch-design/`、`plans/` 的运行时引用
- 它们更像设计输入、阶段性方案和问题追踪记录

建议：

- 若这些内容仍有团队价值，可保留但迁移到 `docs/archive/`
- 若只是一次性讨论材料，可考虑移出主仓库或压缩归档

## 推荐的精简顺序

建议按下面顺序做，风险最低：

1. 删除旧 Python tray 路线
   - `tray/`
   - `run_tray.py`
   - `scripts/build_app.py`
2. 删除旧静态前端
   - `static/`
   - 简化 [app/main.py](/home/buglan/repo/att/app/main.py) 的静态挂载和回退逻辑
3. 删除未接线旧后端热键代码
   - [app/services/hotkey_listener.py](/home/buglan/repo/att/app/services/hotkey_listener.py)
4. 缩掉只为旧 tray 保留的 WebSocket 兼容分支
5. 统一依赖源
   - 只保留 `pyproject.toml`
   - 清理旧 tray 依赖
6. 清理文档和 `.env.example`
7. 清理版本库中的产物文件与本地垃圾

## 每删一轮建议做的回归验证

1. `bash -lc "npm run build"`
2. `python -m pytest tests -q`
3. 启动 Electron，验证这些路径：
   - 主窗口可打开
   - 录音可开始/停止
   - 热键能触发
   - 剪贴板悬浮窗可触发
   - 设置页可保存
   - 历史记录可加载
4. 如你保留 Qwen 流式：
   - 单独验证 `recording_mode=streaming`

## 我额外提醒你的几个“举一反三”点

- 不要只删目录，也要删“与它绑定的协议”和“与它绑定的依赖”。
- 不要同时维护 `requirements.txt` 和 `pyproject.toml` 两份真相。
- 后端里凡是还在提 `"tray"` client 的接口，都值得再问一句：现在到底还有没有真实消费者。
- `static/` 这种“回退页面”如果长期不跑，未来最容易悄悄腐烂，通常比直接删掉更危险。
- 旧文档不只是“看起来乱”，它会持续把后来的人带回错误入口。

## 本次审计的一个额外发现

当前测试失败点不是精简问题，而是中文字符串编码问题：

- [tests/test_qwen_streaming_runtime_provider.py](/home/buglan/repo/att/tests/test_qwen_streaming_runtime_provider.py)
- [app/stt/providers/qwen_streaming_runtime_provider.py](/home/buglan/repo/att/app/stt/providers/qwen_streaming_runtime_provider.py)

这件事和仓库瘦身无关，但建议顺手修掉，否则后续做清理时容易把“编码噪音”误判成“功能回归”。
