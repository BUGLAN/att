# 静音切段与准实时转写设计文档

## 背景

当前录音链路是整段录音结束后再上传，再由后端异步转写。

这条链路的优点是稳定、简单、现有 UI 和状态也已经比较和谐；缺点是当用户长按很久时：

- 单次音频很长
- 上传和转写等待更久
- 一旦中间有明显停顿，也不会提前产出文本

你希望的新能力是：

- 能识别静音
- 在录音过程中分段产出文本
- 不要推翻现有实现，尽量只加代码

## 结论

这个功能不是“很小的补丁”，但也不需要重写现有系统。

复杂度判断：`中等偏上`

原因：

- 前端要新增一套静音检测和分段录音控制
- 录音流程要从“单段上传”变成“会话 + 多段上传 + 最终合并”
- UI 要支持实时累积文本

但它不是从零开始，因为后端已经有相当多的基础设施：

- 设置里已经有 `recording_mode`、`silence_threshold`、`silence_level`
- 后端已经有 `/api/sessions`、`/api/sessions/{id}/segments`、`/api/sessions/{id}/merge`
- WebSocket 已经有 `segment_transcribed`
- 数据模型里已经有 `RecordingSession`、`AudioSegment`

所以更准确地说，这个需求适合做成：

`在现有单段模式不动的前提下，新增一个多段准实时模式`

## 不建议的方案

### 方案 A：直接做真正流式 ASR

即录音过程中不断把 PCM 音频流推给后端，后端持续解码并实时返回字词。

这个方案理论上体验最好，但不适合当前项目直接上：

- 现有前端是 `MediaRecorder -> Blob -> 上传文件`
- 现有后端是 `文件转写`，不是 `音频流转写`
- faster-whisper 目前在项目里的接法也是离线文件式，不是增量 decoder 式
- WebSocket 协议、状态机、容错都要重做

结论：`复杂度高，不符合“尽量不动现有代码”的目标`

## 推荐方案

### 方案 B：静音切段 + 会话聚合 + 准实时输出

核心思想：

- 热键按下后，不再只录一个大文件
- 录音过程中实时检测音量
- 当检测到一段语音结束并进入静音区间时，关闭当前录音段
- 把这一段作为一个完整文件上传到 session
- 后端异步转写这一段
- 前端收到 `segment_transcribed` 后把文本追加到实时预览区
- 热键松开时，结束最后一个片段，并调用 `merge` 生成最终记录

这不是“真正逐字实时”，但从用户感受看已经足够接近：

- 说一句，停一下，很快就能看到这一句出来
- 不需要等整整一分钟录完再出结果

## 为什么推荐这个方案

### 1. 现有单段模式可以完全保留

把当前逻辑继续作为 `single` 模式。

新增 `multi` 模式时才走新链路。

这样可以保证：

- 旧功能不回归
- 风险隔离
- 可以灰度验证

### 2. 最大复用现有后端能力

后端已经有：

- session 创建
- segment 上传
- segment 后台转写
- `segment_transcribed` 广播
- session merge

缺的主要是前端把它们真正串起来。

### 3. 不需要引入新的 ASR 服务形态

继续使用现有 Whisper 文件转写能力即可。

不需要把后端改造成流式音频网关。

## 当前代码现状分析

### 前端现状

当前 [useRecorder.js](E:/repo/att/src/composables/useRecorder.js) 是标准的单段录音流程：

1. `startRecording()`
2. `MediaRecorder` 持续采集
3. `stopRecording()` 后合并成单个 Blob
4. 调 `uploadAudio()`
5. 后端转写整段音频

这条链路天然不支持“中途出字”。

### 后端现状

[routes.py](E:/repo/att/app/api/routes.py) 已经存在多段会话接口：

- `POST /api/sessions`
- `POST /api/sessions/{session_id}/segments`
- `POST /api/sessions/{session_id}/merge`
- `DELETE /api/sessions/{session_id}`

同时也已经有 `segment_transcribed` 的 WebSocket 消息。

### 设置现状

[config.py](E:/repo/att/app/config.py) 和 [settings.js](E:/repo/att/src/stores/settings.js) 已经有这些字段：

- `recording_mode`
- `silence_threshold`
- `silence_level`

所以配置面也不需要重开模型。

## 目标边界

### 本期目标

- 支持静音切段
- 支持片段级准实时转写
- 支持实时累积文本预览
- 最终生成一条完整 record
- 尽量不改现有 `single` 模式

### 非目标

- 不做逐词级 streaming ASR
- 不做服务端持续音频流解码
- 不做边录边优化
- 不改现有整段录音的默认行为

## 总体设计

### 模式分流

录音入口保持不变，但在开始录音时分流：

- `recording_mode === 'single'`：继续走现有逻辑
- `recording_mode === 'multi'`：走静音切段逻辑

### 新链路

1. 用户按下快捷键
2. 前端创建 session
3. 开启麦克风采集
4. 用 `AudioContext + AnalyserNode` 监测实时音量
5. 当音量高于阈值时，进入 speech 状态
6. 当 speech 结束且静音持续超过阈值时，结束当前 segment
7. 上传 segment 到 session
8. 继续监听下一段 speech
9. 后端转写完成后通过 `segment_transcribed` 返回文本
10. 前端把文本追加到实时结果区
11. 用户松开快捷键时，结束最后一段
12. 调用 `merge` 生成最终 record
13. UI 切到完成态

## 关键状态机

建议新增一套只服务于 `multi` 模式的轻量状态机：

- `idle`
- `arming`
- `listening`
- `speaking`
- `segment_stopping`
- `segment_uploading`
- `waiting_transcription`
- `session_finalizing`
- `completed`
- `error`

其中最关键的不是 UI 文案，而是两个事实：

- 当前有没有打开的 segment
- 当前 session 是否仍然有效

## 静音检测设计

### 建议实现

使用 Web Audio API：

- `AudioContext`
- `MediaStreamSource`
- `AnalyserNode`

通过定时采样计算短时能量或 RMS。

### 建议参数

- `silence_level`: 默认 `-40 dB`
- `silence_threshold`: 默认 `1.5s`
- `min_segment_duration`: 建议 `0.8s`
- `min_speech_activate_ms`: 建议 `250ms`
- `segment_cooldown_ms`: 建议 `300ms`

### 判定逻辑

- 连续超过 `silence_level` 达到 `min_speech_activate_ms`，才认为开始讲话
- 讲话后若连续低于阈值超过 `silence_threshold`，认为一段结束
- 太短的片段直接丢弃，不上传

这样可以避免：

- 呼吸声就开段
- 键盘声导致误触发
- 一次短顿就频繁切片

## 录音分段策略

### 为什么不能直接复用当前 1 秒 timeslice

当前实现虽然 `MediaRecorder.start(1000)` 会周期性触发 `dataavailable`，但项目文档已经说明：

- 单个 timeslice chunk 不能可靠作为独立可转写文件

所以推荐策略不是“直接上传每个 chunk”，而是：

- 在一个 segment 生命周期内持续收集 chunk
- 在 segment 结束时调用一次 `mediaRecorder.stop()`
- 拿到完整 Blob 后上传
- 然后立刻创建下一段 recorder 或重新 start

### 推荐的 segment 生命周期

1. 检测到开始说话
2. 若当前没有 segment，则启动一个新的 `MediaRecorder`
3. 讲话中持续缓存 chunk
4. 静音达到阈值后 stop 当前 recorder
5. 生成完整 Blob
6. 上传到 `/sessions/{id}/segments`
7. 回到监听态，等待下一段讲话

## 后端设计

### 可以直接复用的部分

[routes.py](E:/repo/att/app/api/routes.py) 已经具备：

- session 创建
- segment 上传
- segment 后台转写
- `segment_transcribed`
- merge

### 建议补充

为了让前端更好收尾，建议增加两个很小的增强，但不是硬性必须：

1. `session_merged` WebSocket 事件
   用于在 `merge` 完成后广播最终 record 信息

2. merge 返回结构补充 `raw_text`
   便于前端不用再多查一次 record

这两项都属于增量增强，不会破坏现有接口。

## 前端设计

### 推荐新增模块

- `src/composables/useRealtimeRecorder.js`
- `src/composables/useSilenceDetector.js`

不要把所有逻辑继续堆进现有 [useRecorder.js](E:/repo/att/src/composables/useRecorder.js)。

更好的结构是：

- `useRecorder.js` 继续做模式分发
- `useSingleRecorder()` 保留原有逻辑
- `useRealtimeRecorder()` 处理 multi 模式

### 推荐新增 API

在 [client.js](E:/repo/att/src/api/client.js) 里新增：

- `createSession()`
- `uploadSessionSegment(sessionId, blob, filename)`
- `mergeSession(sessionId)`
- `deleteSession(sessionId)`

### UI 建议

浮窗和主界面都新增一个“实时文本缓冲区”概念：

- `finalText`
- `pendingSegmentText`

当 `segment_transcribed` 到达时：

- 把该片段文本 append 到 `finalText`
- 清空该段 pending 状态

最终 merge 完成后：

- 用后端最终 merged text 覆盖一次本地拼接结果

这样可以避免前端本地拼接与后端最终文本不一致。

## 与现有代码的关系

### 保持不动的部分

- 现有 `single` 模式上传与转写链路
- 现有 record 数据结构
- 现有优化链路
- 现有悬浮窗基础结构

### 新增的部分

- 静音检测
- session 录音控制
- segment 级上传
- segment 级实时文本展示

### 需要轻改的部分

- `useRecorder.js` 入口分流
- `useWebSocket.js`/消息消费对 `segment_transcribed` 的真正接入
- 设置页文案，明确 `single / multi` 的区别

## 风险分析

### 风险 1：切段过碎

原因：

- 静音阈值太激进

后果：

- 片段太多
- 上传频繁
- 文本碎裂

缓解：

- 设置最短语音激活时长
- 设置最短 segment 时长
- 设置 segment 冷却时间

### 风险 2：前后段边界丢字

原因：

- 在静音边界停得过于干净

缓解：

- 在停止前保留一小段尾音余量
- 或者在下一段开头增加少量前滚缓存

本期建议先不做复杂预滚缓存，先靠 `silence_threshold + speech_pad` 控制。

### 风险 3：多段模式状态比单段模式更复杂

原因：

- 一个热键会话内存在多个 segment

缓解：

- 单独维护 session 状态
- 不要把单段和多段逻辑混写

### 风险 4：最终合并文本和实时展示文本不一致

缓解：

- 前端实时文本只做预览
- 最终以 `merge` 结果为准覆盖

## 实施建议

建议分三期做，不要一次把所有体验细节全塞进去。

### 第一期：打通最小可用链路

- 保留 `single`
- 新增 `multi`
- 前端做静音检测
- 用 session/segment 接口完成分段上传
- 收到 `segment_transcribed` 后实时追加文本
- 松键后 `merge`

目标：

- 功能跑通
- 用户能感知“边说边出字”

### 第二期：打磨稳定性

- 调整静音参数
- 处理超短段丢弃
- 处理 merge 前最后一段未完成的等待逻辑
- 处理取消录音/异常恢复

### 第三期：优化体验

- 浮窗显示“实时结果 + 当前监听状态”
- 允许主界面看到 session 过程
- 为多段模式增加更明确的 toast 和状态提示

## 推荐文件改动范围

### 新增

- `src/composables/useRealtimeRecorder.js`
- `src/composables/useSilenceDetector.js`
- `plans/silence-realtime-transcription.md`

### 修改

- [useRecorder.js](E:/repo/att/src/composables/useRecorder.js)
- [useWebSocket.js](E:/repo/att/src/composables/useWebSocket.js)
- [client.js](E:/repo/att/src/api/client.js)
- [SettingsPage.vue](E:/repo/att/src/components/settings/SettingsPage.vue)
- [FloatingWindow.vue](E:/repo/att/src/components/floating/FloatingWindow.vue)

### 可选修改

- [routes.py](E:/repo/att/app/api/routes.py)
  仅在需要补 `session_merged` 事件时改

## 最终建议

如果你的目标是：

- 不推翻现在的系统
- 只做增量增强
- 让长按时不必等整段结束才看到结果

那么最合适的方向不是“真流式转写”，而是：

`静音切段 + session 聚合 + 准实时输出`

这个方案和你现有架构是兼容的，且后端已经有一半基础能力。

它不算特别简单，但绝对比“重做成流式 ASR”简单很多，也更符合你现在“代码已经很和谐，尽量不要拆”的要求。
