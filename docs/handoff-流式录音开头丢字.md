# Handoff：真流式录音「按下热键后立刻说话，开头丢字」

> 生成时间：2026-10-11 03:0x（本地）
> 基线提交：`04c75f4 build(deps): 引入 pnpm 锁文件与 workspace 配置`
> 本次排查**只读代码 + 解析日志，未改动任何源码**；修复留待接手会话完成。
> 行号基于上述基线，动手前先 `git log --oneline -1` 确认没有偏移。

> **状态（接手会话，2026-10-11）**：§4.1 主改动与 §4.3 埋点已执行；另补了 §5.1 要求的
> **采集代次守卫**（并行后若松手放弃路径不回滚 worklet，会留下幽灵采集：麦克风与
> AudioContext 永久泄漏、积压队列无限增长）。§4.2 按原文建议**跳过**——
> `STREAM_CONNECT_TIMEOUT_MS = 8000` 已把积压上界限死在 8s。
> 结果见 `docs/已知问题.md` 速览表 #23 与文末「已修复」表。
>
> **后续（复核阶段发现，已一并修复）**：
> 1. §6 验收第 3 条「快速点按不产生幽灵录音」**原文并不成立**——两个 stop 函数的启动窗口判据
>    是 `startPending && !isRecording`，而 `isRecording` 在 prepare 开麦克风之前就被置真，
>    整个启动窗口内判据恒为假（详见 `docs/已知问题.md` 文末「已修复」）。现已改为排队判据
>    `shouldQueueStopDuringStart({ startPending, fromStart })`。
> 2. 与「开头丢字」对称的**停止丢尾包**也已修掉（尾包冲刷提到 `cleanupStreamingAudio()` 之前）。

---

## 1. 任务

修掉这个用户可感知的缺陷：

> 按住快捷键（默认 `Cmd+Alt`）**立刻开口说话**时，最前面一小段语音不会被转写出来。

目标：把"按下热键 → 音频开始被采集"的窗口从实测的 **0.6～0.85s** 压到接近 `getUserMedia` 的物理下限。

---

## 2. 现象与复现

**复现步骤**

1. 真流式模式（`recording_mode=streaming`，provider 为 `doubao_asr` 或 `qwen_asr`）
2. 按下热键**的同时**就开始说第一个词，例如「**今天**下午三点开会」
3. 观察流式结果：开头往往只剩「下午三点开会」，「今天」丢了

**判据**：丢字量随"按下到开口"的间隔缩小而变大；间隔 >1s 时基本不丢 → 典型的启动窗口丢帧。

---

## 3. 排查结论（已确证）

### 3.1 实测延迟（来自真实运行日志）

热键按下（`logs/electron.log` 的 `Desktop agent hotkey press`）到后端两个标记事件（`logs/app.log`）：

| 阶段 | 相对按下时刻 | 样本数 |
|---|---|---|
| 后端开始连豆包（`连接豆包双向流式 ASR`） | **+320～465ms** | 8 次 |
| 会话建立、后端回 READY（`创建流式录音会话`） | **+508～680ms** | 8 次 |

渲染层在 READY 之后才 `new AudioContext` + `await audioWorklet.addModule` + 连图，再叠加约 **+50～150ms**。

⇒ **按下后约 0.6～0.85s 内，没有任何音频采样进入管线。**

### 3.2 根因：不是"录得慢"，是这段音频**根本没被采集**

`src/composables/useRecorder.js` 的启动顺序（基线第 1287-1288 行）：

```js
await connectStreamingSocket(context.settingsStore.settings.asr_language || 'zh') // 等后端 READY
await setupStreamingAudioCapture(stream.value)                                    // 之后才挂 worklet
```

而麦克风**在这之前就已经打开**了：`prepareRecordingEnvironment()`（第 540 行）在
第 569 行 `navigator.mediaDevices.getUserMedia(...)`，第 626 行 `stream.value = mediaStream`。

因此窗口期内的状态是：

- 麦克风已开、系统层在采集；
- 音频图里**没有任何消费者**（`streamingWorkletNode` 尚未创建/连接，见第 945-968 行）；
- WebAudio 是实时的，**不会为稍后才出现的消费者回放之前的采样**
  ⇒ 这段语音永久丢失，任何后端缓冲都救不回来。

### 3.3 关键发现：**排队机制已经存在，只是这段窗口没被它覆盖**

`sendStreamingSamples`（第 344-347 行）在会话未就绪时会**保留**样本：

```js
function sendStreamingSamples(force = false) {
  if (!streamingSocket || streamingSocket.readyState !== WebSocket.OPEN || !currentSessionId.value) {
    return   // 未就绪：样本继续留在 streamingPendingSamples 里
  }
  for (const chunkSize of pcm.planFlushChunkSizes(streamingPendingSampleCount, pcm.STREAM_FLUSH_SAMPLES)) { ... }
}
```

`queueStreamingSamples`（第 363-372 行）先把样本 append 进 `streamingPendingSamples` 再调用它。
采集粒度是每个 render quantum（`src/worklets/pcm-capture-worklet.js` 每 128 采样 post 一次 ≈ 8ms），
所以 READY 之后的**第一个** worklet 回调就会把积压整体冲刷（每次冲刷所有完整 3200 采样块）。

⇒ 只要把"挂采集"提前到与连接**并行**，积压就会被自动接住，不需要新增缓冲逻辑。

---

## 4. 推荐修复（最小改动）

### 4.1 主改动：采集与连接并行

`startStreamingRecording()` 中把第 1287-1288 行的两次串行 await 改成并行：

```js
// 采集与连接并行：麦克风在 prepareRecordingEnvironment 里已经打开，
// 这里必须尽早挂上 worklet，否则连接期间（实测 0.5~0.7s）的语音会永久丢失。
// 期间采集到的样本先停留在 streamingPendingSamples，READY 后由 sendStreamingSamples 统一冲刷。
await Promise.all([
  connectStreamingSocket(context.settingsStore.settings.asr_language || 'zh'),
  setupStreamingAudioCapture(stream.value)
])
```

要点与理由：

- 两个函数在构造数组时即被调用，因此**同时开始**；`Promise.all` 保证任一失败都被外层
  `try/catch`（第 1326 行）收口，不会出现未处理的 rejection。
- 协议顺序不受影响：音频只在 `currentSessionId.value` 就绪后才真正 `send()`（见 3.3 的守卫），
  豆包要求的「先 full client request 再推音频」仍然成立。
- 预期收益：丢失窗口 **0.6～0.85s → ~0.2～0.4s**（剩余部分是 `getUserMedia` 打开麦克风的物理耗时）。

### 4.2 可选：给积压队列加上界（防御性）

现有 `STREAM_CONNECT_TIMEOUT_MS = 8000`（第 20 行）已经把最坏情况限死在 8s ≈ 256KB，
所以**不是必须**。若要做，建议在 `queueStreamingSamples` 里按最老优先丢弃并打点：

```js
const MAX_PENDING_SAMPLES = STREAM_SAMPLE_RATE * 8   // 8s 上界
```

### 4.3 顺手做掉（强烈建议）：把这个量指标变成可观测的

`logRecordingTiming`（第 144-147 行）目前走 `console.log`，**渲染进程 console 不会落进任何日志文件**，
这次排查只能靠跨进程日志反推，很不方便。建议：

- 改为走 `debugLog('renderer.recorder', ...)`（`logRecorderHotkey` 第 71 行就是范本，落 `logs/electron.log`）；
- 新增两个标记：`connect-ready`（`connectStreamingSocket` resolve 处，第 832-836 行）
  与 `capture-started`（`setupStreamingAudioCapture` 连图完成后，第 963 行附近）。

这样以后"开头丢字"可以直接用时间差量化（目标：`capture-started` 应早于 `connect-ready`）。

---

## 5. 千万不要碰坏的既有行为

同一区域近期刚修过三个真实缺陷（`bbc6647`、`4789020`、`9edcdb6`），改动时必须保持：

1. **短按竞争窗口收口**：`streamingStartAborted` / `pendingStopAfterStart` /
   `streamingAwaitingFinish` 三个标志的语义与判断位置（第 1270、1290-1314 行）**不要动**；
   采集提前后，放弃路径仍必须走到 `cleanupStreamingAudio()` + `resetStreamingState()`，
   否则会有"幽灵录音"（无人停止、一直推流、界面永久停在转写中）。
2. **异常收口**：`prepareRecordingEnvironment` 的 catch 里有专门的泄漏收口逻辑
   （第 573-593 行，注释说明它会永久静默拦截热键），改流程时不要绕过。
3. **空文本/无语音收尾**：`stream_finished` 且 `record=null` 的分支（第 855-871 行）必须保持
   完整复位（`setRecording(false)` / `setTranscribing(false)` / `tray.setIcon('idle')`）。

---

## 6. 验收标准

- [ ] 连说 10 次「今天下午三点开会」，首字「今」不再丢失（或丢字率显著下降）
- [ ] `capture-started` 时间点早于 `connect-ready`（若做了 4.3 埋点）
- [ ] 快速点按热键（<300ms 松手）仍走"录音时间过短"路径，不产生幽灵录音
- [ ] 连接失败（如断开豆包网络）时：界面错误提示、麦克风释放、无内存增长
- [ ] `uv run python -m pytest tests -q` 通过（存量 2 个 `test_asr_factory` 失败与本改动无关）
- [ ] `node scripts/run-node-tests.mjs` 通过（存量 1 个 `test_tray_icon_assets.js` 失败与本改动无关）

**建议补的单测**（纯逻辑抽到 `src/composables/recorder/*.mjs` 再测，符合项目约定）：

- `pcm.mjs` 现有 `appendSamples` / `takeSamplesFromQueue` / `planFlushChunkSizes` 已被
  `tests/test_recorder_pcm.mjs` 覆盖；若新增上界裁剪，请在该文件补用例。

---

## 7. 验证用的日志解析脚本（可直接复用）

```python
import json, re
from datetime import datetime

press = [m.group(1) for line in open('logs/electron.log', encoding='utf-8', errors='ignore')
         if (m := re.match(r'(\S+Z) \[main\] Desktop agent hotkey press', line))]

events = []
for f in ('logs/app.log.1', 'logs/app.log'):
    for line in open(f, encoding='utf-8', errors='ignore'):
        try: d = json.loads(line)
        except Exception: continue
        ev = d.get('event', '')
        if isinstance(ev, str) and ('连接豆包' in ev or '创建流式' in ev or '完成流式' in ev):
            events.append((d['timestamp'], ev, d))
events.sort()

parse = lambda t: datetime.fromisoformat(t.replace('Z', '+00:00'))
for p in press[-8:]:
    pt = parse(p)
    conn = next((e for e in events if '连接豆包' in e[1] and parse(e[0]) >= pt), None)
    sess = next((e for e in events if '创建流式' in e[1] and parse(e[0]) >= pt), None)
    if conn and sess:
        print(f"按下 {p[11:23]} → 连接 +{(parse(conn[0])-pt).total_seconds()*1000:5.0f}ms"
              f" → 会话 +{(parse(sess[0])-pt).total_seconds()*1000:5.0f}ms")
```

⚠️ **注意**：`logs/app.log` 由 `JSONRenderer` 写入，中文是 `\uXXXX` 转义，
**直接 grep 中文匹配不到**（本次排查踩过），必须 `json.loads` 后再判断。

---

## 8. 文件速查

| 位置 | 作用 |
|---|---|
| `src/composables/useRecorder.js:144-147` | `logRecordingTiming`（console.log，未落盘 → 建议改造） |
| `src/composables/useRecorder.js:344-372` | `sendStreamingSamples` / `queueStreamingSamples`（积压队列与就绪守卫） |
| `src/composables/useRecorder.js:540-626` | `prepareRecordingEnvironment`：开麦克风 `stream.value = mediaStream` |
| `src/composables/useRecorder.js:757-837` | `connectStreamingSocket`：resolve 条件是后端 `READY` |
| `src/composables/useRecorder.js:945-968` | `setupStreamingAudioCapture`：AudioContext + worklet 连图 |
| `src/composables/useRecorder.js:1258-1288` | `startStreamingRecording`：**本次要改的两次串行 await** |
| `src/worklets/pcm-capture-worklet.js` | 每 128 采样 post 一次（≈8ms） |
| `src/composables/recorder/pcm.mjs` | `STREAM_FLUSH_SAMPLES=3200`、append/take/planFlush |
| `app/services/streaming_session_manager.py` | 后端会话建立（日志 `创建流式录音会话`） |
| `app/stt/providers/doubao_asr_provider.py` | 豆包握手（日志 `连接豆包双向流式 ASR`） |

---

## 9. 可选进一步优化（按性价比排序，非本次必须）

| 方案 | 收益 | 代价 |
|---|---|---|
| 并行化（第 4.1 节） | 省 ~0.4s | 几乎为零 |
| 应用启动时预热 AudioContext + 预加载 worklet 模块 | 再省 ~50～150ms（首次按下冷启动） | 常驻一个空 AudioContext |
| 常驻"预热会话"（按下即用已连好的豆包会话） | 接近归零 | 豆包按时长计费；空闲连接需保活/重建 |
| 预热麦克风（保持一条静音流） | 省 ~0.2～0.4s | **麦克风指示灯常亮**，隐私代价，不建议默认开 |

---

## 10. 协作与项目规约提醒

- 该仓库**可能同时有另一个 agent 在改**（近期它提交了录音/热键相关修复与 pnpm 迁移配置）。
  动手前先 `git status`、`git log --oneline -5`，改完只显式暂存自己的文件（**禁止 `git add -A`**）。
- 注释与新增文档一律中文；提交信息用中文 Conventional Commits，一个提交一件完整的事。
- 改动涉及前端行为时，除了 pytest 还要跑 `node scripts/run-node-tests.mjs`
  （**不要**直接 `node --test`，`tests/` 里有用例会提前 `process.exit(0)` 截断汇总）。
- 若确认修复，建议同时更新：
  - `docs/已知问题.md`（速览表 + 已修复表各加一条）
  - `docs/架构说明.md` 若涉及录音启动流程描述

---

## 11. 未决问题（需要产品/你拍板）

1. 预滚进来的音频（按下后、连接就绪前）**是否计入录音时长**？
   现状 `startDurationClock()` 在 READY 之后才开始（第 1323 行），若把预滚计入，
   时长显示会与音频实际长度一致，但会影响"极短录音"判定（`duration < 1` 那条提示）。
2. 是否接受"应用启动预热 AudioContext"这种常驻资源？
3. 若并行化后仍觉得开头丢字明显（剩余 ~0.2～0.4s），是否考虑预热麦克风（隐私代价）？
