# AGENTS.md

给 AI 编码代理（Codex / Claude Code 等）的工作指引。**动手前先读完本文件**，
尤其是「硬约束」与「陷阱清单」两节——那里每一条都是实际踩过的坑。

项目：Audio to Text（ATT），Electron + Vue 3 + FastAPI 的桌面语音转文字应用。

---

## 1. 命令

```bash
uv sync                              # Python 依赖（要求 3.12，见 pyproject.toml）
npm install                          # 前端依赖

npm run backend:dev:uv               # 只跑后端（uvicorn --reload）
npm run electron:dev                 # Vite(8090) + Electron（Electron 自己拉起后端）
npm run electron:dev:existing-backend # 复用已启动的后端
npm run build                        # 渲染进程构建
node scripts/package-electron.js --win   # 打包（见 §5 已知失败）

uv run python -m pytest tests -q     # 后端测试（281 个用例）
node scripts/run-node-tests.mjs      # 前端纯逻辑测试（15 个文件）
npm test                             # 上面两条
```

`pytest` 只收集 `tests/`（`pyproject.toml` 的 `testpaths`）。
Node 测试必须走 `scripts/run-node-tests.mjs`：`tests/` 里有手写 `main()` 的用例会
`process.exit(0)`，直接用 `node --test` 会截断汇总。

---

## 2. 架构速查

```text
electron/     主进程：后端进程管理、桌面代理(热键/静音)、托盘、悬浮窗、统一浮层、剪贴板监控
src/          Vue：stores(Pinia) / composables(录音编排) / components(common 原语 → 页面)
app/          FastAPI：
  api/routers/   HTTP + WS 端点（只做解析与组装，不写业务）
  services/      编排（settings / records / transcription / model_service）
  repositories/  JSON 持久化（原子写、单写者锁、索引、路径安全）
  stt/           ASR provider（base=批量、streaming_base=流式、factory=装配）
  llm/           LLM 配置、网关、优化器、多 provider
  records/ clipboard/ wordbook/ ai_actions/ prompts/   业务域
  config.py      Settings(环境变量) + UserSettings(data/settings.json)
```

细节见 `docs/架构说明.md`；端点见 `docs/API.md`；配置见 `docs/配置说明.md`。

---

## 3. 硬约束（改动必须遵守）

1. **注释与新增文档一律用中文**（除非有明确理由用英文）。
2. **JSON 写入必须走 `app/repositories/json_store.py`**（`write_json_atomic` /
   `write_json_atomic_async`，同目录 tmp + `os.replace`）。
   禁止 `open(path, "w")` 原地截断写——半截 JSON 会让记录静默消失。
3. **不得绕过单写者约束**：`app/main.py` 启动时 `acquire_writer_lock()`，第二个后端进程
   写同一 `data/` 会被拒绝启动（这是有意设计，不要改成"容错继续跑"）。
4. **密钥不得落明文**：`data/settings.json` 里豆包 Token 存
   `doubao_asr_api_key_encrypted`（Fernet，密钥在 `data/.encryption_key`），
   `data/llm_configs.json` 存 `api_key_encrypted`。
   `GET /api/settings` **只返回** `doubao_asr_api_key_set: bool`；
   日志与异常文案里不得出现密钥。
5. **测试不得依赖本机环境**：`tests/conftest.py` 有 autouse fixture 把
   `app.stt.factory.user_settings` 换成空实现。新增涉及 `get_asr_provider_config()`
   的用例时不要去掉它（否则本机 `data/settings.json` 会让断言漂移，实际踩过）。
6. **ASR 协议细节只允许出现在 `app/stt/providers/*`**：路由、服务、页面都不得直接碰
   二进制帧；新增 provider 只需实现 `ASRProvider`（批量）与/或 `StreamingASRProvider`
   （流式），再在 `app/stt/factory.py` 注册。
7. **前端访问外部能力只走两个包装层**：HTTP → `src/api/client.js`；
   桌面能力 → `src/composables/useElectron.js`。不要在组件里直接 `new WebSocket` 或
   直接 `window.electronAPI.*`。
8. **录音编排的纯逻辑要抽到 `src/composables/recorder/*.mjs`**（不依赖 Vue/浏览器全局），
   并在 `tests/test_recorder_*.mjs` 里补用例——`useRecorder.js` 已经 1800+ 行，
   不要再往里堆可抽离的计算。

---

## 4. 陷阱清单（每条都有实际案例）

| 陷阱 | 说明 |
| --- | --- |
| `data/settings.json` 覆盖 `.env` | `get_asr_provider_config()` 里 `user_settings.get_all()` 后合并，所以改了 `.env` 的 `ASR_PROVIDER` 可能不生效，以设置页/`settings.json` 为准 |
| 豆包密钥优先级 | 用户设置（密文）> runtime 参数 > `DOUBAO_ASR_API_KEY` 环境变量；`resolve_doubao_api_key()` 先看密文 |
| `/api/prompts` 写接口恒 400 | `app/prompts/file_manager.py` 的 `create/update/delete` 主动 `raise ValueError`；预设的唯一真相源是 `prompts/*.txt`，改预设要改文件 |
| 统一浮层没有入口 | `electron/main.js` 的 `overlay:open-clipboard` / `overlay:open-selection` 通道**没有渲染侧调用方**，托盘/快捷键也没有对应项 → 浮层打不开。改浮层前先决定"入口从哪来" |
| `app/api/routes.py` 的聚合 `router` 不参与生产挂载 | `app/main.py` 逐个 `include_router` 子路由；聚合层只为测试与兼容再导出。别顺手 `include_router(routes.router)`，会重复挂载 |
| `/api/streaming/service*` 是别名 | 等价于 `/api/model-service*`，前端只用后者 |
| 健康字段双命名 | `qwen_streaming_*` 是旧名（前端仍读），`streaming_*` / `model_service_*` 是新名，两者同源。删旧名前先全仓检索前端 |
| Qwen 流式在 Windows 不可用 | `get_runtime_capabilities()` 在 Windows 直接把流式判为不支持，需要 `remote_bridge` + 外部桥接服务（`scripts/qwen_streaming_service.py`） |
| 在线 provider 不需要「开启模型」 | 豆包走 `model_service_mode="online"`，凭据配齐即 `online_ready=True` |
| Node 内置 WebSocket 连不上豆包 | undici 的 `new WebSocket(url, options)` 会忽略 headers（实测 401/403）；Python 侧已用 `websockets.sync` + `additional_headers`，别改回 `websocket-client` 之类 |
| 打包漏 `scripts/` | `electron-builder.yml` 的 `extraResources` 没有 `scripts/`，而后端会 `uvicorn scripts.qwen_streaming_service:app`；补打包清单时记得一起加 |
| `Settings` 里若干字段零读取 | `supported_formats` / `chunk_duration` / `target_sample_rate` / `target_channels` 无人读（真实生效的是 `AudioProcessor` 类常量）；`MAX_UPLOAD_SIZE` 在限流里生效但 `validate_audio` 用硬编码 500MB |
| 死代码不要当参考 | `useWebSocket()`（`src/composables/useWebSocket.js`）、`processor.cleanup_temp_files/cleanup_file`、`convert_to_wav`、`chunk_audio` 整条链路、`ASRCapabilityError`、`reset_record_indexes` 等目前**无调用方**，别照抄 |
| 提示词文案不要顺手改 | `src/composables/recorder/statusCopy.mjs` 里真流式那句是"复制**或**粘贴失败"，被 `tests/test_recorder_status_copy.mjs` 锁死；要统一得同时改测试 |
| `writer.lock` 冲突 | 提示"另一个 ATT 后端进程正在写同一份数据目录"时，先关掉另一个后端/容器，或临时 `ATT_ALLOW_MULTI_WRITER=1`（会丢数据风险） |

---

## 5. 当前已知缺陷（不要"顺手修一半"）

完整清单、证据与「会导致什么」见 `docs/已知问题.md`。最影响改动的四条：

1. 统一浮层无入口（要么补入口，要么整体下线，别只改一半）——**现状等于功能不存在**，
   且划词翻译不可达 → 生词本永远为空。
2. `--win` 打包在 nsis 阶段失败（`release/` 只有 `win-unpacked`）——**产不出安装包**；
   打包版还缺 `scripts/`，本地流式桥接起不来。`win.target` 需收敛为 `portable`
   或先裁剪 `python-dist` 载荷。
3. 多段录音最后一片上传失败会卡住：`src/composables/useRecorder.js:599` 的 `recorder.onstop` 没有 catch，
   `uploadRealtimeSegment` 只有 `try/finally`（`:958`），异常会让 `:1115` 的收尾永不执行 →
   **界面永久停在"正在转写"、后续录音无法开始，且这段录音实际丢失**。
   全局 WS 有**固定 3s 重连**（`src/composables/useWebSocket.js:111` 的 `scheduleGlobalReconnect`，
   由 `onclose` 触发，另有 30s 心跳），但无退避、无重试上限、`onerror` 不触发，
   且收尾只由 WS 事件驱动、没有超时降级。
4. LLM provider（`app/llm/providers/*.py`）构造客户端时没传 `timeout`，SDK 默认 600s →
   **优化请求最长挂 10 分钟**，界面一直转圈，记录停在 `optimizing`。

---

## 6. 常见任务的做法

| 任务 | 步骤 |
| --- | --- |
| 新增一个 HTTP 端点 | 在 `app/api/routers/<域>.py` 加路由（只做解析/组装）→ 业务写进 `app/services/` 或对应域 → 在 `tests/test_api_*.py` 补用例 |
| 新增 ASR provider | 实现 `ASRProvider`（必须）与 `StreamingASRProvider`（可选）→ 在 `app/stt/models.py` 加枚举与配置字段 → `app/stt/factory.py` 装配 → `app/services/streaming_session_manager.py` 分派（若支持流式）→ 在 `app/api/schemas.py` 与设置页补配置项 |
| 新增设置项 | `app/config.py` 的 `UserSettings.DEFAULT_SETTINGS` + `app/api/schemas.py` 的 `UserSettingsUpdate`（两处必须同时加，有契约测试）→ 需要触发重建时登记进 `settings_service.MODEL_RUNTIME_KEYS` → 设置页表单 + `src/stores/settings.js` 默认值 |
| 改持久化格式 | 先看 `app/repositories/json_store.py` 的宽容读策略；`Record.from_dict` 这类模型要有缺字段兜底（旧数据要能读） |
| 改录音流程 | 纯逻辑抽到 `src/composables/recorder/*.mjs` 并补 Node 用例；注意"副作用先置位、善后在 await 之后"的历史缺陷模式 |
| 改前端文案 | 先在 `tests/test_recorder_*.mjs` 里确认有没有被锁死 |

---

## 7. 文档索引

`README.md`（门面与快速开始）、`docs/使用说明.md`、`docs/架构说明.md`、
`docs/配置说明.md`、`docs/API.md`、`docs/ASR接入.md`、`docs/开发与运维.md`、
`docs/已知问题.md`。
