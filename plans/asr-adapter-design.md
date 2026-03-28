# ASR 适配器设计文档

## 1. 文档目标

本文档用于为当前 Electron 语音转文字项目设计一套可扩展的 ASR 适配器架构，使系统从“绑定单一模型实现”升级为“支持多种 ASR Provider 的统一接入层”。

当前重点兼容对象：

- 当前实现：`faster-whisper / Whisper`
- 潜在替换目标：`Qwen3-ASR-1.7B`

未来可能支持：

- `SenseVoice`
- `FunASR`
- `WeNet`
- 云端 ASR 服务

## 2. 背景

### 2.1 当前现状

当前项目的后端语音识别主链路，基本围绕单一 Whisper 实现展开：

- 录音上传后进入统一转写入口
- 转写实现集中在 [transcriber.py](E:/repo/att/app/stt/transcriber.py)
- 会话、多段录音、后台任务最终都直接依赖当前 transcriber

这套实现的优点是：

- 结构简单
- 易于维护
- 当前功能已经基本可用

但问题也很明显：

- 业务层直接绑定 Whisper
- 更换模型时侵入面会越来越大
- 无法优雅支持离线、本地、流式、云端等不同类型的 ASR
- 不同模型能力差异无法统一管理

### 2.2 为什么要做适配器

后续如果要尝试更强的中文 ASR 模型，例如 `Qwen3-ASR-1.7B`，直接在现有代码中硬塞一套新逻辑会造成以下问题：

- `routes.py`、session 逻辑、转写逻辑、配置逻辑耦合加重
- 前后端状态链路与模型细节混杂
- 后面再接入第三种模型时成本更高

因此，更合理的方向是新增一层统一 ASR Provider 适配层：

- 业务逻辑只关心“转写能力”
- 模型差异由 provider 内部处理
- 配置与模型切换统一管理

## 3. 需求

### 3.1 功能需求

系统需要支持：

1. 单段音频转写
2. 多段 session 片段转写
3. 未来支持流式 / 准流式转写扩展
4. 按配置切换不同 ASR Provider
5. 各 Provider 输出统一的数据结构

### 3.2 非功能需求

系统需要满足：

1. 不破坏现有单段录音链路
2. 不破坏现有多段 session 链路
3. Provider 切换时对业务层改动最小
4. 便于灰度和 A/B 测试
5. 可以对不同模型能力进行声明式探测

### 3.3 兼容性需求

适配器必须兼容不同模型能力差异，例如：

- 是否支持流式
- 是否支持词级时间戳
- 是否支持内建 VAD
- 是否要求本地 GPU
- 是否支持多语言 / 中文方言

## 4. 当前问题

当前代码层面存在以下结构性问题：

### 4.1 转写实现与业务强耦合

现有业务默认把 ASR 等同于 Whisper：

- 当前 transcriber 既承担“抽象能力”，又承担“Whisper 实现”
- 这会让业务层自然依赖 Whisper 特性

### 4.2 能力边界不明确

不同模型有明显差异：

- Whisper 偏文件式转写
- Qwen3-ASR 更适合高质量中文识别，也可能支持更复杂形态
- WeNet / FunASR 更偏实时流式

但当前系统还没有一层能力边界把它们统一起来。

### 4.3 配置体系无法表达多 Provider

当前设置主要表达的是：

- 模型名
- device
- compute type

而不是：

- provider 类型
- provider 独立参数
- provider 能力开关

## 5. 目标方案

### 5.1 设计原则

本方案遵循以下原则：

1. 先抽象接口，再接入新模型
2. 保持现有业务 API 不变
3. Provider 之间只通过统一结果结构沟通
4. 将模型差异收敛在适配层内部
5. 先支持离线文件转写，再考虑流式扩展

### 5.2 总体结构

建议在 `app/stt/` 下增加分层结构：

```text
app/stt/
  base.py
  factory.py
  models.py
  providers/
    whisper_provider.py
    qwen_asr_provider.py
    sensevoice_provider.py
```

说明：

- `base.py`：定义统一接口
- `factory.py`：根据配置生成 provider
- `models.py`：统一结果和能力结构
- `providers/*`：各模型具体实现

## 6. 接口设计

### 6.1 核心接口

建议定义统一抽象基类：

```python
class ASRProvider(ABC):
    @property
    @abstractmethod
    def name(self) -> str:
        pass

    @abstractmethod
    async def transcribe(
        self,
        audio_path: str | Path,
        language: str = "zh",
        **kwargs
    ) -> TranscriptionResult:
        pass

    async def warmup(self) -> None:
        pass

    async def close(self) -> None:
        pass

    def capabilities(self) -> ASRCapabilities:
        return ASRCapabilities()
```

### 6.2 能力声明

建议定义能力模型：

```python
@dataclass
class ASRCapabilities:
    streaming: bool = False
    word_timestamps: bool = False
    vad: bool = False
    diarization: bool = False
    offline_batch: bool = True
    multi_language: bool = True
```

这样前层调用逻辑就不用猜，而是可以显式判断：

- 当前 provider 能不能做 streaming
- 能不能做词级时间戳
- 能不能依赖 provider 自带 VAD

### 6.3 输出结构统一

无论底层模型是什么，统一都必须返回当前系统已经在使用的结构：

- `TranscriptionSegment`
- `TranscriptionResult`

这样可以最大化复用：

- `routes.py`
- `session_manager`
- 记录存储
- 前端状态展示

## 7. Provider 设计

### 7.1 Whisper Provider

文件：

- `app/stt/providers/whisper_provider.py`

职责：

- 承接现有 `WhisperTranscriber` 的能力
- 完整复用当前 faster-whisper 逻辑
- 作为默认 provider

说明：

这一层本质上是“把现在的实现搬进 provider”，而不是重写逻辑。

### 7.2 Qwen3-ASR Provider

文件：

- `app/stt/providers/qwen_asr_provider.py`

职责：

- 对接 `Qwen3-ASR-1.7B`
- 将其输出映射为统一 `TranscriptionResult`
- 屏蔽模型调用方式、tokenizer、processor 等实现细节

说明：

Qwen3-ASR 很可能与 Whisper 的调用方式不同，因此它更适合作为独立 provider，而不是塞进 `WhisperTranscriber` 内部。

### 7.3 其他 Provider

后续如果接入：

- `SenseVoice`
- `FunASR`
- `WeNet`

都应各自是独立 provider，不应污染已有 provider。

## 8. 工厂设计

### 8.1 Factory 入口

建议新增：

```python
def create_asr_provider(config: ASRConfig) -> ASRProvider:
    if config.provider == "whisper":
        return WhisperProvider(...)
    if config.provider == "qwen_asr":
        return QwenASRProvider(...)
    raise ValueError(...)
```

### 8.2 业务使用方式

当前初始化阶段不再直接创建 `WhisperTranscriber`，而是：

```python
asr_provider = create_asr_provider(asr_config)
```

然后业务层统一使用：

```python
result = await asr_provider.transcribe(audio_path)
```

这样：

- `routes.py` 不关心底层模型是谁
- 多段和单段共用统一入口
- 未来切模型时只改配置

## 9. 配置设计

### 9.1 新增配置项

建议增加：

```json
{
  "asr_provider": "whisper",
  "asr_model": "large-v3",
  "asr_device": "cuda"
}
```

对于不同 provider，再加 provider-specific 配置：

```json
{
  "whisper": {
    "model": "large-v3",
    "compute_type": "float16"
  },
  "qwen_asr": {
    "model": "Qwen3-ASR-1.7B",
    "device": "cuda",
    "max_audio_seconds": 30
  }
}
```

### 9.2 推荐方式

不要让所有 provider 共用一套参数字段。

正确方式是：

- 通用配置：`asr_provider`
- provider 独立配置：`whisper.*` / `qwen_asr.*`

这样不会出现：

- 某些字段只对某个 provider 生效
- 但 UI 和配置层无法解释的混乱情况

## 10. 与现有代码的集成方式

### 10.1 需要改动的核心文件

主要涉及：

- [transcriber.py](E:/repo/att/app/stt/transcriber.py)
- [routes.py](E:/repo/att/app/api/routes.py)
- `main.py` 或应用初始化入口
- `config.py`
- 设置页前后端读写逻辑

### 10.2 建议的迁移方式

第一步：

- 保留 [transcriber.py](E:/repo/att/app/stt/transcriber.py)
- 将其内部逻辑迁入 `WhisperProvider`

第二步：

- 把当前所有对 `WhisperTranscriber` 的直接依赖改成 `ASRProvider`

第三步：

- 增加 `QwenASRProvider`

第四步：

- 设置页增加 provider 切换能力

### 10.3 前端改动范围

前端基本不需要感知具体模型，只需要在设置页增加：

- 当前 ASR provider
- 可能的 provider-specific 参数

录音逻辑、悬浮窗、主界面状态链路都不应直接依赖某个模型名。

## 11. 尝试方案对比

### 方案 A：继续在当前 transcriber 中堆 if/else

做法：

- 在现有 [transcriber.py](E:/repo/att/app/stt/transcriber.py) 内部，根据配置判断调用 Whisper 或 Qwen3-ASR

优点：

- 初期改动少

缺点：

- 结构会迅速恶化
- 多模型调用逻辑混在一起
- 难以测试
- 难以后续扩展第三、第四种模型

结论：

不推荐。

### 方案 B：抽象 Provider 适配层

做法：

- 抽象统一 ASR 接口
- 每个模型独立 provider
- 用 factory 管理实例创建

优点：

- 清晰
- 可扩展
- 易测试
- 迁移成本可控

缺点：

- 初期需要做一次结构整理

结论：

推荐。

### 方案 C：服务化拆分成外部 ASR 微服务

做法：

- 当前主应用不直接加载模型
- 而是通过 HTTP / WebSocket 调用外部 ASR 服务

优点：

- 模型替换完全独立
- 可扩展性最高

缺点：

- 架构改动较大
- 本地部署复杂度明显上升
- 当前项目阶段成本偏高

结论：

暂不推荐作为第一阶段方案。

## 12. 技术细节建议

### 12.1 错误处理

适配层要统一异常模型，例如：

- 模型加载失败
- 音频格式不支持
- 推理超时
- Provider 配置错误

建议定义统一异常类型，例如：

- `ASRProviderError`
- `ASRConfigError`
- `ASRCapabilityError`

### 12.2 日志

每个 provider 应独立打印：

- provider 名称
- model 名称
- device
- 音频时长
- 推理耗时

便于对比同一段音频在不同 provider 下的表现。

### 12.3 测试

建议分三层测试：

1. Provider 单元测试
   - 输入固定音频
   - 验证返回结构完整

2. Factory 测试
   - 不同配置能否正确生成 provider

3. 集成测试
   - 单段上传
   - 多段 session
   - 后台任务转写

### 12.4 流式扩展预留

虽然当前第一阶段主要是文件转写适配，但接口应预留：

```python
class StreamingASRProvider(ASRProvider):
    async def create_stream(self): ...
    async def push_audio(self, chunk): ...
    async def finalize_stream(self): ...
```

这样后续如果你要支持类似微信语音那种低延迟实时流式，就不需要重新推翻整个适配层。

## 13. 实施建议

### 阶段 1：抽象但不换模型

目标：

- 先把当前 Whisper 迁入适配器架构

工作：

- 新增 `ASRProvider`
- 新增 `WhisperProvider`
- 新增 `factory`
- 业务层改为依赖接口

结果：

- 对用户无感知
- 只是完成技术债整理

### 阶段 2：接入 Qwen3-ASR-1.7B

目标：

- 在不影响当前业务流程的前提下接入第二个 provider

工作：

- 实现 `QwenASRProvider`
- 增加 provider 配置
- 做最小范围切换测试

结果：

- 可在设置中切换 Whisper / Qwen3-ASR

### 阶段 3：加入能力探测与灰度

目标：

- 支持按场景选择 provider

例如：

- 单段模式用 Qwen3-ASR
- 多段模式先保留 Whisper
- 或部分用户灰度启用新 provider

### 阶段 4：考虑流式 Provider

目标：

- 为未来微信式实时流式识别做准备

可扩展方向：

- FunASR
- WeNet
- 云端流式 ASR

## 14. 风险与注意事项

### 14.1 不同模型输出风格不同

即便接口统一，不同模型输出在这些方面仍可能不同：

- 标点风格
- 分段颗粒度
- 方言支持
- 专有名词稳定性

因此，适配器只能统一“结构”，不能统一“模型风格”。

### 14.2 Qwen3-ASR 可能更重

与 Whisper 相比，Qwen3-ASR 的部署要求、显存占用、依赖链可能更复杂。

所以适配器设计要支持：

- provider 独立配置
- provider 独立 warmup
- provider 独立降级策略

### 14.3 不要让前端知道底层模型细节

这是很重要的边界：

- 前端只知道“当前 ASR provider”
- 不要让录音页面、悬浮窗、session 逻辑直接分支判断 `whisper` / `qwen`

否则适配器层的价值会被破坏。

## 15. 最终建议

如果目标是：

- 当前继续使用 faster-whisper
- 后续有可能切换到 Qwen3-ASR-1.7B
- 未来还可能尝试流式模型

那么最合理的路线不是“继续在当前 transcriber 中加分支”，而是：

`先做统一 ASR 适配器层，再在适配器层下接入不同 Provider`

这是当前项目最稳、最可扩展、对现有代码破坏最小的方案。

## 16. 推荐下一步

建议下一步直接进入实施阶段 1：

1. 抽象 `ASRProvider`
2. 把当前 Whisper 实现迁入 `WhisperProvider`
3. 增加 `factory`
4. 把业务调用改成依赖 provider 接口

等这一步完成后，再接入 `Qwen3-ASR-1.7B`，成本会明显更低。
## 17. 补充说明：Qwen 流式与 Windows / vllm-windows

针对 `Qwen3-ASR-1.7B` 的流式模式，需要额外补充一个与当前设计强相关的现实约束：

1. Qwen 官方当前将 streaming inference 绑定在 `vLLM backend`
2. 因此，Qwen 的真流式能力不应与当前文件式 `ASRProvider.transcribe(...)` 视为同一种运行形态
3. Windows 上如果使用 `SystemPanic/vllm-windows` 提供的 `vllm` 运行时，这条路线在架构上是成立的

这意味着：

- `QwenASRProvider` 可以继续承担文件式批量转写
- 真流式应继续走独立的 `StreamingASRProvider`
- 对 Windows，不应再假设“只能 Linux / WSL2 才能跑 vLLM”

更具体地说，Windows 下推荐优先采用：

- 主应用保留当前 FastAPI / Electron 架构
- Qwen 真流式服务独立部署
- 主应用通过 `remote_bridge` 方式桥接该服务
- 独立服务使用 `uv` 管理 Python 3.12 环境，并固定安装 GPU 版 `torch` 与 `vllm-windows 0.17.0`

如果后续验证 `vllm-windows` + `qwen-asr[vllm]` 在主应用同一 Python 环境内足够稳定，再考虑让 `local_vllm` 成为可选部署形态。

换句话说，针对 Windows 平台，当前设计的重点不是修改适配器分层，而是：

- 保持批处理 provider 与流式 provider 的边界
- 允许流式 backend 独立部署
- 将 `vllm-windows` 视为 Windows 平台的 vLLM 运行时实现，而不是视为另一套完全不同的架构
