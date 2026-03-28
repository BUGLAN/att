# Qwen 真流式录音链路方案分析

## 1. 目标与结论

本次目标不是重写现有录音系统，而是在尽量不破坏现有代码的前提下，新增一条真正可用的 Qwen 流式 ASR 链路。

结论先行：

- 保留现有 `single` 单段录音。
- 保留现有 `multi` 多段录音。
- 新增第三种 `streaming` 流式录音模式。
- `streaming` 不复用当前“文件上传后异步转写”的接口抽象，而是新增独立的流式 Provider 适配层。
- Qwen 流式模式首期不接入强制对齐器，时间戳能力放到录音结束后的可选后处理阶段讨论。

这是当前最稳妥的方案。原因很直接：现有 `single` 和 `multi` 已经跑通，用户心智和 UI 也已经形成，直接把它们硬改成真流式，风险远大于收益。

## 2. 当前链路现状

### 2.1 `single` 模式

当前 `single` 模式的核心路径是：

1. 前端通过 `MediaRecorder` 录完整段音频。
2. 松开录音键后生成完整 `Blob`。
3. 上传到 `/api/audio/upload`。
4. 后端落盘为 WAV。
5. 后台任务调用 `ASRProvider.transcribe(...)` 异步转写。
6. 通过 `/api/ws/{client_type}` 广播 `transcription_started` / `transcription_completed`。

这条链路本质上是“整段录完再转写”。

### 2.2 `multi` 模式

当前 `multi` 模式的核心路径是：

1. 前端在 [useRecorder.js](/E:/repo/att/src/composables/useRecorder.js) 中使用静音检测器。
2. 每次检测到一句话结束，就停止当前 `MediaRecorder`，拿到一个完整片段 `Blob`。
3. 片段上传到 `/api/sessions/{id}/segments`。
4. 后端异步转写该片段。
5. WebSocket 广播 `segment_transcribed`。
6. 前端把各片段文本拼起来，最后再调用 `/api/sessions/{id}/merge` 生成最终记录。

这条链路本质上是“静音切段后的准实时分段转写”，不是“真流式”。

### 2.3 当前抽象边界

现有 [app/stt/base.py](/E:/repo/att/app/stt/base.py) 是典型的文件式批处理接口：

- `transcribe(audio_path)`
- `transcribe_chunk(chunk)`
- `transcribe_chunks(chunks)`

它假设输入是“已经成型的音频文件或切片文件”。这与真流式所需的“持续接收 PCM 帧并返回增量结果”不是一类问题。

## 3. 为什么不能把现有 `multi` 直接改成真流式

不建议把当前 `multi` 继续往里塞逻辑，原因有四个。

### 3.1 输入介质不同

当前 `multi` 依赖 `MediaRecorder -> 完整片段 Blob -> 文件上传`。

真流式需要的是：

- 浏览器持续采集 PCM 帧
- 按较小粒度推送到后端
- 后端持续喂给流式 ASR 会话
- 服务端持续回传 partial / final

这两个输入模型不兼容。

### 3.2 当前 WebSocket 不是音频流通道

现在的 `/api/ws/{client_type}` 主要承载：

- 托盘与网页的录音控制
- 状态同步
- 转写完成广播

它不是为高频音频帧传输设计的，也不适合直接混入二进制音频流。

### 3.3 当前 ASR Provider 是批处理接口

如果把流式逻辑强塞进现有 `ASRProvider`，会让原本清晰的文件式接口变得混乱，最终同时伤到 Whisper 和 Qwen 的可维护性。

### 3.4 当前 `multi` 已经有明确定位

`multi` 的定位其实已经很明确：

- 适合长按录音但又不想等整段结束
- 依赖本地静音切段
- 后端仍然用批量文件转写

它应该继续保留为“分段准实时模式”，而不是被重新定义成“流式模式”。

## 4. 推荐方案：三种模式并存

建议把录音模式明确拆成三类：

### 4.1 `single`

- 语义：单段录音，结束后整体转写
- 适合：短句、稳定场景
- 现状：保留现有实现

### 4.2 `multi`

- 语义：静音切段，分段上传，边录边逐段出字
- 适合：较长口述，但对“字必须实时滚动出来”要求不极端
- 现状：保留现有实现

### 4.3 `streaming`

- 语义：连续推送 PCM 音频帧，服务端实时返回增量文本
- 适合：真正需要实时字幕式体验
- 现状：新增独立链路

这样做的好处是：

- 不破坏既有用户行为
- 不污染既有批处理 ASR 接口
- 可以把流式相关复杂度隔离到单独模块中

## 5. 与 Qwen 官方能力边界的关系

截至 2026-03-27，我核对了 Qwen 官方仓库 README：

- Qwen3-ASR 官方明确提供 streaming inference。
- 该能力当前只在 vLLM backend 下可用。
- 官方同时明确说明：streaming inference 不支持返回 timestamps。

参考：

- [QwenLM/Qwen3-ASR GitHub README](https://github.com/QwenLM/Qwen3-ASR)

因此这里要做一个非常明确的架构判断：

- 真流式模式不能继续走当前本地 `Qwen3ASRModel.from_pretrained(...)` 这一类文件式调用思路。
- 真流式模式必须围绕 Qwen 的 vLLM streaming 能力设计。
- 强制对齐器不应进入“实时回字”主链路。

## 6. 强制对齐器的建议结论

目前我的建议是：

- `streaming` 模式首期默认不接 `Qwen3-ForcedAligner-0.6B`。
- 如果后续业务确实需要时间戳，再作为录音结束后的可选异步后处理能力接入。

原因：

- 官方说明流式推理本身不返回时间戳。
- 强制对齐更适合“已有完整音频 + 已有最终文本”的离线对齐。
- 把对齐器放进实时链路，会显著增加显存、延迟、状态复杂度和失败面。

更具体地说，强制对齐器更适合下面两类用途：

- 导出字幕或时间轴时补时间戳
- 录音结束后对最终文本做高精度对齐

它不适合承担“正在说话时实时出字”的主链路职责。

这块后面仍然值得单独讨论，但不应该阻塞真流式第一期。

## 7. 总体架构建议

推荐采用“录音模式层 + 流式传输层 + Streaming Provider 适配层 + 最终记录落盘层”的四层结构。

### 7.1 前端录音模式层

建议把当前 [useRecorder.js](/E:/repo/att/src/composables/useRecorder.js) 从“大一统实现”调整为“分发器”：

- `useRecorder.js` 负责模式分发和公共状态暴露
- `useSingleRecorder.js` 承载单段录音
- `useMultiRecorder.js` 承载现有静音切段逻辑
- `useStreamingRecorder.js` 承载真流式逻辑

这里的关键原则是：不要继续把 `streaming` 的状态机堆进现有 `useRecorder.js`。

### 7.2 前端采集层

`streaming` 模式建议使用：

- `AudioContext`
- `MediaStreamAudioSourceNode`
- `AudioWorklet`

由 `AudioWorklet` 直接输出 16kHz、mono、PCM16 或 float32 再转换为 PCM16 的短帧。

推荐发送粒度：

- Worklet 内部按 20ms 到 40ms 采样
- 前端聚合成 100ms 到 200ms 的网络包再发送

原因：

- 过细会导致 WebSocket 包过多
- 过粗会让流式延迟变差

### 7.3 传输层

建议新增独立的流式 WebSocket 通道，不复用 `/api/ws/{client_type}`。

推荐形态：

- 新增 `/api/asr/stream`
- 控制消息走 JSON
- 音频帧走二进制消息

建议消息协议：

客户端到服务端：

- `stream_start`
- `stream_audio`
- `stream_commit`
- `stream_finish`
- `stream_cancel`

服务端到客户端：

- `stream_ready`
- `stream_partial`
- `stream_final`
- `stream_finished`
- `stream_error`
- `stream_metrics`

说明：

- `partial` 表示可被后续修正的临时文本
- `final` 表示某段文本已稳定，可提交到 UI 主文本区
- `finished` 表示整场流式录音已结束，并且最终 record 已经可读

### 7.4 后端 Streaming Provider 适配层

这里建议不要修改现有 `ASRProvider`，而是新增一套独立抽象，例如：

- `StreamingASRProvider`
- `StreamingASRSession`

建议职责划分：

- `ASRProvider` 继续负责文件式单次转写
- `StreamingASRProvider` 负责会话式流推理
- `QwenStreamingProvider` 只负责 Qwen/vLLM 流式能力

推荐首期只做：

- `QwenStreamingProvider`

不建议为了“接口统一”去做一个假的 `WhisperStreamingProvider`。那会重新回到“拿 faster-whisper 伪装流式”的老问题。

### 7.5 Qwen 适配方式

在实现层面，建议把 Qwen 流式能力视为“独立后端能力源”，而不是继续嵌到当前批处理推理对象里。

更稳妥的方向有两个：

1. 应用后端内嵌 `qwen-asr[vllm]`，直接管理 Qwen 流式会话
2. 应用后端作为桥接层，连接到独立启动的 Qwen/vLLM streaming 服务

从最小风险和可调试性看，我更倾向于：

- 第一阶段先做“应用后端桥接 Qwen streaming 服务”
- 先把 APP 侧协议、状态机、录音 UX 跑通
- 后面再决定是否由应用自动拉起 Qwen 服务

原因：

- vLLM 生命周期管理本身比较重
- Electron 开发态下把模型加载、服务拉起、显卡占用、错误恢复全耦合在一个进程里，排障会比较痛苦
- 这个桥接服务不要求一定跑在 Linux / WSL2；如果 Windows 上的 `vllm-windows` 能稳定承载 `qwen-asr[vllm]`，同样可以部署在本机或独立 Windows GPU 机器上

## 8. 最小改动策略

### 8.1 不动现有批处理主干

以下能力尽量不改或只做轻改：

- `/api/audio/upload`
- `/api/sessions/{id}/segments`
- `/api/sessions/{id}/merge`
- 当前 `ASRProvider`
- 当前全局 WebSocket 状态广播逻辑

### 8.2 新增而不是替换

建议新增：

- `recording_mode = streaming`
- 新的前端 composable
- 新的流式 WebSocket 路由
- 新的 streaming provider 抽象
- 新的流式 session 生命周期管理

### 8.3 UI 层做能力门控

建议 `streaming` 模式只在满足以下条件时可用：

- `asr_provider === 'qwen_asr'`
- Qwen streaming 后端健康

否则：

- 可以在设置页显示该选项但置灰
- 或显示为可选，但进入录音时给出明确错误

前者更稳。

## 9. 详细改造点

## 9.1 前端

建议涉及文件：

- [src/composables/useRecorder.js](/E:/repo/att/src/composables/useRecorder.js)
- `src/composables/useSingleRecorder.js`
- `src/composables/useMultiRecorder.js`
- `src/composables/useStreamingRecorder.js`
- `src/composables/useStreamingSocket.js`
- [src/stores/settings.js](/E:/repo/att/src/stores/settings.js)
- [src/components/settings/SettingsPage.vue](/E:/repo/att/src/components/settings/SettingsPage.vue)

具体改动：

- 在设置中把 `recording_mode` 扩展为 `single | multi | streaming`
- `useRecorder.js` 只负责根据模式分发 start/stop
- 新增流式文本状态
  - `streamingPartialText`
  - `streamingCommittedText`
  - `streamingLatencyMs`
  - `streamingConnectionState`
- 流式模式不再依赖静音检测作为正确性前提
- 静音检测在 `streaming` 中最多作为后续优化项，不作为第一期必需项

### 9.2 后端

建议涉及文件：

- [app/api/routes.py](/E:/repo/att/app/api/routes.py)
- [app/services/websocket.py](/E:/repo/att/app/services/websocket.py)
- [app/stt/base.py](/E:/repo/att/app/stt/base.py)
- [app/records/models.py](/E:/repo/att/app/records/models.py)
- 新增 `app/stt/streaming_base.py`
- 新增 `app/stt/qwen_streaming_provider.py`
- 新增 `app/services/streaming_session_manager.py`

具体改动：

- 新增 `RecordingMode.STREAMING`
- 新增流式专用 WebSocket 路由
- 新增流式 session 管理器
- 新增 Streaming Provider 抽象
- 新增 Qwen streaming provider 实现
- 录音结束时将最终文本落成一个标准 `Record`

### 9.3 记录与音频落盘

为了不破坏现有“每条记录都能回看”的产品一致性，建议流式模式也保留最终音频文件。

推荐做法：

- 服务端在接收流式 PCM 时，同时顺序写入临时 WAV
- `stream_finish` 后关闭文件
- 用最终文本 + 最终 WAV 创建 `Record`

这样做有几个好处：

- 与现有 record 结构兼容
- 后续若要做强制对齐，有完整音频可用
- 不需要前端同时维护一套独立的“录音归档”逻辑

## 10. 流式协议建议

建议首版协议如下。

### 10.1 `stream_start`

客户端发送：

- `sample_rate`
- `channels`
- `format`
- `language`
- `recording_id` 或 `session_id`

服务端返回：

- `stream_ready`
- 服务端分配的 `stream_session_id`

### 10.2 `stream_audio`

客户端持续发送二进制 PCM 数据。

建议固定：

- `16k`
- `mono`
- `pcm_s16le`

### 10.3 `stream_partial`

服务端返回暂态文本：

- 允许覆盖上一次 partial
- 不直接写入最终 record

### 10.4 `stream_final`

服务端返回稳定文本片段：

- 前端把它追加到 committed transcript
- 可选带 `segment_index`

### 10.5 `stream_finish`

客户端发送结束命令后：

- 服务端冲刷剩余缓存
- 关闭 Qwen 流式会话
- 关闭 WAV 写入
- 创建最终 `Record`
- 返回 `record_id`

## 11. 状态机建议

### 11.1 前端状态机

建议：

- `idle`
- `connecting`
- `streaming`
- `finalizing`
- `completed`
- `error`

### 11.2 服务端状态机

建议：

- `created`
- `ready`
- `receiving_audio`
- `flushing`
- `completed`
- `cancelled`
- `failed`

原则：

- partial 文本只存在于流式会话上下文中
- 最终 `Record` 只在 `completed` 时落盘

## 12. 兼容性与回退策略

### 12.1 对既有功能的影响

本方案对既有功能的影响应当控制在最低：

- `single` 不变
- `multi` 不变
- 只有在选择 `streaming` 时才进入新链路

### 12.2 失败回退

流式模式应支持明确失败回退：

- Qwen streaming 服务不可用时，不影响 `single` / `multi`
- 流式录音中途失败时，允许保留已确认文本
- 是否自动回退到 `multi`，建议首期不要自动做，避免用户误解模式语义

## 13. 风险分析

### 13.1 最大风险：vLLM 依赖较重

这是这条链路最大的现实风险。

影响包括：

- GPU 显存要求明显高于当前批量文件转写
- 服务初始化时间长
- 依赖问题更复杂

对于 Windows，还要额外关注 `vllm-windows` 的版本匹配限制，例如 Python、CUDA、torch 组合是否与已发布 wheel 对齐。

因此必须把“Qwen 流式引擎不可用”设计成一个可见、可诊断、可隔离的问题。

### 13.2 第二风险：前端音频采集实现复杂度上升

`AudioWorklet` 比 `MediaRecorder` 更底层，调试成本更高。

但这是值得的，因为：

- 真流式需要可控的 PCM 帧
- 继续依赖 `MediaRecorder` 无法得到真正稳定的低延迟流推理输入

### 13.3 第三风险：partial 文本会抖动

流式 ASR 的常见现象是：

- 暂态文本会被修正
- 用户看到的字会来回变化

因此 UI 必须区分：

- 已确认文本
- 正在修正的临时文本

不要把 partial 直接混写进最终文本区。

## 14. 分阶段实施计划

### 阶段一：架构打底

- 扩展 `recording_mode`
- 拆分前端 recorder composable
- 新增 `StreamingASRProvider` 抽象
- 新增流式 WebSocket 路由与 session 管理

交付目标：

- 项目结构能同时容纳批处理和流处理两种 ASR 形态

### 阶段二：打通最小可用链路

- 前端 `AudioWorklet` 采集 PCM
- 建立独立 streaming WebSocket
- 接入 Qwen/vLLM streaming provider
- UI 展示 partial / final
- 录音结束后生成最终 `Record`

交付目标：

- 用户按住讲话时，界面可以连续看到 Qwen 流式结果

### 阶段三：稳定性与体验优化

- 增加健康检查与错误提示
- 增加延迟指标与连接状态
- 优化断线、取消、超时处理
- 评估是否增加“结束后异步强制对齐”

交付目标：

- 真正可日常使用，而不是只能 demo

## 15. 最终建议

这个需求适合做，而且应该做成“新增第三模式”的增量改造，不应该去重定义现有 `multi`。

如果要一句话总结推荐方案，就是：

`single` 保持整段批处理，`multi` 保持静音切段准实时，`streaming` 单独走 Qwen vLLM 真流式链路。

关于强制对齐器，我当前建议是：

- 第一阶段先不接入流式主链路
- 等真流式链路稳定后，再讨论是否在“录音结束后”异步补时间戳

这样最符合“真正可用”和“尽量不破坏原有代码”这两个目标。
