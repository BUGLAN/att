# 文件转写 · 多引擎与 LLM 校对 · 核心设计

> 状态：**已实施**（M1–M3 于 2026-10-22 落地：`5a47c09` 引擎分流+缓存v2+豆包、`62eb618` LLM 校对管道+前端、`629bd0d` Qwen 分块；实施偏差见各节【实施注】）
> 范围：`app/services/file_transcription_service.py`、`app/api/routers/transcribe_file.py`、`app/stt/providers/*`（适配层）、「转文字」页前端
> 关联：[API.md](API.md) §8（v1 契约）、[配置说明.md](配置说明.md) §1.9、外部需求文档 `E:\repo\研究\ATT转写接口需求.md`
> 交互原型：[原型/转文字-多引擎与LLM校对-原型.html](原型/转文字-多引擎与LLM校对-原型.html)（浏览器直接打开，可点击演示流程）
>
> **标记约定**：【已定】= 本文档拍板、按此实施；【待拍板】= 需要用户确认的开放项，集中在 §10。

---

## 0. 现状基线（v1，已上线）

以下能力**已经实现并通过回归**（14 个用例 + 契约基线 46 端点），本文档在其上扩展，不推翻：

| 能力 | 事实 |
| --- | --- |
| 端点 | `POST /api/transcribe`（同步 200 / 长文件 202）、`GET /api/transcribe/jobs/{job_id}` |
| 路径校验 | 绝对路径 + 文件存在 + 扩展名白名单（视频 8 种 / 音频 9 种），失败 404/415 |
| 时长闸门 | ffprobe 探测；> 7200s → 413；> 1800s → 202 后台任务（内存任务表，终态保留 30 分钟，上限 100 条） |
| 视频抽轨 | 视频容器 ffmpeg 抽 16kHz 单声道 wav；音频容器原样透传 |
| 缓存 | 内容 sha256 → `data/transcribe_file_cache.json`（LRU 200 条，`write_json_atomic` 原子写） |
| 并发 | 单并发闸门（asyncio.Lock），忙时 503 + Retry-After |
| 引擎 | **强制本地**：全局配置指向豆包时回落 whisper（v1 需求 §2.4.4「不走在 ASR」） |
| 鉴权 | 可选 `X-ATT-Token`（`TRANSCRIBE_FILE_API_TOKEN`） |
| 前端 | 「转文字」页：拖拽投放区 / 点选 / 粘贴三入口，波形三态，RECENT 历史 |

---

## 1. 目标与非目标

### 目标

1. **三引擎可选**：faster-whisper / Qwen3-ASR / 豆包在线，按请求指定；旧调用方零改动。
2. **ASR → LLM 校对管道**：滑动窗口分批 + 上下文重叠可选，纠错不润色，失败批降级保留原文。
3. **缓存升级到 v2**：同一文件的多引擎、多校对变体聚合存储，提示词版本化自动失效。
4. **过程可观测**：202 任务暴露阶段（asr / proofreading）与窗口进度。

### 非目标

- 不做说话人分离、翻译、摘要（外部需求 §4 明确不做，校对 ≠ 润色 ≠ 翻译）。
- 不做 URL 直接转写（下载职责在采集层）。
- 不进入记录集 / 剪贴板 / 录音优化器链路（与 records 域解耦是 v1 的立身之本）。
- 不做流式（打字机式）校对回传；校对以窗口为原子。
- 豆包 `context` 热词参数**本期不做**（列为 §10 未来项）。

---

## 2. 总体数据流

```
本地文件路径
  │ ①校验(404/415) → ②内容 sha256
  ▼
缓存查（v2 格式：文件条目 → 引擎桶 → 校对变体）────命中────→ 直接整形返回（cached:true）
  │未命中
  ▼
③ffprobe 时长(415/413) → ④>1800s 或 proofread=true → 202 任务
  ▼
⑤引擎装配（engine 参数解析，见 §3.1）
  ▼
⑥输入适配（按引擎决定是否抽 wav，见 §3.2）        ←【单并发闸门只包这一段】
  ▼
⑦ASR 执行（whisper 原生 / qwen 分块拼接 / doubao 分段推流）
  ▼
⑧（可选）LLM 校对管道（§4，不占闸门）
  ▼
⑨缓存写（原子）→ 响应 / 任务终态
```

---

## 3. 引擎层

### 3.1 `engine` 参数与解析优先级【已定】

请求体新增可选字段 `engine`：`"auto" | "whisper" | "qwen_asr" | "doubao_asr"`，缺省 `auto`。

| 请求 engine | 解析结果 |
| --- | --- |
| `whisper` / `qwen_asr` / `doubao_asr` | 显式指定，直接装配对应 provider（优先级最高） |
| `auto`（缺省） | 读全局 `asr_provider`：是本地引擎 → 照用；是 `doubao_asr` → **回落 whisper**（保持 v1 行为，外部脚本零改动、默认零费用） |

> 设计动机：`auto` 的语义是「本地优先」。采集层不传 engine 时行为与 v1 完全一致；
> 想用在引擎必须**显式选择**——把计费决策留给调用方。

### 3.2 输入适配矩阵【已定】

| engine | 非 wav 音频（mp3/m4a/ogg…） | 视频容器（mp4/flv/webm…） | 依据 |
| --- | --- | --- | --- |
| whisper | 原样透传 | ffmpeg 抽 16k mono wav（现状） | PyAV 自带解码，什么都能吃 |
| qwen_asr | **ffmpeg 抽 wav**（v1 缺口，本次补上） | ffmpeg 抽 wav | provider 用 soundfile 读音频，只认 wav/flac/ogg；mp3/m4a 会读失败 |
| doubao_asr | 原样透传 | ffmpeg 抽 wav（也可透传，但统一抽轨减少 provider 内部 pydub 二次转码） | provider 内部 soundfile→pydub/ffmpeg 已有兜底 |

实现落点：`_extract_audio_if_needed(target, engine)`——whisper/doubao 沿用「仅视频容器抽取」；
qwen 改为「除 wav 外一律抽取」。

### 3.3 Qwen3-ASR 长文件分块【已定，参数待实测微调】

**问题**：`max_audio_seconds=30` 参数在 provider 里从未生效，整段音频一次性喂
`model.transcribe()`；Qwen3-ASR 按短音频设计，长文件单次推理会失败或输出截断/幻觉。

**分块设计**（实现在 provider 内，服务层无感知）：

1. **静音优先切分**：用 pydub 静音检测（`detect_nonsilent`，min_silence=500ms）找自然停顿点，
   把音频切成 ≤ `QWEN_ASR_FILE_CHUNK_SECONDS`（默认 30，上限即模型设计上限）的块；
   相邻块**不重叠**（静音处天然边界，避免重复文本）。
2. **硬切兜底**：找不到静音边界（连续说话超窗口）时按窗口硬切。
3. **逐块转写**：块内照常调 `model.transcribe`；时间戳偏移 = 块起点 + 段内偏移。
4. **拼接**：segments 按块顺序拼接；`return_timestamps=True` 时保留词级信息并在块边界重对齐（可接受的近似）。
5. 新增 env：`TRANSCRIBE_FILE_QWEN_CHUNK_SECONDS=30`（进 `.env.example` 与配置说明）。

**验收标准**：长音频拼接处无整句重复、无丢句。
> 【实施注】已完成：SAPI 合成 220.9s 中文语音实测 `engine=qwen_asr`——9 块全部 ≤30s、
> 边界落静音、三段重复内容完整无重复无丢失、时间戳单调收尾对齐，端到端 73s（含模型加载）。
> 真实人声长音频（30 分钟级）的复杂场景留日常使用观察。

### 3.4 豆包在线：分段、凭据与计费【已定，单会话上限待实测】

- provider 的批量 `transcribe()` 已是「文件→文字」完整实现（200ms 推流 + 周期 drain 防背压）。
- **单会话时长上限未验证** → 服务层保守按 `TRANSCRIBE_FILE_DOUBAO_SESSION_SECONDS=1800`（30 分钟）
  预切音频段，逐段调 transcribe，文本拼接、时间戳按段偏移。实测火山支持更长会话后调大即可。
- **凭据**：engine 显式 `doubao_asr` 时装配前检查（密文 > env，复用 `resolve_doubao_api_key()`）；
  未配置 → **409**（见 §8 错误码表），detail 附「请在设置页配置豆包 Access Token」。
- **计费透明**：provider 已回报服务端计费时长 `audio_duration_ms`，写进响应
  `proofread` 同级的 `billing` 字段（仅 doubao 出现）：`{"audio_duration_seconds": 754.2}`。
- **已知限制（写进契约）**：豆包批量只返回整段文本（单 segment 0→duration），
  `want_segments=true` 对豆包等效无效——响应仍给一个整段。

### 3.5 引擎装配与常驻

沿用 v1 的「配置指纹 + 实例常驻 + 变更异步释放」；指纹加入 engine 维度。
三个引擎实例可并存（whisper/qwen 常驻显存/内存，doubao 无常驻），按需懒加载。

---

## 4. LLM 校对管道（核心新逻辑）

### 4.1 触发方式与强制异步【已定】

- 请求体新增 `proofread: bool = false`、`proofread_model_id: str | null = null`（null = LLM v2 网关默认模型条目）。
  > 【实施注】字段名从原稿的 profile_id 正名为 **model_id**——LLM v2 的选择单位是模型条目
  > （entry），校对走 `store.resolve_runtime(model_id)` 与录音优化器同路。
- **`proofread=true` 一律走 202 任务**，即使文件很短：ASR + 校对的总时长不可控（LLM 排队/重试），
  不应占着同步 HTTP。短文件同步路径保持「纯 ASR」不动。
- 前置校验：网关无任何可用档案 → **409**（与豆包凭据同语义：客户端可修复的配置缺失）。

### 4.2 窗口切分【已定；默认值已落定 40 段 / 4000 字 / 200 字】

ASR segments → 窗口序列，规则（**同时满足段数与字符数双上限**）：

- 每窗口 ≤ `batch_segments` 段（默认 40，UI 可选 20/40/60）；
- 每窗口正文字符 ≤ `batch_chars`（默认 4000）——**为豆包准备**：它整场只有一个 segment，
  按段数切不开，必须按文本长度切；
- 切窗发生在段边界（不拆断一个 segment）；
- 可选**上下文重叠**：每窗口携带前文尾部 `context_chars`（默认 200 字）作为提示词里的
  「前文参考」，**不参与输出**（键集合只含本窗段号）——防止跨窗人称/术语漂移，代价是少量 token。

窗口参数从哪来：请求级 `proofread_options: {batch_segments?, batch_chars?, context_chars?}`（可选，
全部有默认值）；App 前端在「LLM 校对」面板暴露。

### 4.3 提示词（自有草稿，版本化）【已定：v1 草稿见附录A】

设计原则（策略参考开源调研结论，文案自写，见附录B 许可证说明）：

1. 输入：`{段号: 原文}` JSON + 可选「前文参考」+ 可选「参考信息」（文件名等）。
2. 要求：只纠错（错字/近音字/标点/口头语/明显专名错误）；禁改写句式、禁同义替换、禁翻译、禁增删段。
3. 输出：与输入**键集合完全一致**的纯 JSON，无解释、无 markdown 围栏。

`PROMPT_VERSION = 1` 常量：进缓存 key（§5）。改提示词必须 +1，旧校对缓存自动失效。

### 4.4 解析、验证与重试【已定】

每窗口独立执行「LLM → 验证 → 反馈 → 重试」循环（最多 3 次）：

1. **解析容错**：剥 markdown 围栏 / 尾随逗号 / 单引号包裹键等常见畸形（自实现轻量修复，不引第三方）。
2. **验证一：键完全匹配**——输出键集合 ≠ 输入键集合（吞段/加段/并段）即失败，
   错误明细（缺哪些键/多哪些键）作为反馈喂回对话。
3. **验证二：相似度阈值**——逐条 `difflib.SequenceMatcher`：常规文本 ≥ 0.7，
   ≤10 词短文本 ≥ 0.3；低于阈值判「改写过头」，同样反馈重试。
4. 3 次仍不过 → **该批保留原文**，记入 `proofread.fallback_batches`（窗口序号数组），流程继续。

### 4.5 降级语义【已定】

| 情形 | 行为 |
| --- | --- |
| 个别窗口失败 | 保留原文，任务照常 done，`fallback_batches` 非空 |
| LLM 网关整体不可用（连第一个窗口都失败） | **不把任务置 failed**：ASR 结果仍然有价值——`proofread.applied=false` + `proofread.error=...`，`text` 回落 ASR 原文，任务 done |
| ASR 文本为空（没识别到语音） | 跳过校对：`applied=false, reason="asr_empty"` |

### 4.6 并发与资源【已定】

- **校对不占单并发闸门**（闸门只包 ASR 执行段）：校对期间其他文件的 ASR 可正常跑。
- 窗口并发：`TRANSCRIBE_FILE_PROOF_CONCURRENCY=2`（LLM 侧温和并发；窗口间无顺序依赖，
  上下文重叠取自**原文本**而非前窗输出，天然可并行）。
- LLM 调用走 **app/llm v2 网关**（LiteLLM），用法与录音优化器一致：`temperature=0.2`，
  强制超时/重试由网关注入（LLM v2 已修复的历史缺陷，不在本层重复造）。

### 4.7 响应字段（proofread=true 时）【已定】

```json
{
  "text": "校对后全文",
  "raw_text": "ASR 原文（仅 proofread=true 时出现）",
  "duration_seconds": 754.2,
  "engine": "whisper:large-v3",
  "cached": false,
  "segments": [{ "start": 0.0, "end": 3.2, "text": "校对后段文本" }],
  "proofread": {
    "applied": true,
    "llm_profile": "deepseek-chat",
    "windows": 8,
    "requests": 9,
    "fallback_batches": [],
    "prompt_version": 1
  }
}
```

- segments 的 **text 是校对后文本，时间戳保持 ASR 原值**（校对不改变时间轴）。
- 前端「对照」视图：后端只给 `text` + `raw_text`，diff 由前端按句切分计算（不新增端点）。

### 4.8 202 任务状态扩展【已定】

`GET /api/transcribe/jobs/{id}` 响应增加：

```json
{
  "status": "running",
  "stage": "proofreading",
  "progress": { "window_done": 3, "window_total": 8 },
  "result": null,
  "error": ""
}
```

- `status` 仍只有 `running | done | failed`（v1 兼容）；
- `stage`：`asr → proofreading → done`（无校对时跳过 proofreading）；
- `progress` 仅 proofreading 阶段非空。

---

## 5. 缓存设计 v2【已定】

### 5.1 条目结构（按文件聚合）

```json
{
  "<sha256>": {
    "size": 12345678,
    "cached_at": 1761140000,
    "engines": {
      "whisper": {
        "text": "...",
        "duration_seconds": 754.2,
        "engine_label": "whisper:large-v3",
        "segments": [...],
        "proofread": {
          "deepseek-chat@1": { "text": "...", "windows": 8, "requests": 9, "fallback_batches": [] }
        }
      },
      "qwen_asr": { ... },
      "doubao_asr": { ..., "billing": { "audio_duration_seconds": 754.2 } }
    }
  }
}
```

- **LRU 淘汰整文件条目**（上限仍 `TRANSCRIBE_FILE_CACHE_SIZE=200`）：换引擎重转不挤占其他文件的名额。
- 校对变体键 = `{profile_id}@{PROMPT_VERSION}`：换 LLM 档案或升提示词都自然 miss，互不污染。
- 缓存命中条件 = 文件 sha256 + engine + （若开校对）该 profile 的校对变体存在。
  命中校对缓存时 `cached:true` 且 `proofread` 元数据原样回放。

### 5.2 v1 → v2 迁移（宽容读，符合 json_store 惯例）

- v1 条目是平铺的 `{text, duration_seconds, engine, segments}`（必然由 WhisperProvider 产出，
  因为 v1 强制本地并回落 whisper）。
- 读时：无 `engines` 桶 → 视为 `engines.whisper`；下次写该文件时升级为 v2 形态。
- 不做一次性全量迁移脚本（旧条目自然被 LRU 淘汰）。

---

## 6. API 契约 v2【已定】

### 6.1 请求（新增字段全部可选，v1 调用方零感知）

```json
{
  "path": "E:\\tmp\\dv\\abc.mp4",
  "language": "zh",
  "want_segments": false,
  "engine": "auto",
  "proofread": false,
  "proofread_profile_id": null,
  "proofread_options": { "batch_segments": 40, "batch_chars": 4000, "context_chars": 200 }
}
```

### 6.2 行为矩阵

| 场景 | 路径 |
| --- | --- |
| 短文件 + 无校对 | 同步 200（v1 不变） |
| 短文件 + 校对 | **202**（§4.1） |
| 长文件（>1800s） | 202（不变），有校对时任务含 proofreading 阶段 |

### 6.3 错误码总表（新增 409）

| 码 | 含义 | v2 变化 |
| --- | --- | --- |
| 404 | 文件不存在 / 相对路径 / 任务号过期 | 不变 |
| 415 | 无法解码（扩展名 / ffprobe / 抽轨失败 / qwen 读取失败） | 不变 |
| 413 | 超 7200s 上限 | 不变 |
| 503 | 引擎忙 / 任务表满，带 Retry-After | 不变 |
| **409** | **前置配置缺失**：① engine=doubao_asr 且豆包凭据未配置；② proofread=true 且无任何 LLM 档案 | **新增**，detail 附可读中文与去向提示，采集层**不可重试** |
| 422 | engine / proofread 字段非法 | FastAPI 自动 |
| 500 | 其他内部错误 | 不变 |

---

## 7. 前端设计要点（对照原型）

- **引擎三卡**：faster-whisper（默认选中）/ Qwen3-ASR / 豆包；UI 不暴露 `auto`（auto 只留给 API 缺省语义），
  UI 默认即显式 `whisper`。
- **豆包卡凭据态**：读 `GET /api/settings` 现成的 `doubao_asr_api_key_set`——false 时置灰 +
  「去设置」跳转（跳 `navigate:settings`，已有 IPC）。
- **校对面板**：开关（**默认关**，见 §10-1）+ LLM 档案下拉（读 LLM v2 档案列表）+
  批大小 / 上下文重叠下拉 + token 预估行（前端粗估：窗口数 = ceil(段数/批大小)，token ≈ 字符数 × 1.1）。
- **双阶段进度**：轮询 202 的 stage/progress 渲染「① ASR ② 校对 x/y」两张进度卡（原型已示）。
- **结果三视图**：校对后 / 原文 / 对照（前端 diff）；徽标：引擎、时长、`已校对`、`缓存命中`。
- 历史架条目加引擎徽标（`whisper+校对` / `doubao`…）。

---

## 8. 测试计划

| 层 | 用例要点 |
| --- | --- |
| 单元：引擎分流 | engine × 扩展名 × 期望行为矩阵（是否抽轨、装配哪个 provider）；auto 回落 whisper；显式 doubao 无凭据 → 409 |
| 单元：qwen 分块 | 静音切分边界（恰好 30s / 无静音硬切 / 空音频）、时间戳偏移拼接、多块 segment 顺序 |
| 单元：doubao 分段 | >30min 切段数与拼接；计费时长聚合 |
| 单元：窗口切分器 | 段数上限 / 字符上限（豆包单 segment 长文本场景）/ 段边界不拆断 / 上下文重叠不含本窗键 |
| 单元：校对验证器 | 键集合不等 → 反馈重试；相似度 0.69 拒收 / 0.71 通过；3 次耗尽保留原文；解析容错（围栏/尾逗号） |
| 单元：缓存 v2 | 多引擎桶互不覆盖；校对变体 key 含 profile@version；v1 平铺条目宽容读为 whisper；LRU 整条淘汰 |
| 契约 | 新字段全可选（旧请求体回归）；proofread=true 短文件也 202；409 两种触发；stage/progress 字段 |
| 回归 | v1 的 14 个用例**全部保持**；端点契约基线不变（无新端点） |
| 验收（人工） | 30 分钟真实中文音频 × 三引擎；校对前后人工抽检 10 处（纠错对 / 无改写）；豆包计费时长对账 |

---

## 9. 实施里程碑

| 阶段 | 内容 | 预估 | 状态 |
| --- | --- | --- | --- |
| M1 | engine 参数 + 输入适配矩阵 + 缓存 v2（含迁移）+ 409 + 豆包接入（含分段） | 0.5 天 | ✅ `5a47c09` |
| M2 | LLM 校对管道（窗口/提示词/验证/降级）+ 202 stage/progress + 前端校对面板与对照视图 | 1 天 | ✅ `62eb618` |
| M3 | Qwen 长文件分块（静音切分 + 拼接）+ 真实长音频验收 | 1–2 天 | ✅ `629bd0d` |
| M4 | 文档同步（API.md §8 / 配置说明 / 外部需求文档）+ 全量回归（pytest + node + build） | 0.5 天 | ✅ 本提交 |

顺序理由：M1 是 M2/M3 的地基（引擎分流与缓存格式）；M2 先于 M3 因为不依赖分块且价值最高；
M3 风险最大（拼接质量需实测），放最后单独验收。

---

## 10. 开放问题（已按倾向值实施；见各行「结果」列）

| # | 问题 | 倾向 | 结果 |
| --- | --- | --- | --- |
| 1 | 校对默认开还是关 | 默认关（采集层显式传 true） | **✅ 已按倾向实施**（API 缺省 false，UI 开关默认关） |
| 2 | 窗口批大小默认值 | 40 段 / 4000 字 / 200 字重叠 | **✅ 已按倾向实施**（UI 可选 20/40/60 段，重叠 200 字或关） |
| 3 | 豆包单会话时长上限 | 保守 30 分钟，实测后调大 | **✅ 已实施 1800s**；待真实长音频实测后调大（env 可调） |
| 4 | Qwen 分块拼接质量验收 | 无整句重复/丢失 | **✅ 已完成**（220.9s 语音 9 块实测通过，见 §3.3 实施注） |
| 5 | 未来项：请求体加 `context`（豆包热词 / 校对参考术语） | 本期不做 | ⏸ 未做，留作下期 |

---

## 附录A：校对提示词草稿 v1（自有文案）

```
【系统】
你是语音转写（ASR）文本的校对员。用户给你一个 JSON 对象：键是段落编号，值是识别原文。
你的任务是逐条纠错，规则：

1. 只修正识别错误：错别字、同音/近音字、标点缺失或错用、明显的专有名词错误；
2. 删除口头语气词（呃、嗯、啊）与拟声词（*笑声*、*咳嗽*）；
3. 严禁：改写句式、替换同义词、翻译、总结、合并或拆分段落；
4. 前文参考（如有）仅用于判断术语与人称，不是待校对内容；
5. 输出键集合必须与输入完全一致，值与键一一对应；
6. 只输出 JSON 对象本体：不要解释、不要 markdown 代码围栏。

【用户】
（可选）前文参考：<前文尾部 N 字>
（可选）参考信息：文件名 / 术语表
待校对：
{"0": "大家好呃今天我们来学习机器学习的基 本 概念", "1": "它使用反向传播算法来训练模型嗯"}
```

`PROMPT_VERSION=1`；任何措辞调整都递增版本号使校对缓存失效。

## 附录B：开源调研结论与许可证边界

| 项目 | 协议 | 结论 |
| --- | --- | --- |
| VideoCaptioner（16.3k★） | **GPL-3.0** | 滑窗 + 验证设计（键匹配 / 相似度阈值 / 反馈重试 / 失败保留原文）的策略来源；**代码与提示词文案不得复制**，本文档 §4 的实现与附录A 文案均为自写 |
| pyVideoTrans（19.5k★） | GPL-3.0 | 同类批处理思路佐证，同样只借鉴策略 |
| GPT-Subtrans | MIT | 跨批上下文延续（上批尾部做下批前文）策略参考，实现协议干净 |

> VideoCaptioner 的核心可借鉴点（读码结论，非复制）：分批字典并发 → 每批独立
> agent loop（LLM→验证→反馈≤3 次）→ json 容错解析 → 失败批保留原文降级；
> 相似度阈值 0.7（短文本 0.3）、temperature 0.2。
