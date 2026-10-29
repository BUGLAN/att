# ATT 重写架构与模块设计（阶段②交付物）

> 输入：阶段①《[Tauri重写-01-功能梳理清单.md](Tauri重写-01-功能梳理清单.md)》与《[Tauri重写-01a-前端审计附录.md](Tauri重写-01a-前端审计附录.md)》。
> 技术栈：Tauri 2.x + Rust（工作区多 crate），前端 Vue 3 + Pinia 保留，仅适配 IPC 层。
> 本文档 = 架构与模块划分说明。**编码在阶段③，按 §13 分八个子阶段交付。**

---

## 0. 设计目标与非目标

**目标**（对应任务书 6 项要求）：

| # | 要求 | 对应设计 |
| --- | --- | --- |
| 1 | Tauri 2.x + Rust 完全替换 Electron + Python | 单进程原生应用，无 Python 运行时分发（§2 D5） |
| 2 | 前端 UI/样式保留，仅换 IPC 层 | 保留全部组件与样式；useElectron→useDesktop 保持导出形状（§10） |
| 3 | trait Provider/Adapter + 状态机 + 事件总线，采集/处理/引擎解耦，符合开闭原则 | §3/§4；§4.4 开闭验证走查 |
| 4 | 三种录音模式：缓冲/采样率/背压/线程模型 | §5 |
| 5 | 快捷键：任意键组合、冲突检测、点按/按住、三端一致、无粘滞 | §6（20 条缺陷逐一修复，§6.7） |
| 6 | UI 动效（/ui-ux-pro-max） | §10.5（阶段 3h 实施，架构已预留无性能开销通道） |

**非目标**：多语言 UI、浅色主题、移动端、浮层/生词本等已下线功能恢复（§7.2 of 01）。

**硬约束**（继承阶段①结论）：
- 密钥红线：明文永不落盘/不进日志/GET 响应不含密文字段；Fernet 旧密文必须能解（§8.3）
- 前端依赖协议保真面（裸数组、health 27 字段、NDJSON 等）在过渡期保持（§10.1）
- 三防御语义（不丢开头/不丢尾/防幽灵采集）必须保留（§5.7）

---

## 1. 总体架构

### 1.1 拓扑

```
┌─────────────────────────── Tauri 进程（唯一进程）───────────────────────────┐
│                                                                             │
│  ┌──────── WebView（前端，保留）────────┐        ┌──────── Rust 核心 ────────┐ │
│  │ Vue3+Pinia  组件/样式/stores 不变     │        │                          │ │
│  │  useDesktop(=useElectron 换内核)     │ invoke │  ┌────────────────────┐  │ │
│  │  api/client.js(几乎不变) ────────────┼───────►│  │ Tauri commands     │  │ │
│  │  useRecordingBridge(新,薄)          │◄───────┼──┤ Tauri events       │  │ │
│  └───────────────┬─────────────────────┘ listen │  └────────┬───────────┘  │ │
│                  │                                   │              │      │ │
│                  │ HTTP(127.0.0.1:随机端口)           ▼              ▼      │ │
│                  └──────────────► ┌──────────────┐  ┌────────────────────┐ │ │
│                                  │ 内嵌 axum     │  │ 事件总线(broadcast) │ │ │
│                                  │ 数据面 API    │  │ recording/llm/...  │ │ │
│                                  └──────┬───────┘  └─────────┬──────────┘ │ │
│                                         │                    │            │ │
│   ┌─────────────────────────────────────┼────────────────────┼──────────┐ │ │
│   │ att-core（领域内核，headless 可测）   │                    │          │ │ │
│   │  RecordingSessionActor ◄──命令 mpsc ─┤─ HotkeyTrigger ◄───┤          │ │ │
│   │  AudioPipeline(采集→重采样→VAD→分发) │                    │          │ │ │
│   │   └► TranscribeProvider(批量/流式)   │  LlmGateway ─► 优化器/校对     │ │ │
│   │  Records/Clipboard/Prompts/Settings/Storage/迁移           │          │ │ │
│   └──────────────────────────────────────────────────────────────────────┘ │ │
│   att-hotkey(Win/mac/Linux 平台后端)  att-desktop(托盘/悬浮窗/静音/粘贴/剪贴板)│
└─────────────────────────────────────────────────────────────────────────────┘
        外部：火山 SAUC WS │ Qwen 桥接(HTTP,可选外部) │ LLM API │ 系统输入/输出设备
```

### 1.2 三条平面（关键分层规则）

| 平面 | 通道 | 内容 | 理由 |
| --- | --- | --- | --- |
| **控制面** | Tauri commands + events | 录音会话控制、热键、托盘、悬浮窗、剪贴板事件、实时转写/音量/优化增量 | 低延迟、类型化、随窗口生命周期 |
| **数据面** | 内嵌 axum（仅回环） | 记录/设置/LLM 配置/剪贴板/文件转写/health、NDJSON 流、音频播放与导出 URL | 前端 client.js 近零改动；下载/播放用 URL 最自然；浏览器 dev 模式可用 |
| **音频面** | 进程内（rtrb 环 + tokio mpsc） | PCM 采集→处理→Provider | 音频不再经过渲染进程（现状每 200ms 一帧要穿过 WebView JS 事件循环） |

### 1.3 进程与线程

- **单进程**（Tauri）。可选外部进程：Qwen 流式桥接（用户自建，见 D5）；ffmpeg/ffprobe 按需子进程（音视频解码探测，与现状 pydub 隐式依赖相同）。
- 线程模型见 §5.8 总表；关键规则：OS 音频回调内**零分配零锁**；状态机单 Actor 串行决策（消灭现状 30 个并发标志）。

---

## 2. 关键架构决策（ADR）

### D1 音频采集移入 Rust（cpal）——最大决策

现状采集在渲染进程（getUserMedia/MediaRecorder/AudioWorklet），音频每帧穿越 WebView（01 §0.1）。
**决策**：采集、编码、切段全部移入 Rust（cpal + rtrb + rubato）。
**理由**：① 快捷键→开录不再依赖渲染层存活/不节流（修复缺陷 #15、#17 延迟链）；② Linux webkitgtk 的 getUserMedia 不可靠，三端一致只有原生采集能保证；③ 流式路径砍掉「worklet→JS→WS」两级跳，延迟与丢帧面收缩；④ 单段录音直接产 WAV（不再经 webm/opus 有损压缩）。
**代价**：设置页麦克风枚举从 getUserMedia 改为 Rust 命令；音量条/实时文本改为事件驱动（§10.3 已设计为 store 形状不变）。

### D2 混合 IPC：内嵌 axum 数据面 + Tauri 控制面

不把全部 API 改成 invoke 的理由：NDJSON 流式优化、记录音频播放（`<audio src>`）、导出下载、client.js 45+ 函数的改造成本。HTTP 保留在回环随机端口，CSP 已放行 `127.0.0.1:*`。**两个 WS 端点移除**（见 D4），其余 50 个 HTTP 端点契约不变（含 health 27 字段、裸数组等，01 §3.9「必须保真」清单照办）。

### D3 录音编排移入 Rust（Actor + 状态机）

现状 useRecorder.js 2443 行、约 30 个并发标志、多次「启动窗口」事故（01 §5.2）。
**决策**：`RecordingSessionActor` 单 mpsc 消费者串行决策，状态机显式建模（§7.2）；三防御语义（并行建连/尾包先冲/启动窗口排队停止）在状态机内原生实现。
**代价**：useRecorder.js 被替换为薄客户端（§10.3），UI 组件与 stores 形状不变。

### D4 移除 WS /api/ws 与 WS /api/asr/stream

它们的存在只为弥补「采集在渲染层、编排跨进程」：全局 WS 12 类事件改由事件总线→Tauri events 重新以**同名事件**发出（前端 handleWebSocketMessage 逻辑迁入桥接层，§10.2）；流式会话改为进程内直连（音频面）。重连问题（固定 3s 无退避，缺陷 #9）随之消失——事件随窗口存在。

### D5 本地引擎策略（诚实取舍，需用户确认）

| 现引擎 | Python 生态 | Rust 方案 | 迁移影响 |
| --- | --- | --- | --- |
| faster-whisper | CTranslate2 | **whisper.cpp（whisper-rs 0.16）** | 模型换 GGML 格式需重新下载（large-v3 ≈3GB）；速度同档（int8 量化） |
| Qwen3-ASR 本地批量 | qwen-asr+torch | **无 Rust 等价物** | 降级为「远端桥接 Provider」：协议保留（HTTP start/chunk/finish），桥接服务不随包分发，用户可自建/Docker/局域网 |
| Qwen 流式 | vLLM 桥接 | 同上（远端桥接） | Windows 本就不可用（01 §3.5），影响面小 |
| 豆包 SAUC / ChatGPT 听写 | 在线 | 纯 Rust 重写（tokio-tungstenite/reqwest，可自定义 header——顺带修复 Node undici 不能带鉴权头的坑） | 无 |

**开闭原则保证**：新增本地引擎（如未来 Qwen 的 GGUF/ONNX 版）= 新增一个 `TranscribeProvider` 实现 + 注册项，核心零改动（§4.4 走查）。

### D6 LLM 网关自研，替代 LiteLLM

LiteLLM 是 Python 生态。Rust 网关：**OpenAI 兼容协议为基座**（reqwest + SSE 流解析）+ 协议适配器（Anthropic Messages、Gemini generateContent）+ 服务商注册表（沿用 14 家与 `compose_model_string` 规则，custom 恒 `openai/<ID>`）。必保语义（01 §3.6）：强制超时/重试、按输入长度派生三阶段超时、流式双看门狗（首 token/块间空闲）、reasoning_effort 预检与降级、output_truncated 不可重试、密钥脱敏。

### D7 存储沿用 JSON 布局；单实例锁取代跨进程写者锁

`data/` 目录结构与文件格式**不变**（01 §3.7），迁移成本最小化。原子写保留（tmp+fsync+rename）；跨进程 `.writer.lock` 删除（单进程 + tauri-plugin-single-instance 双保险，Docker 共目录场景随 Python 后端一起退役）；进程内按资源 `Mutex` 串行化保留。配置单一真相源收敛到 Rust `settings` 模块（消灭 hotkey 归一化 4 份实现，01 §4.3-18）。

---

## 3. Workspace 与模块划分

```
src-tauri/
├── Cargo.toml            # workspace
├── crates/
│   ├── att-core/         # 领域内核：不依赖 tauri/平台 GUI，headless 可测
│   ├── att-hotkey/       # 全局热键平台后端（feature: win32 / macos / x11 / portal）
│   ├── att-desktop/      # 托盘/悬浮窗控制/静音/粘贴/剪贴板监控（依赖 tauri）
│   └── att-server/       # 内嵌 axum 数据面 API（依赖 att-core）
└── src/                  # tauri app：装配、commands、事件桥、生命周期
```

**依赖规则**（箭头=依赖）：`app → {att-core, att-hotkey, att-desktop, att-server}`；`att-server → att-core`；`att-desktop → att-core, att-hotkey`；`att-core → ∅`（仅 tokio/serde/标准库与音频算法库）。CI 对 att-core 单独 `cargo test --no-default-features` 保证无桌面依赖。

### att-core 模块清单

| 模块 | 职责 | 关键类型 |
| --- | --- | --- |
| `audio` | 采集抽象、SPSC 环、重采样、混音、电平表、VAD、分段策略、PCM/WAV 编解码 | `AudioSource`、`Pipeline`、`SilenceDetector`、`SegmentPolicy`、`Pcm` |
| `stt` | 批量/流式转写 trait + 引擎实现 + 工厂 + 能力声明 | `TranscribeProvider`、`StreamingTranscriber`、`DoubaoSaucProvider`、`QwenBridgeProvider`、`WhisperCppProvider`、`ChatGptAsrProvider` |
| `recording` | 录音会话 Actor 与状态机、三模式编排、看门狗 | `SessionActor`、`SessionState`、`Mode` |
| `llm` | 网关（OpenAI 兼容+适配器）、注册表、档案/模型存储、优化器、流式优化、滑窗校对 | `LlmGateway`、`Registry`、`LlmStore`、`Optimizer`、`Proofreader` |
| `records` | Record/Session 模型与仓库、索引、housekeeping | `Record`、`RecordRepo`、`RecordIndex` |
| `clipboard` / `prompts` | 剪贴板域、提示词域（只读） | … |
| `settings` | Settings(env) + UserSettings + 热键绑定模型（唯一归一化实现） | `Settings`、`UserSettings`、`HotkeyBinding` |
| `storage` | 原子写、宽容读、Fernet 兼容加密、迁移 | `json_store`、`crypto`、`migration` |
| `events` | 进程内事件总线与全部领域事件定义 | `EventBus`、`AppEvent` |

---

## 4. 核心抽象（trait 与设计模式）

### 4.1 模式总览

| 模式 | 用在哪 |
| --- | --- |
| **Provider（trait 对象 + 注册表）** | 转写引擎、LLM 供应商、音频源 |
| **Adapter** | 平台热键后端、LLM 协议适配、静音控制器 |
| **Actor（命令队列串行）** | RecordingSessionActor、HotkeyTrigger |
| **State machine** | 录音会话、Record 生命周期、Provider 会话、热键触发 |
| **Observer / 消息总线** | EventBus（tokio broadcast）→ 托盘/悬浮窗/前端桥 |
| **Strategy** | 触发模式（点按/按住）、分段策略、背压策略 |
| **Watchdog** | 收尾/流式/优化看门狗、热键盘活 |

### 4.2 AudioSource（采集解耦）

```rust
trait AudioSource: Send {
    fn list_devices(&self) -> Vec<DeviceId>;
    fn open(&self, cfg: SourceConfig) -> Result<SourceStream>;  // cfg: device, 建议缓冲
}
struct SourceConfig { device: Option<DeviceId>, ring_capacity_frames: usize /*默认≈2s*/ }
// SourceStream 内部：cpal 回调 → mono 混音 → rtrb SPSC 环（零分配路径）
```
实现：`CpalSource`（生产）、`WavFileSource`（文件转写/回放测试）、`SynthSource`（测试：正弦/静音/序列帧——CI 无设备也能全链路测）。

### 4.3 AudioPipeline（处理解耦）

```
ring(设备原生率, f32 mono) ─► [重采样→16k] ─► [电平表40ms] ─► [VAD(可换)] ─► 分发器
```
- 重采样：rubato（设备率≠16k 时）；=16k 直通。
- VAD v1 = 能量阈值（移植现 silence_level/threshold 语义），接口留 `VadStage`，后续可换 silero。
- 分发器按当前模式把 10ms 帧路由到 sink（单段 WAV 写盘 / 多段分段器 / 流式 3200 采样分块）。
- 电平事件 30Hz 合并后上事件总线（UI 波形）。

### 4.4 TranscribeProvider（引擎解耦 + 开闭验证）

```rust
trait TranscribeProvider: Send + Sync {
    fn id(&self) -> &'static str;                       // "whisper" | "qwen_asr" | "doubao_asr" | "chatgpt_asr"
    fn capabilities(&self) -> Capabilities;             // streaming/word_timestamps/offline_batch...
    fn runtime_state(&self, mode: Mode) -> RuntimeState;// online_ready / needs_warmup / bridge(url) / unsupported(reason)
    async fn warmup(&self) -> Result<()>;
    async fn transcribe(&self, audio: AudioInput, opts: TranscribeOpts) -> Result<TranscriptionResult>;
    fn streaming(&self) -> Option<&dyn StreamingTranscriber> { None }
}
trait StreamingTranscriber: Send + Sync {
    async fn open_session(&self, lang: Lang, ctx: &str) -> Result<StreamSession>;
}
// StreamSession: push_pcm(i16 mono 16k) → StreamUpdate{ text, is_final } ; finish(); cancel()
```
注册表 `stt::registry()`：`Vec<Arc<dyn TranscribeProvider>>` 由 app 装配注入。设置里 `asr_provider` 换引擎=换选择，不动核心。
**开闭走查**：
1. 新增在线 ASR（例：腾讯云）→ 新文件 `providers/tencent.rs` 实现 trait + 注册一行；录音/文件转写/设置页全部自动获得，核心 0 改动 ✓
2. 新增本地引擎（例：未来 Qwen GGUF）→ 同上，runtime_state 声明 needs_warmup + 模型路径解析 ✓
3. 更换采集后端（例：虚拟音频设备聚合）→ 仅新增 AudioSource 实现 ✓

### 4.5 LlmGateway（D6）

```rust
trait LlmProtocol { async fn chat(&self, req: ChatRequest) -> Result<ChatReply>;
                    async fn chat_stream(&self, req: ChatRequest) -> Result<ChatStream>; }
// 实现：OpenaiCompatAdapter（覆盖 deepseek/openai/doubao/moonshot/zhipu/groq/mistral/xai/openrouter/ollama/custom）
//       AnthropicAdapter、GeminiAdapter
struct LlmGateway { protocol_of(provider) -> &dyn LlmProtocol, timeouts: derive_timeouts, watchdogs }
```
保留：reasoning_effort 预检/降级、output_truncated 不可重试、密钥仅运行期解密、脱敏日志。

### 4.6 HotkeyBackend（§6 详述）

```rust
trait HotkeyBackend {
    fn probe(&self) -> BackendCapability;               // 支持度/权限/限制说明
    fn register(&self, spec: HotkeySpec) -> Result<BindingId, HotkeyError>; // HotkeyError::Conflict=系统冲突
    fn unregister(&self, id: BindingId);
    fn events(&self) -> Receiver<RawKeyEvent>;           // { binding, phase: Pressed|Released }
    fn poll_physical_state(&self, spec) -> bool;         // 防粘滞防线1：真实键态查询
    fn resync_all(&self);                                // 防粘滞防线2：系统事件后重置
}
```

### 4.7 EventBus 与命令通道

- `EventBus = tokio::sync::broadcast<AppEvent>`；lagged 只影响 UI（可丢弃），关键命令走 `mpsc<SessionCmd>` 到 Actor（不丢）。
- 订阅者：Tauri 事件桥（→前端，含旧事件名重放）、托盘控制器、悬浮窗控制器、att-server（health 聚合）。
- 前端桥**按旧 WS 事件名原样重发**（transcription_completed/optimization_delta/...），前端消息处理逻辑仅换订阅来源（§10.2）。

### 4.8 Repository（存储解耦）

`trait RecordRepo / ClipboardRepo / LlmConfigRepo / SettingsRepo`，默认 JSON 实现（布局不变）；测试用内存实现。原子写与宽容读收敛在 `storage::json_store`（移植 write_json_atomic / read_json_lenient 语义与测试）。

---

## 5. 录音三模式设计（缓冲/采样率/背压/线程）

### 5.1 通用采集路径（三模式共用）

```
[cpal 回调(设备率 f32, Nch)] → mono 混音 → rtrb SPSC 环(容量≈2s, 2^n 帧)
        │  （回调内：零分配、零锁、零系统调用）
        ▼
[pipeline 专用线程] 读环 → 重采样 16k → 电平表 → VAD → 10ms 帧分发
        ▼ 按模式的 sink（tokio 任务，各有独立有界通道）
```

### 5.2 采样率与格式处理

- 采集：接受设备原生率（44.1k/48k 常见），**不强行请求 16k**（WASAPI/CoreAudio 共享模式不可靠）；mono 混音在回调完成。
- 规范化：rubato 重采样至 **16k mono f32**，再转 **i16 LE**（流式上行与引擎输入统一格式，兼容豆包 6400B/200ms 帧与桥接协议）。
- 落盘：单段/多段直接写 **WAV (PCM16)**（hound 流式写 + 收尾回填头）——摆脱 webm 有损转码链，时长=采样数/16000，精确无漂移。
- 文件转写：ffmpeg/ffprobe 解码探测（现状同依赖），非 WAV 先转 16k mono wav。

### 5.3 单段（single）

1. Actor: Idle→Preparing（开设备+Provider 预检，**期间 ring 已在收数**）→Recording。
2. sink：pipeline 线程顺序写盘线程的有界队列（4096 帧≈256ms 缓冲），盘写抖动由环+队列吸收。
3. 停止：关设备→冲队列→WAV 收尾→Actor: Finalizing→`transcribe(wav)`→建 Record(pending→transcribing→transcribed)→（auto_optimize）Optimizing。
4. 状态与文本经事件总线推送；**空音频（<0.5s）**走「录音时间过短」路径（移植现语义）。

### 5.4 多段（multi）

1. VAD（能量阈值+时长）驱动分段状态：`Listening→Speaking→SegmentClose`；阈值与最短 2.2s/最长 10s/强制 0.8s 策略**原值移植**（`SegmentPolicy`，Rust 单测锁定）。
2. 每段：闭段时把该段采样区间切片成独立 WAV → 有界段队列(2) → 转写任务（并行度≤2）→ `recording:segment` 事件（实时预览按 sequence 排序拼接，语义同现状）。
3. 停止：当前段强制闭段→等待在途段完成（**上限 30s 看门狗**，超时按已到齐处理并标记 degraded——修复缺陷 #2 卡死）→merge（合并 WAV 存在则整段重转，失败回退拼接文本——现状语义）→ Record。
4. 无语音→删会话并报「未检测到语音」（现状语义）。

### 5.5 流式（streaming）

1. Preparing：**开设备与建 Provider 会话并行**（ring 先行收数——「不丢开头」在原生侧天然成立：建连 0.5~0.7s 的样本在环里，会话就绪后统一冲刷，协议顺序不变）。
2. 分块：3200 采样（200ms）i16 帧 → 有界通道(8≈1.6s)→Provider 推送任务→`recording:partial`。
3. 停止：**先冲尾包再关采集**（顺序由 Actor 状态机保证，配 15s 收尾看门狗）→finish→`stream_finished` 语义：空文本→不建 Record（删音频不留档，现状语义）；非空→Record(transcribed)→可选优化。
4. Provider 会话意外断开：状态机回 Finalizing(失败) → 按已收 partial 文本兜底建 Record 并标记 degraded（优于现状的裸错误）。

### 5.6 背压与降级策略（显式化）

| 环节 | 机制 | 满载行为 |
| --- | --- | --- |
| 回调→环 | rtrb SPSC，容量≈2s | 回调永不阻塞；理论溢出=pipeline 死亡，视为 bug 上报（不可达设计） |
| pipeline→sink | tokio mpsc 有界 | 转写/网络 sink 慢于实时：单段/多段缓冲在盘与段队列自然吸收；**流式**通道满持续 >30s → 丢弃最旧块 + `recording:degraded` 事件（UI 可见），不静默 |
| 事件总线 | broadcast | UI lag 只丢 UI 事件，不影响录音正确性 |
| Actor 命令 | mpsc 无界 | 命令量极小（人手速度），不设限 |

### 5.7 时间戳与不丢帧/不截断保证

- **唯一时钟 = 16k 采样计数**：duration=samples/16000；录音开始墙钟时间仅作元数据展示。VAD 切段、流式分块全部基于采样计数 → 无重采样累积误差、无毫秒漂移。
- 不丢帧：回调零阻塞 + 环容量 2s（容忍 pipeline 最坏调度）+ 各 sink 有界缓冲。
- 不截断：停止顺序由状态机强制（先冲尾部后关资源）；WAV 头收尾回填，异常退出留 .tmp 不污染。

### 5.8 线程模型总表

| 执行体 | 数量 | 实时性要求 | 职责 |
| --- | --- | --- | --- |
| OS 音频回调 | 1 | 硬实时（零分配/零锁） | mono 混音→写环 |
| pipeline 线程 | 1 | 软实时（独占核优先） | 重采样/电平/VAD/分发 |
| 磁盘写线程 | 1 | 普通 | WAV 流式落盘 |
| SessionActor | 1（tokio） | 串行决策 | 状态机、看门狗、命令 |
| Provider IO 任务 | N（tokio） | 异步 | 豆包 WS/桥接 HTTP/whisper.cpp（whisper 在独立 CPU 线程池跑推理，避免阻塞 runtime） |
| LLM 网关任务 | N | 异步 | chat/SSE |
| 平台热键线程 | 1/平台 | 事件泵 | Win 消息循环/CFRunLoop/X11 event |
| Tauri 主线程 | 1 | UI | 窗口/托盘/事件转发 |

---

## 6. 快捷键系统设计（重写核心）

### 6.1 键位模型（布局无关，修复缺陷 #1/#3/#4）

- 规范表示 = **USB HID Usage ID**（硬件键位）：Windows 扫描码↔HID 映射、macOS 虚拟键码↔HID、X11 keycode（基于 evdev，XKB 下已是物理键）。
- 绑定存储 `{ hid_key, modifiers: {ctrl,alt,shift,meta} }` → **同一物理键位在三端语义一致**，换键盘布局不影响触发；显示名按当前布局在 UI 层解析（`Win+Alt`、`⌘⇧R`）。
- 支持**任意 HID 主键**（F 区/方向/空格/回车/小键盘/标点/媒体键…）+ 4 修饰键任意组合；修饰键集合匹配语义=「精确集合」（沿用现状严格性，符合直觉）。

### 6.2 配置模型（多绑定，修复 #7 单键单动作）

```rust
struct HotkeyBinding { action: HotkeyAction /* Record toggle|start|stop|cancel, ShowWindow, ShowSettings... */,
                       hid_key: Option<HidKey>, modifiers: ModSet, trigger: TriggerMode /* Toggle | Hold */,
                       enabled: bool }
```
迁移：旧 `hotkey{modifiers,key}` + desktop-settings.json → `[{action: Record, trigger: Hold(默认，兼容现状), …}]`。
v1 设置页 UI 暴露 1 个录音绑定 + 触发模式切换 + 冲突提示；架构支持 N 绑定。

### 6.3 平台实现矩阵

| 平台 | 注册（按下+吞键+冲突检测） | 释放检测 | 权限/限制 |
| --- | --- | --- | --- |
| Windows | `RegisterHotKey`（windows crate；注册即系统级吞键，占用即失败=冲突可检测；MOD_NOREPEAT 防自动重复） | `WH_KEYBOARD_LL` 低级钩子跟踪成员键 key-up（仅监听不吞） | 无需特权 |
| macOS | Carbon `RegisterEventHotKey`（global-hotkey crate；事件派发本应用=等效吞键） | `CGEventTap` 监听 key-up（仅监听） | 辅助功能权限（沿用现检测+引导流程）；无权限时热键仍可用（按下事件可用），按住模式降级提示改用点按 |
| Linux X11 | `XGrabKey`（抓取即吞键；BadAccess=冲突可检测） | 同一抓取天然收到 KeyRelease | 需 X11；键盘布局变更时重抓 |
| Linux Wayland | XDG portal `GlobalShortcuts`（compositor 注册 UI，activated/deactivated 双信号可支撑按住） | portal 信号 | 组合键由 compositor 分配，**不可由应用任意指定**（平台限制，如实告知）；无 portal 支持时明确降级提示（优于现状静默失效） |

全局：`HotkeyBackend` trait 统一（§4.6）；三端上层触发语义完全一致。

### 6.4 触发状态机（Strategy）

```
点按 Toggle:  Pressed ─► 翻转发令（Idle→start；Active→stop）；Released 忽略
按住 Hold:    Pressed ─► start；Released ─► stop；KeyStateLost ─► stop（合成释放）
公共:         Preparing 中收到 Pressed(start)→若再按/再松按目标模式排队（pending_stop 语义，Actor 内实现）
自动重复:     RegisterHotKey MOD_NOREPEAT + held-set 去重（替代 250ms 防抖——缺陷 #10 不再需要防抖，因为重复事件在源头被抑制）
```

### 6.5 防粘滞三重防线（修复缺陷 #8）

1. **真实键态重同步**：系统事件（见 2）与 Hold-active 期间每 2s，用 `GetAsyncKeyState`/`CGEventSourceKeyState`/`XQueryKeymap` 查询物理键态，与 held-set 比对，不一致→合成 release+重置。焦点切换（Win 开始菜单吞 Win-up 等）由此收敛。
2. **系统事件强制重置**：休眠/唤醒（WM_POWERBROADCAST / NSWorkspaceDidWake / logind PrepareForSleep）、会话锁屏解锁（WTS_SESSION_CHANGE / logind Lock）→ `resync_all()`；若 Hold 录音进行中→同步下发 stop。
3. **会话看门狗兜底**：流式收尾 15s / 多段收尾 30s / 优化 60s（沿用现值）；看门狗超时也触发一次键态重同步排除键因。

### 6.6 冲突检测（修复缺陷 #6/#7）

- **应用内**：保存时对绑定表做规范化查重（同 HID+修饰集合），重复→拒绝并高亮冲突项。
- **系统级**：注册失败（RegisterHotKey 失败/XGrabKey BadAccess）→ 返回 `HotkeyError::Conflict` → 设置页展示「被其他应用占用」。
- **预检**：保存前 try-register→立即注销（dry-run），实时反馈而非保存后才发现。

### 6.7 阶段① 20 条缺陷 → 设计元素对照（全部覆盖）

| 缺陷# | 修复设计 |
| --- | --- |
| 1-5（键位能力） | §6.1 任意 HID 主键 + 布局无关 + 显式修饰集合语义（Shift+数字天然可用：按物理键判定） |
| 6（无注册/吞键） | §6.3 系统级注册三平台（macOS 例外已声明） |
| 7（单键单动作） | §6.2 多绑定模型 |
| 8（粘滞） | §6.5 三重防线 |
| 9（换键不发 release） | 换绑定时后端统一 `resync_all` + 合成旧绑定 release 事件 |
| 10（250ms 防抖吞按） | MOD_NOREPEAT + held 去重（源头抑制，无需时间窗） |
| 11（崩溃不重启） | 热键在进程内，无子进程可崩；后端线程 panic=启动期自检+运行期 supervise 重初始化 |
| 12（请求无超时） | 无跨进程请求；命令全部带超时 |
| 13（agent-ready 丢失） | 无 agent；就绪态是状态机属性，可查询（get_recording_state） |
| 14（macOS 无权限静默） | `probe()` 显式能力上报 + 设置页状态卡（沿用权限引导文案） |
| 15（业务在渲染层） | §1.2 控制面在 Rust；渲染层只显示 |
| 16（无点按模式） | §6.4 TriggerMode |
| 17（延迟链路） | 热键→Actor→开设备，全程进程内（目标 <50ms 到开始收数，替代现状 ~385ms+IPC） |
| 18（归一化 4 份） | D7 单一实现于 `settings` |
| 19（Linux/Wayland） | §6.3 X11 主路径 + portal + 显式降级提示 |
| 20（静音联动） | §9.4：默认设备跟随（IMMNotification/coreaudio 属性监听/pactl）、异常路径统一恢复（drop 守卫） |

---

## 7. 状态机与事件总线

### 7.1 RecordingSessionActor 状态机

```
Idle ─start(mode)→ Preparing ─ready→ Recording(single|multi|streaming)
Preparing/Recording ─stop/cancel→ Finalizing ─→ Transcribing ─→ (auto) Optimizing ─→ Done → Idle
任意态 ─error→ Failed → Idle（错误原因落 Record.error，修复已知问题#7）
Preparing 期间 stop → 排队（pending_stop，进 Recording 即刻收口——三防御之三）
```
- 输入：SessionCmd{Start,Stop,Cancel,HotkeyEvent,ProviderEvent,Watchdog}；输出：AppEvent + Provider/设备控制。
- 旧 useRecorder 的 9 种「旧模式×新模式」组合在此收敛为状态机迁移（Rust 单测穷举）。

### 7.2 Record 状态机（不变，01 §5.3）

pending→transcribing→transcribed→optimizing→optimized / →failed；优化失败复位 transcribed；失败写 error 原因。

### 7.3 Provider 流式会话状态机

`Connecting→Ready{push}→Finishing→Done | Failed(断线兜底=已收 partial 建 Record+degraded) | Cancelled`。

### 7.4 事件清单（节选；前端桥按需重放旧名）

| AppEvent | → 前端（Tauri event 名） | 替代现状 |
| --- | --- | --- |
| RecordingStateChanged{phase,mode,elapsed} | recording:state | livePhase/appStore 派生 |
| LivePartial{text,is_final} | recording:partial | stream_partial（WS） |
| AudioLevel{rms} | recording:level | audioLevel ref |
| SegmentTranscribed{seq,text} | recording:segment | segment_transcribed（WS） |
| RecordChanged{id,status,…} | transcription_completed 等旧名重放 | /api/ws 12 类事件 |
| OptimizationDelta/… | optimization_* 旧名重放 | 同上 |
| HotkeyTriggered{action,phase} | hotkey:press / hotkey:release | IPC |
| ClipboardCaptured | clipboard:changed | IPC |
| DesktopPermission{msg} | desktop:permission-required | IPC |

---

## 8. 数据存储与迁移

### 8.1 布局：与现状逐字节兼容（01 §3.7 表不变）

唯一变更：新增 `data/hotkeys.json`（或并入 settings.json 的 `hotkeys` 键——**采用并入 settings.json**，少一个文件）；删除 `.writer.lock`、`data/prompts/presets.json`、`wordbook/`（不迁移）。

### 8.2 原子写与锁

`tmp+fsync+rename` 保留；进程内按资源路径 `Mutex`；跨进程保护=单实例插件（二次启动唤起已有窗口）。

### 8.3 加密兼容

fernet crate（0.2）+ 现有 `.encryption_key`（44B Fernet 密钥）→ 直接解密 `doubao_asr_api_key_encrypted` / `llm_v2.json` 密文；阶段 3a 写**往返兼容测试**（用 Python 产出的密文样本解密 + Rust 加密后 Python 可解）。GET /api/settings 仍只回报 `_set` 布尔。

### 8.4 一次性迁移（首启动，幂等，产出报告）

1. settings.json：剥离退役键；`hotkey`+desktop-settings.json → `hotkeys[]`（trigger 默认 Hold=现状行为）。
2. llm_v2.json/records/sessions/clipboard/transcribe_file_cache：格式不变直接沿用。
3. **音频错位修复**（01 缺陷#3）：`audio/records/*.wav` ↔ record.audio_path/duration 启发式匹配（时长误差 <50ms 优先、再退化到时间邻接）→ 重命名为 `{record_id}.wav`；匹配失败的 wav 移入 `audio/orphans/` 并在迁移报告列出；record 缺音频标记 `audio_missing`。
4. 迁移报告写入 `data/migration-report.json`，设置页诊断区可查看。

### 8.5 配置单一真相源

- `settings` 模块唯一持有默认值+校验+归一化（含热键）；HTTP PUT 白名单契约（未知键丢弃/null=未提供/400 中文）照旧；MODEL_RUNTIME_KEYS 副作用（停服务+重建 provider）照旧。
- env 覆盖（Settings）保留给高级用户与桥接配置；桌面专属设置不再有第二存储。

---

## 9. 桌面壳设计（att-desktop）

| 能力 | Rust 方案 | 平台差异/修复点 |
| --- | --- | --- |
| 托盘 | tauri tray；四态图标资产沿用（win 七档 DPI 按主屏 scale 选档，逻辑移植）；菜单=最近 10 条+设置+退出 | 记录数据结构沿用 records.js:20-27 形状 |
| 悬浮窗 | WebviewWindow（floating.html）：transparent+always_on_top+skip_taskbar+visible_on_all_workspaces+预创建隐藏；底部居中定位逻辑移植 | Linux 透明需 `WEBKIT_DISABLE_COMPOSITING_MODE=1`（已知 Tauri 事项）；状态推送改走 Tauri 事件（floating:state） |
| 剪贴板监控 | arboard 轮询 800ms（语义移植：CRLF 规范化/内容哈希去重/内部写 5s TTL/图片 PNG 哈希落盘——落盘目录跟数据目录，**修复 asar 只读路径缺陷**） | 图片写剪贴板 arboard 支持 |
| 粘贴模拟 | enigo（Ctrl/Cmd+V） | 去掉「每次 spawn Python」的 200ms 延迟 |
| 录音静音 | Windows `IAudioEndpointVolume`（IMMNotificationClient 跟随默认设备）；macOS coreaudio `kAudioDevicePropertyMute`（默认设备监听，osascript 兜底）；Linux `pactl set-sink-mute @DEFAULT_SINK@`（**Linux 从不支持变为支持**） | Drop 守卫保证任何退出路径恢复；录音中用户手动改静音→以「结束时查询实际值」替代盲写快照（修复缺陷 #20 覆盖问题） |
| 单实例 | tauri-plugin-single-instance（二次启动唤起主窗） | — |
| 对话框/拖拽 | tauri-plugin-dialog（音视频过滤器移植）；拖拽经 Tauri onDragDropEvent 原生拿路径（替代 webUtils） | — |
| 窗口 | Tauri window API；自绘标题栏 `data-tauri-drag-region` | macOS 红绿灯隐藏沿用现方案语义 |

---

## 10. 前端适配边界 v2（对阶段①附录 §10 的修订）

### 10.1 IPC 契约映射（useElectron.js 导出形状不变，内部换实现）

| 旧 electronAPI | 新实现 | 备注 |
| --- | --- | --- |
| getPort | invoke `get_backend_port` | 随机端口由 Rust 启动时分配 |
| minimize/maximize/close、setTrayIcon、updateTrayMenu、floating:*、debugLog | 对应 invoke（命令名保持 camelCase 语义） | send 类改为 invoke 无返回 |
| readClipboardText/writeClipboard/writeImage/pasteClipboard | invoke（arboard/enigo） | |
| syncInputSettings/syncDesktopSettings/syncRecordingState/get/saveDesktopSettings | invoke（写 settings.json 单一真相源；热键即时重绑） | |
| openMediaFile / getPathForFile | dialog 插件 / 拖拽事件 | |
| onHotkeyPress/Release、onPasteText、onNavigateSettings、onClipboardChanged、onDesktopPermissionRequired、onDesktopAgentError | listen（**事件名保持旧名**） | agent-error→desktop:error 语义保留 |
| **新增** | listen recording:state/partial/level/segment；invoke list_audio_devices/set_audio_device、hotkey 配置命令 | 麦克风枚举不再走 getUserMedia |
| api.*（44 个 HTTP 透传） | **不变**（仍走 client.js→内嵌 axum） | 仅 getBackendPort 换源 |

### 10.2 WS 替换

`useWebSocket.js` 删除；新 `useBackendEvents.js`：listen 上述事件（含旧名重放）→ 分发到原 handleWebSocketMessage 同形状的处理器（迁入 useRecordingBridge）。连接状态徽章改为 Rust health 状态事件。

### 10.3 useRecorder 替换（stores 形状不变，组件零改动）

- 新 `useRecordingBridge.js`（目标 <200 行）：invoke start/stop/cancel + listen 状态/实时文本/音量/分段 → 写入 appStore（isRecording/isTranscribing）、`livePhase/liveTranscript/liveSegments/audioLevel` 模块级 ref（同导出名）、optimizationStream store。
- RecorderSection/RecordCard/FloatingWindow 等组件**不改**（消费的响应式形状保留）。
- recorder/*.mjs 中随架构消亡的：pollPolicy（轮询被事件替代）、streamProtocol（协议进 Rust）；其余移植 Rust 并以单测锁定（§12）。

### 10.4 必动文件清单（最终版）

useElectron.js（主战场）、api/client.js（getBackendPort）、stores/settings.js（getPlatform→os 插件）、FloatingWindow.vue（floatingAPI→事件）、TitleBar.vue+electron-overrides.css（drag-region）、index.html（CSP+`ipc:`）、vite.config.js（Tauri 对接）、App.vue（isElectron→isTauri）、useWebSocket.js→useBackendEvents.js、useRecorder.js→useRecordingBridge.js、SettingsPage 麦克风枚举/热键录入区（新命令+触发模式 UI）。

### 10.5 UI 动效（阶段 3h，/ui-ux-pro-max）

在现令牌体系（三档时长+统一缓动+reduced-motion）内补：录音钮状态过渡、波形条呼吸/峰值、面板进入退出、流式文本光标、加载骨架微动效、页面切换淡入。约束：仅 transform/opacity/composite 属性；不新增 JS 动画引擎；波形用 CSS 驱动（数据经 30Hz 事件已是低频）。

---

## 11. 安全设计

- 密钥：仅运行期解密于内存；日志统一脱敏过滤器（移植 redact_secrets）；GET 响应排除密文字段（契约测试锁定）。
- 网络：axum 仅绑定 127.0.0.1（随机端口）；CSP 更新 `ipc:` 并保留回环白名单；豆包 WS `proxy=None` 直连语义保留。
- 文件转写 token（X-ATT-Token）契约保留（外部脚本用户）。
- 拖拽/剪贴板图片路径校验沿用「必须落在数据目录内」规则。

---

## 12. 测试策略

### 12.1 用例移植映射

| 现前端 .mjs | 去向 |
| --- | --- |
| test_recorder_pcm / segment_policy / tray_state / floating_session / stop_flush_order / recording_stop_state | **移植 Rust 单测**（audio::pcm、SegmentPolicy、tray 派生、floating 语义、收尾顺序由状态机单测锁死、停止排队迁移穷举） |
| test_recorder_poll_policy / stream_protocol | 随架构消亡（轮询/WS 协议不复存在） |
| test_status_copy / settings_form_sync / clipboard_card_actions / export_download / llm_logic / transcribe_file_model / streaming_ui(streamScroll) / async_singleton | **保留前端**（纯 UI 逻辑不动） |
| 后端 pytest（350+） | 核心（存储/加密/设置契约/provider 协议编解码/超时与错误分级/housekeeping/索引/迁移）移植 Rust 单测；API 层用 axum 测试框架（tower::TestServer）保留契约测试（health 27 字段、裸数组、NDJSON、settings 白名单） |

### 12.2 平台与 CI

- CI：三平台 `cargo test`（att-core headless）+ 前端 node 测试 + vite build + `tauri build` 冒烟。
- SynthSource 合成音频全链路测试（无麦克风 CI 上跑三模式状态机）。
- 平台手工清单（每 OS 发版前）：热键注册/冲突/点按/按住/休眠唤醒/锁屏/焦点切换/默认设备切换/托盘/悬浮窗/静音恢复/粘贴。
- Fernet 往返兼容测试（Python 产密文样本 ↔ Rust）。

---

## 13. 实施阶段划分（阶段③，每步可验收）

| 步 | 内容 | 验收标准 |
| --- | --- | --- |
| 3a 骨架 | workspace+att-core 骨架+storage+settings+迁移+axum 数据面（records/settings/health/llm 只读）+Tauri 壳（主窗口/标题栏/单实例）+前端 useDesktop/client 适配 | 应用启动，前端页面全部可浏览，旧数据迁移完成且报告正确 |
| 3b 音频 | AudioSource(cpal)+Pipeline+单段模式+whisper.cpp Provider+Record 落库+音量/状态事件 | 热键未接前用 UI 按钮完成单段录音全链路；采样数=时长×16k 零误差 |
| 3c 快捷键 | att-hotkey 三平台+触发状态机+防粘滞+冲突检测+设置页热键 UI | 三平台清单通过（含休眠/锁屏/焦点用例）；点按/按住可用 |
| 3d 流式 | StreamingTranscriber+豆包 SAUC Provider+流式模式状态机 | 按下即说零丢字（并行建连+环缓冲）；尾包完整；断线兜底 |
| 3e 多段 | VAD+SegmentPolicy+分段转写+merge+看门狗 | 三防御语义复测；最后一片失败不再卡死 |
| 3f LLM | att-llm 网关+档案/模型存储（fernet 兼容）+流式优化+文件转写+滑窗校对 | NDJSON 契约测试通过；reasoning_effort/output_truncated 语义复测 |
| 3g 桌面壳 | 托盘/悬浮窗/剪贴板监控/粘贴/静音/对话框/拖拽 | 01 §1 全部联动行为复测；缺陷 #20 修复验证 |
| 3h 打磨 | 迁移报告 UI+打包（三平台）+自检+UI 动效（ui-ux-pro-max）+文档 | 三平台安装包产出+启动自检+动效过 reduced-motion 检查 |

---

## 14. 风险登记册

| # | 风险 | 等级 | 缓解 |
| --- | --- | --- | --- |
| 1 | whisper.cpp 与 faster-whisper 精度/速度差异（large-v3 CPU int8） | 中 | 3b 初期做基准对比；保留模型目录约定便于后续换引擎（开闭） |
| 2 | 本地 Qwen3-ASR 不可用（D5 取舍） | 中 | 远端桥接 Provider 保底；文档给自建指引；待用户确认 |
| 3 | macOS CGEventTap 权限/吞键边界 | 中 | 权限引导沿用；无权限降级点按模式；3c 实测清单覆盖 |
| 4 | Wayland portal 覆盖率 | 低（声明性） | 显式降级提示（优于静默）；X11 会话完美支持 |
| 5 | fernet crate 兼容边缘（padding/版本头） | 低 | 3a 往返测试+真实密文样本 |
| 6 | LLM 协议适配工作量（Anthropic/Gemini 流式） | 中 | D6 决策点确认 v1 覆盖范围后再定 |
| 7 | Linux 透明悬浮窗 | 低 | env 兜底已知方案；不行则不透明底色降级 |
| 8 | 音频错位迁移启发式误判 | 中 | 只重命名高置信匹配；孤儿隔离不删除；报告可人工复核 |
| 9 | 三平台测试设备可用性（当前开发机为 Windows） | 高 | CI 三平台+每 OS 发版手工清单；请用户确认 mac/Linux 实机可用性 |
| 10 | Tauri 2 WebView2 依赖（Win 老系统） | 低 | 安装器捆绑 WebView2 bootstrapper |

---

## 15. 待你拍板的决策点（确认后进入阶段③）

1. **D1-D4 总体确认**：采集/编排移入 Rust、内嵌 axum 数据面、移除两个 WS——前端组件零改动但 useRecorder/useWebSocket 被薄桥接替换（形状保留）。
2. **D5-Whisper**：本地引擎换 whisper.cpp（GGML 模型需重新下载 ≈3GB）。
3. **D5-Qwen**：本地 Qwen3-ASR 不随包（远端桥接保留，自建/Docker）；可接受？
4. **D6-LLM 覆盖**：v1 建议先覆盖现用集（OpenAI 兼容系+DeepSeek+Anthropic+Gemini+Ollama+custom），其余注册表项后续按需补——确认或要求全量 14 家。
5. **快捷键默认触发模式**：保持「按住」为默认（兼容现行为），用户可切「点按」——确认。
6. **迁移策略**：音频错位按高置信匹配修复+孤儿隔离（不删）；writer.lock 移除——确认。
7. **macOS/Linux 实机**：3c/3g 的平台验证是否有实机可用？（影响验收节奏）
