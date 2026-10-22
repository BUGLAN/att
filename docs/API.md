# API 接口说明

本文档中的全部事实由代码提取，主要来源：`app/main.py`、`app/api/routers/*.py`、
`app/clipboard/routes.py`、`app/api/schemas.py`、`app/api/uploads.py`、`app/services/*`。

> 代码与常识不符之处一律以代码为准，并在备注中标注。
> 2026-10 功能减法：统一浮层 / 生词本（`/api/wordbook/*`）/
> AI 动作（`/api/ai-actions/*`）/ 提示词写接口相关端点已删除，本文不再收录。

## 0. 基础信息

| 项 | 事实 | 出处 |
| --- | --- | --- |
| 默认监听 | `127.0.0.1:8000`（直接 `python -m app.main` 时，仅回环；可用 `BACKEND_HOST` / `ATT_BACKEND_PORT` / `PORT` 覆盖）；Electron 拉起时同样绑定 `127.0.0.1` | `app/main.py`（`__main__`）、`electron/python/manager.js:86,145-182` |
| 路由装配 | `app/main.py` 逐个 `include_router`：records / sessions / settings / prompts / llm / clipboard / transcribe-file | `app/main.py` |
| 聚合 router | `app/api/routes.py` 的聚合 router **不参与生产挂载**，只为测试与兼容再导出（且它只含 records/sessions/settings/prompts/llm 五个子路由，不含 clipboard） | `app/api/routes.py` |
| 前缀 | 各子路由自带 `prefix="/api"`；`clipboard` 为 `/api/clipboard` | `app/api/routers/*.py` |
| 静态资源 | `dist/assets` 存在时挂载到 `/assets` | `app/main.py:181-183` |
| 根路径 | `dist/index.html` 存在则返回该文件，否则返回 `{name, version, docs, health}` JSON | `app/main.py:186-198` |
| 鉴权 | **其余端点无任何鉴权**；例外：`/api/transcribe*` 在 `TRANSCRIBE_FILE_API_TOKEN` 非空时要求 `X-ATT-Token` 头（见 §8）。跨域隔离靠 CORS 白名单：`http://localhost:8090`、`http://127.0.0.1:8090`、`null` + 正则 `^https?://(localhost\|127\.0\.0\.1)(:\d+)?$`，且 `allow_credentials=False` | `app/main.py:151-165`、`app/api/routers/transcribe_file.py` |
| 请求校验失败 | 路由内 Pydantic 请求体校验失败由 FastAPI 自动返回 **422**（如查询参数的 `pattern`/`le`）；而 `PUT /api/settings` 的请求体校验被路由捕获后转成 **400** | `app/api/routers/records.py:179`、`app/services/settings_service.py:216-221` |

---

## 1. 系统与健康检查

| 方法 | 路径 | 用途 | 关键请求字段 | 关键响应字段 | 备注 |
| --- | --- | --- | --- | --- | --- |
| GET | `/` | 返回前端页面或服务信息 | — | 有 `dist/index.html` 时返回 HTML；否则 `{name, version, docs, health}` | `app/main.py:186-198` |
| GET | `/assets/*` | 前端静态资源 | — | 文件内容 | 仅当 `dist/assets` 存在时挂载 `app/main.py:181-183` |
| GET | `/api/health` | 健康检查 + 模型/流式状态 | query `probe_runtime`（bool，默认 `false`，为 `true` 时会真实探测流式 provider） | `status`、`runtime_environment`、`asr_provider`、`asr_model`、`deepseek_configured`、`ws_clients`、`qwen_streaming_*`（旧名）、`streaming_*`、`model_service_*`（新名，与旧名同源） | `app/api/routers/settings.py:36-45`、字段拼装在 `app/services/model_service.py:81-108` |

`probe_runtime=true` 只对 `qwen_asr` / `doubao_asr` 生效（`app/services/model_service.py:63-70`）。

---

## 2. 记录与音频（records）

| 方法 | 路径 | 用途 | 关键请求字段 | 关键响应字段 | 备注 |
| --- | --- | --- | --- | --- | --- |
| POST | `/api/audio/upload` | 上传音频并（可选）异步转写 | multipart `file`；query `auto_transcribe`（默认 `true`） | `status`、`record_id`、`duration`、`transcription_status` | `app/api/routers/records.py:66-151`；无文件名 400；格式/时长非法 400；超限 413（见 §5.1） |
| GET | `/api/records` | 列出记录 | query `status`、`limit`（默认 200，上限 1000）、`offset`（≥0） | **记录数组**（不是 `{items,total}`） | `records.py:156-173`；注释明确「响应体保持为记录数组，前端契约不变」 |
| GET | `/api/records/search` | 按文本/收藏搜索 | query `q`、`search_type`（`all\|raw\|optimized`）、`favorite_only`、`page`（≥1）、`page_size`（1..200） | `items`、`total`、`page`、`page_size`、`total_pages` | `records.py:176-203`；内部先取最多 5000 条再内存分页 |
| GET | `/api/records/{record_id}` | 记录详情 | path `record_id` | `Record.to_dict()`：`id/created_at/updated_at/audio_path/duration/raw_text/optimized_text/status/processing_time/is_modified/is_favorite/error/revision` | `records.py:206-212`、`app/records/models.py:102-118` |
| GET | `/api/records/{record_id}/status` | 轮询转写状态（最小字段集） | path `record_id` | `record_id`、`status`、`raw_text`（仅 transcribed/optimizing/optimized 时非 null）、`optimized_text`（仅 optimized）、`duration`、`processing_time`、`is_favorite` | `records.py:215-239` |
| PUT | `/api/records/{record_id}` | 修改记录文本/收藏 | body：`raw_text`、`optimized_text`、`is_modified`、`is_favorite`、`revision`（均可选） | 更新后的 `Record.to_dict()` | `records.py:242-269`；传 `revision` 且版本已变 → **409** |
| DELETE | `/api/records/{record_id}` | 删除记录及其音频 | path `record_id` | `{status:"deleted", record_id}` | `records.py:272-279` |
| GET | `/api/records/{record_id}/audio` | 播放音频 | path `record_id` | WAV 文件流（`media_type="audio/wav"`） | `records.py:282-304`；`audio_path` 为空或非普通文件 → 404 |
| POST | `/api/records/{record_id}/optimize` | 用 AI 优化文本（非流式，保留兼容） | query `style`（默认 `"default"`） | `record_id`、`original_text`、`optimized_text`、`processing_time`、`tokens_used`、`truncated` | `records.py`；无 `raw_text` → 400；优化异常 → **502**，同时广播 WS `optimization_failed`；失败文案已脱敏 |
| POST | `/api/records/{record_id}/optimize/stream` | 流式优化（NDJSON，推荐） | 无 body；path `record_id` | 200，`Content-Type: application/x-ndjson`；逐行事件：`{type:"start",record_id,original_text}`、`{type:"delta",text}`、`{type:"done",optimized_text,processing_time,tokens_used,truncated,degraded}`、`{type:"error",error,status,raw_text}` | 流式失败若发生在首条增量前，服务端会回退一次非流式 `optimize()`；客户端断开/取消时记录复位为 `transcribed`；HTTP 调用不重复广播增量 |
| POST | `/api/records/{record_id}/optimize/cancel` | 取消正在执行的流式优化 | path `record_id` | `{record_id, cancelled}` | 手动 HTTP 流与后台自动优化共用任务表；无活动任务返回 `cancelled:false` |
| GET | `/api/records/{record_id}/export` | 导出单条记录 | query `format`（`txt`/`json`，默认 `txt`）、`use_optimized`（默认 `true`） | `PlainTextResponse`，带 `Content-Disposition: attachment; filename=record_<id>.<format>` | `records.py:382-418`；`format` 非法 → 400（`ExportFormat` 仅 `txt`/`json`，`app/records/models.py:21-24`） |

---

## 3. 多段录音会话（sessions）

| 方法 | 路径 | 用途 | 关键请求字段 | 关键响应字段 | 备注 |
| --- | --- | --- | --- | --- | --- |
| POST | `/api/sessions` | 新建多段录音会话 | — | `session_id`、`status`、`created_at` | `app/api/routers/sessions.py:44-52` |
| GET | `/api/sessions/{session_id}` | 会话详情 | path `session_id` | `RecordingSession.to_dict()` | `sessions.py:55-61`；不存在 404 |
| POST | `/api/sessions/{session_id}/segments` | 上传一个片段 | multipart `file`（服务端强制按 `segment_<uuid>.webm` 命名保存） | `segment_id`、`sequence`、`duration`、`status` | `sessions.py:64-117`；会话不存在 404；格式/时长非法 400；超限 413 |
| POST | `/api/sessions/{session_id}/merge` | 合并全部片段为一条记录 | path `session_id` | `session_id`、`record_id`、`merged_text`、`optimized_text`、`record`、`total_duration`、`segment_count` | `sessions.py:120-195`；整段转写失败会回退到分段拼接文本（不报错）；`auto_optimize=true` 时顺带优化 |
| DELETE | `/api/sessions/{session_id}` | 删除会话及其片段 | path `session_id` | `{status:"deleted", session_id}` | `sessions.py:198-204` |
| WS | `/api/ws` | 全局状态广播通道 | 见 §4.2 | 见 §4.2 | `sessions.py:209-241` |
| WS | `/api/asr/stream` | 真流式 ASR | 见 §4.1 | 见 §4.1 | `sessions.py:243-352` |

---

## 4. WebSocket 协议

### 4.1 `/api/asr/stream`（真流式 ASR）

连接后服务端立即 `accept()`（`sessions.py:246`），不发送握手消息。

**客户端 → 服务端**

| 帧类型 | 内容 | 说明 |
| --- | --- | --- |
| 文本 JSON | `{"type":"stream_start","language":"zh","context":""}` | `language` 默认 `"zh"`，`context` 默认 `""`（`sessions.py:269-280`）。前端实际只发 `type` + `language`（`src/composables/recorder/streamProtocol.mjs:112-117`） |
| 文本 JSON | `{"type":"stream_finish"}` | 结束并落库（`sessions.py:282-293`）；未启动会话时报 `流式会话尚未启动` |
| 文本 JSON | `{"type":"stream_cancel"}` | 取消当前会话（`sessions.py:295-303`）；即使没有活动会话也回 `stream_cancelled` |
| 文本 JSON | `{"type":"ping"}` | 服务端回 `{"type":"pong"}`（无 `data`/`timestamp`）（`sessions.py:305-307`） |
| 二进制帧 | 原始 **16 kHz / 单声道 / int16 小端 PCM** | 直接作为 WAV 帧写入磁盘（`app/services/streaming_session_manager.py:92-98, 148-150`）。**服务端不做格式校验**；16 kHz 由前端强制（`src/composables/useRecorder.js:847-850`） |
| 其他文本 | 任意 `type` | 抛 `ValueError(f"未知的流式命令: {command}")` → 转成 `stream_error` 并关闭连接（`sessions.py:309`） |

**服务端 → 客户端**（`timestamp` 均为 `datetime.now().isoformat()`，`sessions.py:385-387`）

| 帧类型 | 载荷 | 触发时机 | 出处 |
| --- | --- | --- | --- |
| `stream_ready` | `{"type":"stream_ready","data":{"session_id","language","sample_rate":16000},"timestamp"}` | 收到 `stream_start` 后 | `sessions.py:275-279`、`streaming_session_manager.py:124-128` |
| `stream_partial` | `{"type":"stream_partial","data":{"text","language","duration"},"timestamp"}` | **每个**二进制音频帧都会回一条 | `sessions.py:315-320`、`streaming_session_manager.py:157-161` |
| `stream_finished` | `{"type":"stream_finished","data":{"record","raw_text","language","duration", "optimized_text"?},"timestamp"}` | 收到 `stream_finish` 且识别到有效文本后；`optimized_text` 仅在 `auto_optimize=true` 且优化成功时出现 | `sessions.py:286-292`、`streaming_session_manager.py:194-224` |
| `stream_cancelled` | `{"type":"stream_cancelled","timestamp"}`（**无 `data` 字段**） | 收到 `stream_cancel` 后 | `sessions.py:299-302` |
| `stream_error` | `{"type":"stream_error","data":{"error":"<异常文本>"},"timestamp"}`，随后服务端 `close()` | 任意未捕获异常（含未知命令、未启动就发音频、未识别到文本等） | `sessions.py:324-338` |

其它契约：

- 音频会话为空文本时 `finish_session` 抛 `未识别到有效语音文本`（`streaming_session_manager.py:185`），最终表现为 `stream_error`。
- 客户端断开（`websocket.disconnect`）时跳出循环，`finally` 统一 `cancel_session`，避免会话与临时 WAV 泄漏（`sessions.py:260-263, 339-352`）。
- 当前 provider 不支持真流式时，`_get_provider()` 抛 `RuntimeError`（`streaming_session_manager.py:75-79`）→ `stream_error`。

### 4.2 `/api/ws`（全局状态通道）

服务端**只接受文本 JSON**（`await websocket.receive_json()`，`sessions.py:224`）。

**客户端 → 服务端**

| `type` | 字段 | 行为 |
| --- | --- | --- |
| `status_update` | `recording`、`transcribing`、`record_id`、`duration` | 更新共享状态并向**所有**连接广播 `status`（`sessions.py:365-372`、`app/services/websocket.py:102-106`） |
| `ping` | — | 仅向该连接回 `{"type":"pong"}`（`sessions.py:374-376`） |
| 其他 | — | 只记一条 warning，**不回任何消息**（`sessions.py:378-382`） |

**服务端 → 客户端**

| `type` | 载荷 | 触发时机 | 出处 |
| --- | --- | --- | --- |
| `connected` | `{type, client_id, status:{recording,transcribing,record_id,duration,connected_clients}, timestamp}`，`timestamp` 为**毫秒整数** | 连接建立后仅发给新客户端 | `websocket.py:42-47` |
| `status` | `{type:"status", recording, transcribing, record_id, duration, connected_clients, timestamp}`，`timestamp` 为**毫秒整数** | 连接/断开、`status_update` 后广播给所有客户端 | `websocket.py:95-100`、`sessions.py:229-231` |
| `pong` | `{type:"pong"}` | 收到 `ping` | `sessions.py:375` |
| `optimization_started` | `{type, data:{record_id}, timestamp}`，ISO 字符串 | 自动优化开始（单段/流式/多段 merge） | `app/services/optimization_stream_service.py` |
| `optimization_delta` | `{type, data:{record_id, text}, timestamp}`，ISO 字符串；`text` 为增量片段 | 流式优化每收到一块内容 | 同上 |
| `optimization_completed` | `{type, data:{record_id, optimized_text, processing_time, tokens_used, truncated}, timestamp}`，ISO 字符串 | 自动优化成功落盘后 | 同上 |
| `optimization_cancelled` | `{type, data:{record_id, raw_text}, timestamp}`，ISO 字符串 | 自动优化被取消且记录复位后 | 同上 |
| `optimization_failed` | `{type, data:{record_id, error, status, text}, timestamp}`，此处 `timestamp` 为 **ISO 字符串**（与上面两种毫秒整数不一致） | 自动优化失败 / 非流式 `/optimize` 失败时广播 | `app/services/optimization_stream_service.py`；`text` 为原文兜底 |
| ~~`optimization_degraded`~~ | —— | **已随 LLM v2 移除**（2026-10-22）：`.env` 内置 key 兜底删除后不再有「降级成功」状态，配置失败直接走 `optimization_failed` | —— |

---

## 5. 设置与模型服务（settings / model-service）

### 5.1 端点表

| 方法 | 路径 | 用途 | 关键请求字段 | 关键响应字段 | 备注 |
| --- | --- | --- | --- | --- | --- |
| GET | `/api/settings` | 读取用户设置 | — | `data/settings.json` 全量键 + 派生标记 `doubao_asr_api_key_set` | `settings.py:94-97`、`app/services/settings_service.py:171-196` |
| PUT | `/api/settings` | 更新用户设置 | 请求体为设置键字典（白名单见 `app/api/schemas.py:31-97`） | 更新后的完整设置（同样脱敏） | `settings.py:100-118`；未知键静默丢弃；校验失败 400 |
| GET | `/api/model-service` | 模型服务状态 | — | `managed`、`provider_type`、`model_name`、`recording_mode`、`loading`、`running`、`ready`、`error`、`pid`、`base_url`、`streaming_*`、`model_service_*`、`health`、`components` | `settings.py:50-53`、`app/services/model_service_manager.py:141-166` |
| POST | `/api/model-service/start` | 启动/预热模型服务 | body `{"wait_until_ready": true}`（默认 `true`） | 同状态结构 | `settings.py:56-62`；异常 → 500 |
| POST | `/api/model-service/stop` | 停止模型服务 | — | 同状态结构 | `settings.py:65-71`；异常 → 500 |
| GET | `/api/streaming/service` | **兼容别名**，等价 `GET /api/model-service` | — | 同上 | `settings.py:74-77` |
| POST | `/api/streaming/service/start` | **兼容别名**，等价 `POST /api/model-service/start` | 同上 | 同上 | `settings.py:80-83` |
| POST | `/api/streaming/service/stop` | **兼容别名**，等价 `POST /api/model-service/stop` | — | 同上 | `settings.py:86-89` |

> `/api/streaming/service*` 与 `/api/model-service*` 是同一实现的兼容别名（`settings.py:9` 注释、`app/services/model_service.py:5-6`）；前端只用后者。

### 5.2 上传体积上限

| 层 | 上限 | 触发结果 | 出处 |
| --- | --- | --- | --- |
| 请求体限流（上传/片段共用） | `settings.max_upload_size`，默认 `500 * 1024 * 1024` | **413**，detail 为「上传文件过大：请求体超过上限 X MB」或「…音频超过上限 X MB」 | `app/api/routers/records.py:95`、`app/api/routers/sessions.py:79`、`app/api/uploads.py:23-45`、默认值 `app/config.py:356` |
| `AudioProcessor.validate_audio` | **硬编码** `MAX_FILE_SIZE = 500 * 1024 * 1024` | **400**，detail 为「File too large: N bytes. Max: …」 | `app/audio/processor.py:31, 72-76` |

限流实现先看 `Content-Length` 快速拒绝，再按 1 MiB 分块累计，内存上界为 `limit + 1 MiB`（`app/api/uploads.py:14-15, 25-44`）。
若把 `MAX_UPLOAD_SIZE` 调大到 500 MB 以上，500 MB～新上限之间的文件会改由 `validate_audio` 以 **400** 拒绝。

### 5.3 `PUT /api/settings` 的校验失败行为

请求体先经 `UserSettingsUpdate`（`app/api/schemas.py:31-97`）校验，规则：

1. **未知键静默丢弃**（`model_config = ConfigDict(extra="ignore")`，`schemas.py:38`）——Electron 会整份提交快照，因此这是有意保留的宽松策略。
2. **显式传 `null` 视为「本次未提供」**，不会写进 `settings.json`（`settings_service.py:223-228`）。
3. 类型 / 枚举 / 范围非法 → **400**，detail 形如 `设置项校验失败：silence_threshold 必须是数字`（`app/api/schemas.py:100-127`）。
4. 过滤后没有任何有效键 → **400** `No valid settings provided`（`settings_service.py:243-244`）。
5. `hotkey` 非法 → **400** `Invalid hotkey configuration`（`settings_service.py:230-231`）。
6. 需要重建 ASR provider 但运行时 provider 未初始化 → **500**；重建过程抛异常 → **500**（**设置已落盘**）（`settings_service.py:271-288`）。

写入后的副作用：模型运行时相关键变化会先 `model_service_manager.stop()` + 关闭流式会话管理器，再重建 provider（`settings_service.py:258-291`、`MODEL_RUNTIME_KEYS` 定义见 `settings_service.py:34-52`）。

### 5.4 `GET /api/settings` 的豆包密钥脱敏契约

- 响应中**不含** `doubao_asr_api_key_encrypted`（密文被剔除），只返回布尔 `doubao_asr_api_key_set`（`settings_service.py:133-145`）。
- `doubao_asr_api_key`（明文）从来不是响应字段，也**不会**落盘到 `settings.json`；写入时立即加密为 `doubao_asr_api_key_encrypted`（`settings_service.py:148-168`）。
- 空串或未传 = 保留已存密钥；`doubao_asr_api_key_clear=true` 才清除（`settings_service.py:157-168`）。
- `GET /api/settings` 默认纯读：只有「提示词预设 id 失效」或「浮层枚举值失效」这两类可修复键才会单次落盘（`settings_service.py:183-194`）。

---

## 6. 提示词预设（prompts）

| 方法 | 路径 | 用途 | 关键请求字段 | 关键响应字段 | 备注 |
| --- | --- | --- | --- | --- | --- |
| GET | `/api/prompts` | 列出全部预设 | — | `presets`（数组）、`default_preset_id`、`last_used_preset_id` | `app/api/routers/prompts.py`；注意 `last_used_preset_id` 实际回的是 `default_preset_id` 的归一化结果 |
| PUT | `/api/prompts/default` | 设为默认预设 | body `{"preset_id": "..."}` | `{status:"updated", default_preset_id}` | `prompts.py`；预设不存在 → 404 |

> **预设是只读的**：唯一真相源是仓库根目录 `prompts/*.txt`，预设 id 由文件名派生为
> `preset-<文件名去掉扩展名>`（`file_manager.py:17-18, 44-45`）。
> 历史上的 POST/PUT/DELETE 写端点（恒 400）与 GET 单条 / preview 已在功能减法中删除。

---

## 7. LLM 服务（llm，v2）

> 2026-10-22 重写：接入层换 LiteLLM，配置从「一层配置」拆成「供应商档案 + 模型条目」
> 两级（设计见 `docs/LLM配置模块重设计.md`）。旧 `/api/llm/configs*` 端点已整体移除。

| 方法 | 路径 | 用途 | 关键请求字段 | 关键响应字段 | 备注 |
| --- | --- | --- | --- | --- | --- |
| GET | `/api/llm/providers` | 服务商目录（协议前缀 + 元信息，**不含模型清单**） | — | `providers[]`：`id/name/litellm_prefix/default_base_url/requires_key/supports_model_listing/docs_url/notes/suggested_models` | `app/llm/registry.py` |
| GET | `/api/llm/state` | 档案 + 模型 + 默认绑定聚合视图（设置页首屏） | — | `profiles[]`、`models[]`、`default_model_id`、`fallback_model_id`、`skipped_entries[]` | `llm.py` GET state；**响应不含任何密钥字段** |
| GET / POST | `/api/llm/profiles` | 档案列表 / 新建 | POST：`provider`、`name`、`api_key`（明文，落盘前加密）、可选 `api_base/extra_headers/extra_query` | 安全序列化的档案对象（无密钥字段） | `ValueError` → 400（未知服务商 / custom 缺端点 / 名称空） |
| GET / PUT / DELETE | `/api/llm/profiles/{profile_id}` | 档案详情 / 更新 / 删除 | PUT 可选 `name/api_key/api_base/extra_*`（`api_key` 缺省 = 保持现有密钥） | 同上 | DELETE 级联其下模型；含默认模型 → **409** |
| POST | `/api/llm/profiles/{profile_id}/verify` | 验证已存档案（鉴权 + 尽力列模型） | — | `{ok, error_code, message, latency_ms, remote_models[]}` | 结果持久化到档案 `status`；验证失败也是 200 |
| POST | `/api/llm/profiles/{profile_id}/remote-models` | 拉取上游模型列表（OpenAI 兼容 `/models`） | — | `{profile_id, models[]}` | 上游不支持 → **409** `{manual_only: true, reason}` |
| POST | `/api/llm/verify-draft` | **保存前验证**（草稿密钥仅内存使用后即弃） | `provider`、可选 `api_key/api_base/extra_headers/model_name` | 同验证结构 | `model_name` 提供时验证到模型级 |
| GET / POST | `/api/llm/models` | 模型列表（可 `?profile_id=` 过滤）/ 批量新建 | POST：`items[]`：`profile_id`、`model`（裸名自动补前缀，含 `/` 原样透传）、可选 `display_name/params/set_default` | `{created[], skipped[{model, reason}]}` | 重复 / 档案缺失的条目进 `skipped`，不整批失败 |
| PUT / DELETE | `/api/llm/models/{model_id}` | 更新 / 删除模型 | PUT：可选 `display_name/model/params` | 模型对象 / `{status:"deleted"}` | 删除默认模型 → **409**；参数校验失败 → 400 |
| POST | `/api/llm/models/{model_id}/verify` | 单模型轻量验证（约 5 token 补全，超时 15s） | — | 验证结构 | 结果持久化到模型 `status` |
| POST | `/api/llm/models/{model_id}/activate` | 设为默认优化模型 | — | `{status:"updated", default_model_id}` | 即时生效，下一次优化请求即用新模型 |
| PUT | `/api/llm/settings` | 更新默认 / 备用模型绑定 | `default_model_id`、`fallback_model_id`（显式 `null` = 清除备用） | `{status:"updated", ...}` | 不存在的模型 → 404 |

`error_code` 分级（所有验证端点）：`ok / auth_failed / model_not_found / quota_exceeded / timeout / network_error / provider_error / unsupported_provider`；
`message` 一律经 `app/llm/security.py` 脱敏。模型串规则：`<litellm前缀>/<模型名>`（如 `deepseek/deepseek-flash`），
模型名由用户拉取或手输，**原样透传**，服务商发布新模型无需改代码。

---

## 8. 本地文件转写（transcribe-file）

面向外部采集脚本与 App 内「转文字」页的**本地文件 → 文字**接口。与 records 域完全解耦：
不建记录、不发 WebSocket；结果按**文件内容 sha256 + 引擎**缓存（v2 格式按文件聚合、
引擎分桶、校对变体挂 `{model}@{PROMPT_VERSION}`，落盘 `data/transcribe_file_cache.json`，
LRU 上限 `TRANSCRIBE_FILE_CACHE_SIZE` 按整文件条目淘汰），同一文件重复请求秒回。
业务实现在 `app/services/file_transcription_service.py`，LLM 校对管道在
`app/services/transcribe_proofread.py`，路由在 `app/api/routers/transcribe_file.py`。
完整设计见 [文件转写-多引擎与LLM校对-设计.md](文件转写-多引擎与LLM校对-设计.md)。

| 方法 | 路径 | 用途 | 关键请求字段 | 关键响应字段 | 备注 |
| --- | --- | --- | --- | --- | --- |
| POST | `/api/transcribe` | 转写本地音视频文件 | JSON：`path`（必填，本地绝对路径）、`language`（默认 `zh`）、`want_segments`（默认 `false`）、`engine`（`auto\|whisper\|qwen_asr\|doubao_asr`，默认 `auto`）、`proofread`（默认 `false`）、`proofread_model_id`（可选，缺省默认模型）、`proofread_options`（`batch_segments/batch_chars/context_chars`，可选） | 短文件同步 200：`text`、`duration_seconds`、`engine`、`cached`、`segments`（仅 `want_segments:true`）、`billing`（仅豆包）；`proofread=true` 或长文件 **202**：`{job_id}` | 视频容器先用 ffmpeg 抽 16kHz 单声道音轨（qwen 引擎除 wav 外一律抽轨）；qwen 长文件按 `TRANSCRIBE_FILE_QWEN_CHUNK_SECONDS` 静音分块拼接；豆包超 `TRANSCRIBE_FILE_DOUBAO_SESSION_SECONDS` 分段多会话 |
| GET | `/api/transcribe/jobs/{job_id}` | 轮询 202 任务 | path `job_id` | `status`（`running\|done\|failed`）、`stage`（`asr\|proofreading\|done`）、`progress`（校对阶段 `{window_done, window_total}`）、`result`（done 时同 200 响应体，校对时含 `raw_text` 与 `proofread` 元数据）、`error` | 任务状态只存内存，终态保留 30 分钟后清理；未知/过期任务 404 |

行为约定：

- **时长上限**：超过 `TRANSCRIBE_FILE_MAX_DURATION_SECONDS`（默认 7200 秒）返回 **413**；超过 `TRANSCRIBE_FILE_ASYNC_THRESHOLD_SECONDS`（默认 1800 秒）**或 `proofread=true`** 走 202 异步。
- **并发**：单并发闸门只包 ASR 执行段（校对不占闸门），忙时 **503** + `Retry-After: <秒>`（`TRANSCRIBE_FILE_BUSY_RETRY_AFTER_SECONDS`），不排队堆积内存。
- **引擎**：`engine=auto`=本地优先（全局配置指向豆包时自动回落 whisper，日志告警）；显式 `doubao_asr` 才走在线（按时长计费，响应带 `billing`；`want_segments` 对豆包只返回整段单 segment）。
- **LLM 校对**（`proofread=true`）：滑动窗口分批（默认 40 段且 4000 字，段边界；可选前文 200 字重叠）→ 每窗「LLM → 验证（键完全匹配 + 相似度 ≥0.7，短文本 ≥0.3）→ 反馈重试 ≤3」→ 失败批保留原文计入 `proofread.fallback_batches`；网关整体不可用降级 `applied=false` 照常交付 ASR 原文。LLM 走 v2 网关，窗口并发 `TRANSCRIBE_FILE_PROOF_CONCURRENCY`。
- **鉴权**：`TRANSCRIBE_FILE_API_TOKEN` 非空时要求请求头 `X-ATT-Token` 精确匹配（`secrets.compare_digest`），不匹配 **401**；默认空 = 仅回环监听、不鉴权。
- **错误码**：404 文件不存在/相对路径/任务号过期；415 扩展名不识别、ffprobe 解码失败、ffmpeg 抽音轨失败；413 超时长上限；**409** 前置配置缺失（显式豆包但凭据未配置 / 开校对但无任何 LLM 模型，不可重试）；503 引擎忙/任务表满；422 engine 非法（Literal 校验）；500 其他内部错误（detail 带原因）。

---

## 9. 剪贴板（clipboard）

| 方法 | 路径 | 用途 | 关键请求字段 | 关键响应字段 | 备注 |
| --- | --- | --- | --- | --- | --- |
| GET | `/api/clipboard/items` | 列出条目 | query `query`、`item_type`（`text\|link\|code\|file\|image`）、`favorite_only`、`pinned_only`、`limit`（1..500，默认 100） | **条目数组** | `app/clipboard/routes.py:43-60` |
| GET | `/api/clipboard/stats` | 统计 | — | `total`、`today`、`favorites`、`pinned` | `routes.py:63-66`、`app/clipboard/manager.py:390-401` |
| POST | `/api/clipboard/items` | 新建条目 | `content`（必填，`min_length=1`）、`source`（默认 `manual`）、`item_type`、`title`、`summary`、`metadata` | 条目字典 | `routes.py:69-80` |
| GET | `/api/clipboard/items/{item_id}` | 读取条目 | path `item_id` | 条目字典 | `routes.py:83-89`；不存在 404 |
| GET | `/api/clipboard/items/{item_id}/image` | 读取图片内容 | path `item_id` | 图片文件流 | `routes.py:92-103`；无图片或路径越界（不在 `data/clipboard/images/` 内）一律 404 |
| PUT | `/api/clipboard/items/{item_id}` | 更新条目 | `content`、`title`、`summary`、`is_favorite`、`is_pinned`（均可选） | 条目字典 | `routes.py:106-119`；不存在 404 |
| DELETE | `/api/clipboard/items/{item_id}` | 删除条目 | path `item_id` | `{status:"deleted", item_id}` | `routes.py:122-128`；不存在 404 |

---

## 10. 响应状态码约定

| 状态码 | 出现场景 | 代码证据 |
| --- | --- | --- |
| **400** | ① 上传无文件名：`No filename provided`；② 音频校验失败（格式不支持 / 时长 < 0.5s / 空文件 / 超过硬编码 500MB）；③ 记录无 `raw_text` 却请求优化：`No text to optimize`；④ 导出 `format` 非 `txt`/`json`；⑤ `PUT /api/settings` 类型/枚举/范围校验失败、无有效键、热键非法；⑥ LLM 配置创建/更新异常、未知 provider | `records.py:92, 100-101, 319, 396-400`；`sessions.py:82-83`；`settings_service.py:216-244`；`llm.py:99-100, 117-118` |
| **404** | 资源不存在：记录、会话、提示词预设、LLM 配置、剪贴板条目/图片；以及记录音频路径为空或不是普通文件 | `records.py:211, 222, 267, 277, 287, 294, 299, 316, 392`；`sessions.py:60, 76, 125, 203`；`prompts.py`；`llm.py:69, 110, 127, 142, 157`；`clipboard/routes.py:88, 97, 101, 118, 127` |
| **409** | 记录乐观锁冲突：`PUT /api/records/{id}` 传了 `revision`，且该记录仍存在但版本已变（detail：`记录已被其他操作修改，请刷新后重试`） | `records.py:258-266` |
| **413** | 上传请求体或音频超过 `settings.max_upload_size`（`POST /api/audio/upload`、`POST /api/sessions/{id}/segments`） | `app/api/uploads.py:26-43` |
| **422** | FastAPI/Pydantic 对**未捕获**的请求体或查询参数校验失败自动返回（如 `search_type` 不在枚举内、`page_size > 200`） | `records.py:179-182`、`clipboard/routes.py:46-49` |
| **500** | ① `POST /api/model-service/start|stop` 内部异常；② `PUT /api/settings` 需重建 provider 但 provider 未初始化，或重建过程抛异常（设置已落盘） | `settings.py:62, 71`；`settings_service.py:273, 285-288` |
| **502** | `POST /api/records/{id}/optimize` 优化失败（detail：`文本优化失败：…`，并广播 WS `optimization_failed`） | `records.py:360-363` |

> 说明：文档里未列出的 4xx/5xx 只可能来自 FastAPI 自身（如路由不匹配的 404、方法不允许的 405）。

---

## 11. 待确认

- `GET /api/prompts` 的 `last_used_preset_id` 字段回传的是 `default_preset_id` 的归一化值（`prompts.py`），
  但 `PUT /api/prompts/default` 会同时写 `default_preset_id` 与 `last_used_preset_id`。
  这是**有意为之还是笔误**，代码中无注释说明，**待确认**。
