# ASR 接入说明

本文说明本项目**语音识别（ASR）的四条接入链路**、如何切换与验证，以及如何新增一个 provider。
所有事实来自代码：`app/stt/`、`app/services/streaming_session_manager.py`、`app/config.py`。

---

## 1. 抽象与 provider 对比

ASR 层有两个接口（`app/stt/`）：

| 接口 | 文件 | 用途 |
| --- | --- | --- |
| `ASRProvider` | `app/stt/base.py` | 批量转写：`transcribe(audio_path)`，单段/多段录音与导入音频都走它 |
| `StreamingASRProvider` / `StreamingASRSession` | `app/stt/streaming_base.py` | 流式会话：`create_session()` → `append_audio(pcm16k)` → `finish()` |

能力通过 `ASRCapabilities` / `ASRRuntimeCapabilities` 声明（`app/stt/models.py`），
`get_runtime_capabilities(recording_mode)` 决定前端能否用某个模式：

| provider | 批量 | 流式 | `model_service_mode` | 是否需要预热 | 备注 |
| --- | --- | --- | --- | --- | --- |
| `whisper` | ✅ | ❌ | `provider` | 需要（点「开启模型」） | faster-whisper 本地推理 |
| `qwen_asr` | ✅ | ✅ | `provider` / `managed_streaming_bridge` | 需要 | 流式依赖 vLLM；**Windows 上流式不可用**，须走远端桥接 |
| `doubao_asr` | ✅ | ✅ | `online` | **不需要** | 火山引擎在线识别，凭据配齐即就绪 |
| `chatgpt_asr` | ✅ | ❌ | `online` | **不需要** | ChatGPT 听写接口（在线批量），见 §6 |

`model_service_mode` 的语义：

- `provider`：后端进程内加载模型（Whisper / Qwen 批量）；
- `managed_streaming_bridge`：由 `app/services/streaming_bridge_target.py` 管理一个流式桥接服务
  （本地子进程或远端容器），Qwen 流式走这条；
- `online`：没有本地模型，`online_ready=True` 就代表可用（豆包）。

---

## 2. 如何选择与配置 provider

优先级（`app/stt/factory.py` 的 `get_asr_provider_config()`）：

```text
运行时参数（测试/内部调用） > data/settings.json（设置页） > .env > 代码默认值
```

也就是说**设置页选中的 provider 会覆盖 `.env` 的 `ASR_PROVIDER`**。
切换后 `PUT /api/settings` 会卸载旧模型服务并重建 provider（`app/services/settings_service.py`
的 `MODEL_RUNTIME_KEYS` / `PROVIDER_REBUILD_KEYS`）。

---

## 3. Whisper（faster-whisper，本地）

| 配置项 | 环境变量 | 默认值 | 说明 |
| --- | --- | --- | --- |
| 模型 | `WHISPER_MODEL` | `large-v3` | tiny / base / small / medium / large-v3 |
| 设备 | `WHISPER_DEVICE` | `auto` | auto / cuda / cpu |
| 计算类型 | `WHISPER_COMPUTE_TYPE` | `int8` | CUDA 建议 `float16`，CPU 用 `int8` |
| 模型缓存 | — | `models/faster_whisper` | `app/config.py` 的 `faster_whisper_models_path` |

- 只支持批量转写，`capabilities().streaming = False`；选它时录音模式只能用「单段」或「多段」。
- 首次「开启模型」会下载模型到 `models/faster_whisper/`。

---

## 4. Qwen3-ASR（本地 vLLM 或远端桥接）

### 4.1 两种后端

`app/stt/providers/qwen_streaming_runtime_provider.py` 的 `QwenStreamingProvider` 按
`QWEN_STREAMING_BACKEND` 选择：

| backend | 判定 | 实现 |
| --- | --- | --- |
| `auto`（默认） | 配了 `QWEN_STREAMING_BASE_URL` 就走远端，否则本地 | — |
| `local_vllm` | 在主 FastAPI 进程内加载 vLLM 模型 | `QwenLocalVLLMStreamingProvider` |
| `remote_bridge` | 调用外部桥接服务 | `QwenRemoteBridgeStreamingProvider` |

**Windows 限制**：`supports_local_vllm_streaming()` 在 Windows 恒为 False，
`get_runtime_capabilities()` 直接把流式判为「不支持」，前端会提示切换单段/多段。
要在 Windows 上用 Qwen 流式，必须跑桥接服务并设置 `QWEN_STREAMING_BASE_URL`。

### 4.2 桥接服务的 HTTP 契约

由 `scripts/qwen_streaming_service.py` 提供（`npm run qwen:streaming:uv` 启动，端口 9000；
npm 脚本绑 `0.0.0.0`，后端「本地托管」模式用 `127.0.0.1:9000`）：

| 方法 | 路径 | 用途 |
| --- | --- | --- |
| GET | `/api/health` | `{ready, loaded, loading, error, model_name}` |
| POST | `/api/preload` | 触发模型加载（可能耗时数分钟） |
| POST | `/api/start` | `{language, context}` → `{session_id, language}` |
| POST | `/api/chunk/{session_id}` | body 为 16k 单声道 PCM16 → `{text, language}` |
| POST | `/api/finish/{session_id}` | 收尾 → `{text, language}` |
| DELETE | `/api/session/{session_id}` | 取消会话 |

### 4.3 本地 vLLM 参数

`QWEN_STREAMING_GPU_MEMORY_UTILIZATION`（默认 0.85）、`QWEN_STREAMING_MAX_MODEL_LEN`（16384）、
`QWEN_STREAMING_CHUNK_SIZE_SEC`（2.0）、`QWEN_STREAMING_UNFIXED_CHUNK_NUM`（2）、
`QWEN_STREAMING_UNFIXED_TOKEN_NUM`（5）。

> 注意：这些值由**桥接脚本**直接 `os.getenv` 读取，`app/config.py` 里的同名字段当前无人读取
> （见 `docs/配置说明.md` 的「写了不生效」一节）。

---

## 5. 豆包在线流式 ASR（火山引擎 · SAUC 协议）

### 5.1 接入参数

| 项 | 值 |
| --- | --- |
| 接入点 | `wss://openspeech.bytedance.com/api/v3/sauc/bigmodel`（`DOUBAO_ASR_ENDPOINT`） |
| 鉴权 | 握手头 `X-Api-Key`（Access Token）、`X-Api-Resource-Id`、`X-Api-Connect-Id` |
| 资源 ID | `volc.seedasr.sauc.duration`（ASR 2.0 · 时长，默认）/ `volc.seedasr.sauc.concurrent` / `volc.bigasr.sauc.duration`（1.0）/ `volc.bigasr.sauc.concurrent` |
| 音频 | PCM / 16 kHz / 单声道 / 16 bit，200 ms 一包（3200 采样 = 6400 字节） |
| 依赖 | 仅用现有 `websockets` 包的同步客户端（`websockets.sync`），未新增依赖 |

> **Access Token ≠ 方舟大模型 API Key**：两者体系不同，用错会直接 401。
> **Resource ID 必须与账号实际开通的产品匹配**，否则握手 403 且服务端不会告诉你换哪个：
> ASR 1.0 用 `volc.bigasr.sauc.*`，ASR 2.0 用 `volc.seedasr.sauc.*`。

### 5.2 二进制帧（SAUC）

4 字节头：`byte0 = 版本(4b)|头长(4b)`、`byte1 = 消息类型(4b)|flags(4b)`、
`byte2 = 序列化(4b)|压缩(4b)`、`byte3` 保留。

| 消息类型 | 值 | 方向 |
| --- | --- | --- |
| 全量客户端请求 | `0b0001` | 客户端 → 服务端 |
| 纯音频请求 | `0b0010` | 客户端 → 服务端 |
| 全量服务端响应 | `0b1001` | 服务端 → 客户端 |
| 错误响应 | `0b1111` | 服务端 → 客户端 |

flags：`0` 无序号 / `1` 正序号 / `2` 末包无序号 / `3` 末包负序号。
序列化：`0` RAW、`1` JSON；压缩：`0` 不压缩、`1` gzip。

```text
全量请求：[4B 头][payload_size uint32 BE][JSON]
音频帧：  [4B 头][payload_size uint32 BE][PCM]      ← 末包 flags=0b0010
结果帧：  [4B 头][sequence int32?][payload_size uint32 BE][JSON]
错误帧：  [4B 头][错误码 uint32 BE][信息长度 uint32 BE][信息 UTF-8]
```

> 实现上按官方布局解析错误帧，同时保留对「头后多一个标志字节」形态的容错
> （`_parse_error_frame` 两种偏移都试，取长度吻合且错误码合理的那个）。
> 响应是否 gzip 一律**看头**，不看请求用了什么；错误帧解压失败按原文处理。

### 5.3 请求体要点

```jsonc
{
  "user": { "uid": "att-desktop-xxxxxxxx" },
  "audio": { "format": "pcm", "rate": 16000, "bits": 16, "channel": 1, "language": "zh-CN" },
  "request": {
    "model_name": "bigmodel",
    "enable_itn": true, "enable_punc": true, "enable_ddc": false,
    "result_type": "full",        // 必须 full：single 只跟随当前句且不补发 final，会静默丢句
    "show_utterances": true,      // 必须 true：definite 判据依赖句级信息
    "corpus": { "context": "{\"context_type\":\"dialog_ctx\",\"context_data\":[{\"text\":\"…\"}]}" }
  }
}
```

- `corpus.context` 是**字符串**，内容是 JSON；直接塞纯文本会被判成 `45000001` 并直接断连。
  续录时把已有文字稿按**码点**截尾到 800 字符（`ASR_CONTEXT_MAX_CHARS`）后传入。
- `end_window_size`（毫秒，0 = 关闭）：开启后服务端会在该静音时长后输出 `definite`，
  实时字幕能更早定稿；`force_to_speech_time` 配合它使用。

### 5.4 结果解析：definite 连续前缀

服务端返回的是**整场累积文本**，同一句话会反复下发，因此不能「text 变了就追加」：

```text
utterances = [已定句1, 已定句2, …, 进行中句N]
prefix = utterances 从头开始连续 definite 的个数
→ 新确认的句子 = utterances[len(已确认):prefix]
→ 中间结果     = utterances[prefix:] 拼接
→ 展示文本     = 已确认拼接 + 中间结果
```

三条硬规则（`advance_definite_prefix()`）：

1. 每一帧都走前缀推进，**不做** `text === lastText` 短路——句子定稿时累积文本可能不变，
   短路会把定稿事件整个吞掉（表现为全程零 final）；
2. 拿不到句级信息（`utterances` 为空）时不确认，只更新中间结果；
3. 服务端回退（前缀变短）时同步收缩已确认列表，避免重复提交。

### 5.5 密钥流转（硬约束）

```text
设置页输入（明文，仅内存）
  → PUT /api/settings { doubao_asr_api_key: "..." }
  → 加密写入 data/settings.json: doubao_asr_api_key_encrypted
  → GET /api/settings 只返回 doubao_asr_api_key_set: true/false
  → factory 解密 → 只用于构造握手头（不进日志、不进异常文案）
```

- 空串 = 「本次不修改」；清除必须显式 `doubao_asr_api_key_clear: true`；
- 环境变量 `DOUBAO_ASR_API_KEY` 作为兜底，优先级低于设置页保存的密文。

### 5.6 错误归一化

三类来源统一成 `SaucSessionError`（带 `kind` / `retryable` / `next_action` / `provider_code`），
文案直接告诉用户是「去修复」还是「立即重试」：

| 来源 | 例子 |
| --- | --- |
| 服务端错误帧 | `45000001` 参数无效（protocol，不重试）；`45000002` 音频为空；`45000081` 等包超时（network，重试）；`45000151` 音频格式错；`55000031` 服务过载（quota）；`55000000–55999999` 服务端内部错误（server，重试） |
| 握手 HTTP 状态 | `401` Token 无效；`403` Resource ID 与开通产品不匹配；`404` 接入点错误；`429` 频率限制；`5xx` 服务端异常 |
| WebSocket 关闭码 | `1002/1003/1007/1008/4400–4499` → protocol，不重试；`1011–1013` → server，重试；其余（`1006`/`1001`/`1015`…）→ network，重试；`1000` 正常关闭（常见于长时间静音被 VAD 关会话） |

> 服务端拒绝请求时常常**不发错误帧、直接断连**，所以关闭码必须纳入归一化，
> 否则「请求被拒」会被当成「网络抖动」反复重试。

### 5.7 推流与收尾节奏

- 每包推完等待服务端结果（默认 0.1s），把「已确认句 + 未定稿尾巴」作为整场文本回给前端；
- `finish()` 发末包后等「末包标记 / 连接关闭 / 静默 1.2s」三者之一，硬超时 6s，避免停止录音时卡住；
- 批量转写（单段/多段）复用同一条会话：快速推流 + 每 25 包清一次接收队列，防止反向背压。

### 5.8 验证方式

```powershell
$env:DOUBAO_ASR_API_KEY = "<火山语音技术 Access Token>"
uv run python scripts/probe_doubao_asr.py --wav test_audio.wav
uv run python scripts/probe_doubao_asr.py --wav x.wav --fast           # 无间隔快速推送
$env:DOUBAO_ASR_RESOURCE_ID = "volc.bigasr.sauc.duration"              # 换 ASR 1.0 资源 ID
```

脚本复用 App 内同一套 provider 代码，输出握手 log id、逐帧文本与 6 条判据：
握手成功 / 收到中间结果 / 收到已确认结果 / 中间与最终可区分 /
**首句回显 ≤ 2s（按首个中间结果算，不是最终确认）** / 计费时长与音频时长一致（±5%）。

也可用健康检查确认运行时状态：

```powershell
Invoke-RestMethod "http://127.0.0.1:8000/api/health?probe_runtime=true" |
  Select-Object asr_provider, asr_model, streaming_available, streaming_backend, model_service_ready, streaming_error
```

期望：`asr_provider=doubao_asr`、`asr_model=bigmodel`、`streaming_available=True`、
`model_service_ready=True`。缺 Token 时 `streaming_available=False` 且 `streaming_error`
会写明「未配置火山引擎豆包语音 Access Token…」。

---

## 6. ChatGPT 听写 ASR（在线批量）

> 实现：`app/stt/providers/chatgpt_asr_provider.py`，调用逻辑移植自
> [Wangnov/codex-asr](https://github.com/Wangnov/codex-asr)（MIT License）。

### 6.1 接口与协议

| 项目 | 内容 |
| --- | --- |
| 端点 | `POST https://chatgpt.com/backend-api/transcribe`（可在设置页覆盖） |
| 鉴权 | **ChatGPT 网页账号 access token**（等同登录态，不是 platform API Key） |
| 请求 | `multipart/form-data`：`file`（按后缀推断 MIME）+ 可选 `language` |
| 响应 | `{"text": …, "asset_pointer": …, "asset_ttl": …, "asset_format": …}`，只取 `text` |
| 限制 | 非流式、无时间戳；建议单文件 < 11 分钟；支持 wav/mp3/m4a/mp4/flac/ogg/webm |

鉴权头四件套（缺一会被边缘风控拦截成 403 HTML 页）：

```text
Authorization: Bearer <token>
originator: Codex Desktop
User-Agent: Codex Desktop/26.429.30905 (windows; x86_64)
ChatGPT-Account-Id: <从 JWT 的 https://api.openai.com/auth.chatgpt_account_id 提取>
```

`ChatGPT-Account-Id` 由 provider 从令牌里自动提取（`account_id_from_access_token`），
提取失败时省略该头。粘贴令牌时带 `Bearer ` 前缀也能识别（会剥掉）。

### 6.2 密钥流转（硬约束）

与豆包 Access Token 完全同一套约定（`settings_service.SECRET_SPECS`）：

- 明文只出现在 `PUT /api/settings` 请求里，落盘前加密为
  `chatgpt_asr_api_key_encrypted`（Fernet，密钥在 `data/.encryption_key`）；
- `GET /api/settings` 只回报 `chatgpt_asr_api_key_set: bool`；
- 解析优先级：用户设置密文 > `CHATGPT_ASR_API_KEY` 环境变量；
- 令牌不进日志与异常文案（401/403 的提示只说「令牌无效/被风控」，不回显内容）。

### 6.3 设置项与重建

| 设置键 | 默认值 | 说明 |
| --- | --- | --- |
| `chatgpt_asr_api_key` | （只写） | 明文令牌，写入即加密；`chatgpt_asr_api_key_clear=true` 显式清除 |
| `chatgpt_asr_endpoint` | `https://chatgpt.com/backend-api/transcribe` | 端点覆盖 |
| `chatgpt_asr_timeout_seconds` | `120` | 请求超时（10–600） |

三个键都登记在 `MODEL_RUNTIME_KEYS`：变更即卸载模型服务并重建 provider。

### 6.4 能力与录音模式

- `model_service_mode="online"`，令牌配齐即 `online_ready=True`，**不需要**点「开启模型」；
- 不支持流式：录音模式选「流式」时会提示切换单段/多段（`get_runtime_capabilities`）。

### 6.5 验证方式

```bash
# 进程内直调（不依赖 HTTP 层）：factory 装配 → 真实端点
CHATGPT_ASR_API_KEY=<token> uv run python -c "
import asyncio
from app.stt.factory import get_asr_provider_config, create_asr_provider
p = create_asr_provider(get_asr_provider_config({'asr_provider': 'chatgpt_asr'}))
print(asyncio.run(p.transcribe('test.wav', 'zh')).segments[0].text)
"
```

单测见 `tests/test_chatgpt_asr_provider.py`（鉴权头、状态码归一、空 text、工厂装配，不联网）。

### 6.6 风险须知

非官方逆向接口：OpenAI 改协议或加强风控后可能失效；令牌等同账号登录态，
只能存本地（本项目的加密落盘即为此设计）；频繁大量使用有账号风险，
重要场合请用官方 API 或本地模型。

---

## 7. 新增一个 ASR provider

1. 在 `app/stt/providers/` 新建实现：
   - 批量：继承 `ASRProvider`，实现 `provider_type` / `model_name` / `transcribe()`，
     需要时覆写 `capabilities()` 与 `get_runtime_capabilities()`；
   - 流式：继承 `StreamingASRProvider`，`create_session()` 返回一个 `StreamingASRSession`。
2. `app/stt/models.py`：在 `ASRProviderType` 加枚举值，在 `ASRProviderConfig` 加配置字段。
3. `app/stt/factory.py`：在 `get_asr_provider_config()` 组装配置（密钥要按既有优先级解析）、
   在 `create_asr_provider()` 里装配。
4. 若支持流式：在 `app/services/streaming_session_manager.py` 的 `_get_provider()` 里分派。
5. 设置侧：`app/api/schemas.py` 的 `ASRProviderName` 与字段白名单、`app/services/settings_service.py`
   的 provider 校验集合与 `MODEL_RUNTIME_KEYS`（密钥/接入点变化要重建 provider）。
6. 前端：`src/components/settings/SettingsPage.vue` 的 provider 选项与表单、
   `src/stores/settings.js` 默认值；若支持流式，还要在
   `src/composables/recorder/streamProtocol.mjs` 的 `STREAMING_ASR_PROVIDERS` 里登记。
7. 测试：协议/会话单测（假 socket，不联网）+ 工厂与设置项测试。

---

## 8. 故障排查

| 现象 | 原因与处理 |
| --- | --- |
| 401 / `鉴权失败` | Token 错或过期；确认用的是**火山语音技术**的 Access Token，不是方舟大模型 Key |
| 403 / `Resource ID 与账号开通的产品不匹配` | 按 1.0/2.0 逐个试 `volc.bigasr.sauc.*` / `volc.seedasr.sauc.*` |
| 开始录音即报「未配置 Access Token」 | 设置页未保存成功；`GET /api/health?probe_runtime=true` 看 `streaming_error` |
| 灰字定稿很慢 | 把设置页「强制断句静音时长」设为 200–800 ms（对应 `end_window_size`） |
| 「会话已被服务端关闭（可能是长时间静音）」 | 服务端 VAD 关会话；**豆包会话本身不做自动重连**（全局 WS 的重连与这条 ASR 连接无关），重开录音即可 |
| Qwen 流式在 Windows 不可用 | 改用 `remote_bridge` + `scripts/qwen_streaming_service.py`，或切换单段/多段 |
| ChatGPT 听写报 401 | 令牌过期（JWT 有 `exp`），重新从网页登录态获取并更新设置 |
| ChatGPT 听写报 403「被风控拦截」 | 边缘风控；稍后重试、更换网络，或重新登录获取新令牌 |
| ChatGPT 听写转写为空 | 音频里没有可识别语音（`text` 为空是合法结果），不是故障 |
| 改了 `.env` 的 `ASR_PROVIDER` 没生效 | 设置页/`data/settings.json` 优先级更高，改那里 |
