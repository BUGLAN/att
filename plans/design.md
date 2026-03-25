# AudioToolsTester 设计文档

## 1. 整体架构

### 1.1 系统架构图

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                              客户端层                                        │
├─────────────────┬─────────────────┬─────────────────┬───────────────────────┤
│   Web Frontend  │  Electron App   │   Mobile App    │   第三方集成          │
│   (现有,增强)   │   (新增)        │   (新增)        │   (API开放)           │
└────────┬────────┴────────┬────────┴────────┬────────┴───────────────────────┘
         │                 │                 │
         └────────────────┬┴─────────────────┘
                          │ WebSocket / REST API
┌─────────────────────────┴───────────────────────────────────────────────────┐
│                              服务端层                                        │
├─────────────────────────────────────────────────────────────────────────────┤
│  FastAPI Backend                                                            │
│  ┌──────────────┐  ┌──────────────┐  ┌──────────────┐  ┌──────────────┐    │
│  │ API Routes   │  │ WebSocket    │  │ Audio Proc   │  │ LLM Gateway  │    │
│  │ (REST)       │  │ Manager      │  │ (Whisper)    │  │ (多服务商)   │    │
│  └──────────────┘  └──────────────┘  └──────────────┘  └──────────────┘    │
└─────────────────────────────────────────────────────────────────────────────┘
                          │
┌─────────────────────────┴───────────────────────────────────────────────────┐
│                              数据层                                          │
├─────────────────────────────────────────────────────────────────────────────┤
│  ┌──────────────┐  ┌──────────────┐  ┌──────────────┐  ┌──────────────┐    │
│  │ Records      │  │ Settings     │  │ Prompt       │  │ LLM Configs  │    │
│  │ (JSON)       │  │ (JSON)       │  │ Presets      │  │ (加密JSON)   │    │
│  └──────────────┘  └──────────────┘  └──────────────┘  └──────────────┘    │
└─────────────────────────────────────────────────────────────────────────────┘
```

### 1.2 技术栈

| 层级 | 现有技术 | 新增技术 |
|------|----------|----------|
| Web前端 | Vanilla JS, HTML/CSS | 增强WebSocket, 新增录音模式UI |
| 桌面端 | PySide6 (将被替换) | Electron + React/Vue |
| 移动端 | 无 | React Native 或 Flutter |
| 后端 | FastAPI, faster-whisper | 新增LLM Gateway层 |
| 数据存储 | JSON文件 | JSON文件 + 可选SQLite |

---

## 2. 模块设计

### 2.1 AI提示词预设模块

#### 2.1.1 数据模型

```python
# app/prompts/models.py

from pydantic import BaseModel
from typing import Optional, List
from enum import Enum
from datetime import datetime

class PromptCategory(str, Enum):
    GENERAL = "general"        # 通用
    CODING = "coding"          # 程序员
    NOVEL = "novel"           # 小说
    MEETING = "meeting"        # 会议
    MARKETING = "marketing"    # 文案
    CUSTOM = "custom"          # 自定义

class PromptPreset(BaseModel):
    id: str                           # 预设ID
    name: str                         # 预设名称
    description: Optional[str]        # 描述
    category: PromptCategory          # 分类
    prompt_template: str              # 提示词模板，含 {text} 占位符
    is_builtin: bool = False          # 是否内置预设
    created_at: datetime
    updated_at: Optional[datetime]

class PromptPresetCreate(BaseModel):
    name: str
    description: Optional[str] = None
    category: PromptCategory = PromptCategory.CUSTOM
    prompt_template: str

class PromptPresetUpdate(BaseModel):
    name: Optional[str] = None
    description: Optional[str] = None
    category: Optional[PromptCategory] = None
    prompt_template: Optional[str] = None
```

#### 2.1.2 存储结构

```
data/
├── prompts/
│   ├── presets.json          # 预设列表
│   └── presets/              # 每个预设单独文件（可选，便于导入导出）
│       ├── general.json
│       ├── coding.json
│       └── ...
```

**presets.json 结构：**
```json
{
  "presets": [
    {
      "id": "builtin-general",
      "name": "通用优化",
      "description": "适用于日常语音转文字的通用优化",
      "category": "general",
      "prompt_template": "你是一位语音转文字大师...",
      "is_builtin": true,
      "created_at": "2024-01-01T00:00:00Z"
    },
    {
      "id": "builtin-coding",
      "name": "程序员模式",
      "description": "针对程序员场景，优化代码术语和英文表达",
      "category": "coding",
      "prompt_template": "你是程序员助手...",
      "is_builtin": true,
      "created_at": "2024-01-01T00:00:00Z"
    }
  ],
  "default_preset_id": "builtin-general",
  "last_used_preset_id": "builtin-coding"
}
```

#### 2.1.3 API设计

```
GET    /api/prompts              # 获取所有预设列表
GET    /api/prompts/{id}         # 获取单个预设详情
POST   /api/prompts              # 创建自定义预设
PUT    /api/prompts/{id}         # 更新预设（仅自定义预设）
DELETE /api/prompts/{id}         # 删除预设（仅自定义预设）
POST   /api/prompts/{id}/preview # 预览预设效果（传入测试文本）
POST   /api/prompts/import       # 导入预设（JSON）
GET    /api/prompts/{id}/export  # 导出预设
PUT    /api/prompts/default      # 设置默认预设
```

#### 2.1.4 内置预设模板

```python
# app/prompts/builtin_presets.py

BUILTIN_PRESETS = {
    "general": """你是一位语音转文字专家，请优化以下语音转写文本。

要求：
1. 修正语音识别错误
2. 添加正确的标点符号
3. 去除口语化表达和重复内容
4. 保持原意，使表达更加通顺

原文：
{text}

优化后的文字：""",

    "coding": """你是程序员助手，请优化以下语音转写的技术内容。

要求：
1. 准确保留技术术语和英文单词
2. 修正代码相关的识别错误
3. 保持技术表达的准确性
4. 适当添加代码格式化标记

原文：
{text}

优化后的文字：""",

    "novel": """你是文学编辑，请优化以下语音转写的小说内容。

要求：
1. 保持人物对话的风格一致性
2. 统一人名和地名
3. 增强场景描写的文学性
4. 保留作者的写作风格

原文：
{text}

优化后的文字：""",

    "meeting": """你是会议记录助手，请整理以下语音转写的会议内容。

要求：
1. 提取关键讨论点和决议
2. 识别不同发言者（如有）
3. 整理成结构化的会议纪要
4. 突出行动项和待办事项

原文：
{text}

整理后的会议纪要：""",

    "marketing": """你是营销文案专家，请优化以下语音转写的营销内容。

要求：
1. 增强文案的吸引力和说服力
2. 突出产品/服务的核心卖点
3. 保持品牌调性一致
4. 优化语言表达，更具感染力

原文：
{text}

优化后的文案："""
}
```

---

### 2.2 多段录音模式模块

#### 2.2.1 架构设计

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                              前端录音控制器                                  │
├─────────────────────────────────────────────────────────────────────────────┤
│  ┌──────────────┐  ┌──────────────┐  ┌──────────────┐  ┌──────────────┐    │
│  │ AudioRecorder│  │ SilenceDetect│  │ SegmentQueue │  │ StreamDisplay│    │
│  │ (录音管理)   │  │ (静音检测)   │  │ (片段队列)   │  │ (流式展示)   │    │
│  └──────────────┘  └──────────────┘  └──────────────┘  └──────────────┘    │
└─────────────────────────────────────────────────────────────────────────────┘
         │                    │                    │
         │ 音频流              │ 分割信号           │ 片段上传
         ▼                    ▼                    ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│                              后端处理流水线                                  │
├─────────────────────────────────────────────────────────────────────────────┤
│  ┌──────────────┐  ┌──────────────┐  ┌──────────────┐  ┌──────────────┐    │
│  │ Upload API   │  │ Transcribe   │  │ WebSocket    │  │ Record Merge │    │
│  │ (片段上传)   │  │ Worker       │  │ Push         │  │ (片段合并)   │    │
│  └──────────────┘  └──────────────┘  └──────────────┘  └──────────────┘    │
└─────────────────────────────────────────────────────────────────────────────┘
```

#### 2.2.2 数据模型

```python
# app/records/models.py (扩展)

from enum import Enum
from typing import List, Optional
from datetime import datetime

class RecordingMode(str, Enum):
    SINGLE = "single"      # 单段录音
    MULTI = "multi"        # 多段录音

class AudioSegment(BaseModel):
    """录音片段"""
    id: str
    session_id: str              # 所属会话ID
    sequence: int                # 片段序号
    audio_path: str              # 音频文件路径
    duration: float              # 时长（秒）
    start_time: datetime         # 开始时间
    end_time: datetime           # 结束时间
    status: RecordStatus         # 状态
    raw_text: Optional[str]      # 转写文本
    processing_time: Optional[float]

class RecordingSession(BaseModel):
    """录音会话（多段模式）"""
    id: str
    mode: RecordingMode = RecordingMode.MULTI
    status: str                  # recording, paused, completed
    segments: List[AudioSegment] = []
    total_duration: float = 0.0
    merged_text: Optional[str]   # 合并后的完整文本
    created_at: datetime
    completed_at: Optional[datetime]

# 扩展 UserSettings
class UserSettings(BaseModel):
    # ... 现有字段 ...
    recording_mode: RecordingMode = RecordingMode.SINGLE
    silence_threshold: float = 1.5    # 静音阈值（秒）
    silence_level: int = -40          # 静音音量阈值（dB）
```

#### 2.2.3 前端静音检测实现

```javascript
// static/js/silence_detector.js

class SilenceDetector {
    constructor(options = {}) {
        this.threshold = options.threshold || 1.5;      // 静音时长阈值（秒）
        this.silenceLevel = options.silenceLevel || -40; // 静音音量阈值（dB）
        this.onSegmentEnd = options.onSegmentEnd;        // 片段结束回调

        this.audioContext = null;
        this.analyser = null;
        this.isMonitoring = false;
        this.silenceStartTime = null;
    }

    async init(stream) {
        this.audioContext = new AudioContext();
        const source = this.audioContext.createMediaStreamSource(stream);
        this.analyser = this.audioContext.createAnalyser();
        this.analyser.fftSize = 2048;
        source.connect(this.analyser);
    }

    startMonitoring() {
        this.isMonitoring = true;
        this.silenceStartTime = null;
        this._checkAudio();
    }

    stopMonitoring() {
        this.isMonitoring = false;
    }

    _checkAudio() {
        if (!this.isMonitoring) return;

        const dataArray = new Uint8Array(this.analyser.frequencyBinCount);
        this.analyser.getByteFrequencyData(dataArray);

        // 计算平均音量
        const average = dataArray.reduce((a, b) => a + b) / dataArray.length;
        const dB = average > 0 ? 20 * Math.log10(average / 255) : -100;

        const now = Date.now();

        if (dB < this.silenceLevel) {
            // 静音状态
            if (!this.silenceStartTime) {
                this.silenceStartTime = now;
            } else if ((now - this.silenceStartTime) / 1000 >= this.threshold) {
                // 静音超过阈值，触发分割
                this.onSegmentEnd && this.onSegmentEnd();
                this.silenceStartTime = null;
            }
        } else {
            // 有声音，重置静音计时
            this.silenceStartTime = null;
        }

        requestAnimationFrame(() => this._checkAudio());
    }
}
```

#### 2.2.4 流式上传与展示

```javascript
// static/js/multi_segment_recorder.js

class MultiSegmentRecorder {
    constructor(options = {}) {
        this.sessionId = null;
        this.segments = [];
        this.currentSegmentIndex = 0;
        this.recorder = null;
        this.silenceDetector = new SilenceDetector({
            threshold: options.silenceThreshold || 1.5,
            onSegmentEnd: () => this._splitAndUpload()
        });

        this.wsClient = options.wsClient;  // WebSocket客户端
    }

    async start() {
        const stream = await navigator.mediaDevices.getUserMedia({ audio: true });
        await this.silenceDetector.init(stream);

        this.recorder = new MediaRecorder(stream, {
            mimeType: 'audio/webm;codecs=opus'
        });

        // 创建录音会话
        this.sessionId = await this._createSession();

        this.recorder.start();
        this.silenceDetector.startMonitoring();

        // 监听转录结果
        this.wsClient.on('transcription', (data) => {
            this._displayTranscription(data);
        });
    }

    async _splitAndUpload() {
        // 暂停当前录音，获取片段
        const chunk = await this._getCurrentChunk();

        // 上传片段
        const segmentId = await this._uploadSegment(chunk);

        // 继续录音
        this.currentSegmentIndex++;
    }

    async stop() {
        this.silenceDetector.stopMonitoring();
        this.recorder.stop();

        // 上传最后一个片段
        const lastChunk = await this._getCurrentChunk();
        if (lastChunk.size > 0) {
            await this._uploadSegment(lastChunk);
        }

        // 合并会话
        await this._mergeSession();
    }

    _displayTranscription(data) {
        const container = document.getElementById('transcription-display');
        const segmentDiv = document.createElement('div');
        segmentDiv.className = 'segment';
        segmentDiv.dataset.segmentId = data.segment_id;
        segmentDiv.innerHTML = `
            <span class="segment-time">${data.duration}s</span>
            <p class="segment-text">${data.text}</p>
        `;
        container.appendChild(segmentDiv);

        // 滚动到底部
        container.scrollTop = container.scrollHeight;
    }
}
```

#### 2.2.5 后端API扩展

```python
# app/api/routes.py (扩展)

@router.post("/sessions")
async def create_session():
    """创建多段录音会话"""
    session = await session_manager.create_session()
    return {"session_id": session.id}

@router.post("/sessions/{session_id}/segments")
async def upload_segment(
    session_id: str,
    file: UploadFile,
    background_tasks: BackgroundTasks
):
    """上传录音片段"""
    segment = await session_manager.add_segment(session_id, file)
    background_tasks.add_task(
        _transcribe_segment_background,
        segment.id
    )
    return {"segment_id": segment.id, "sequence": segment.sequence}

@router.post("/sessions/{session_id}/merge")
async def merge_session(session_id: str):
    """合并会话所有片段"""
    record = await session_manager.merge_session(session_id)
    return record

@router.websocket("/api/ws/{client_type}")
async def websocket_endpoint(websocket: WebSocket, client_type: str):
    # ... 现有代码 ...

    # 新增消息类型
    if message["type"] == "segment_transcribed":
        # 广播片段转录结果
        await manager.broadcast({
            "type": "transcription",
            "segment_id": message["segment_id"],
            "text": message["text"],
            "duration": message["duration"]
        })
```

---

### 2.3 WebSocket状态通知模块

#### 2.3.1 消息协议扩展

```python
# app/services/websocket.py (扩展)

# 新增消息类型
class MessageType(str, Enum):
    # 现有类型
    RECORDING = "recording"
    RECORDING_COMMAND = "recording_command"
    STATUS_UPDATE = "status_update"
    HOTKEY_UPDATE = "hotkey_update"
    HOTKEY_CHANGED = "hotkey_changed"
    WEB_RECORDING = "web_recording"
    SETTINGS_UPDATE = "settings_update"
    SETTINGS_CHANGED = "settings_changed"
    PING = "ping"
    PONG = "pong"

    # 新增类型
    TRANSCRIPTION_STARTED = "transcription_started"
    TRANSCRIPTION_PROGRESS = "transcription_progress"
    TRANSCRIPTION_COMPLETED = "transcription_completed"
    OPTIMIZATION_STARTED = "optimization_started"
    OPTIMIZATION_COMPLETED = "optimization_completed"
    ERROR = "error"
    SEGMENT_TRANSCRIBED = "segment_transcribed"

# 消息结构
class WSMessage(BaseModel):
    type: MessageType
    data: Optional[dict] = None
    timestamp: datetime = Field(default_factory=datetime.now)

# 示例消息
{
    "type": "transcription_progress",
    "data": {
        "record_id": "abc123",
        "progress": 45,           # 百分比
        "estimated_remaining": 10  # 预估剩余秒数
    },
    "timestamp": "2024-01-15T10:30:00Z"
}

{
    "type": "transcription_completed",
    "data": {
        "record_id": "abc123",
        "text": "转录完成的文本...",
        "duration": 30.5,
        "processing_time": 2.3
    },
    "timestamp": "2024-01-15T10:30:02Z"
}

{
    "type": "segment_transcribed",
    "data": {
        "session_id": "session123",
        "segment_id": "seg001",
        "sequence": 1,
        "text": "第一段转录文本...",
        "duration": 5.2
    },
    "timestamp": "2024-01-15T10:30:01Z"
}
```

#### 2.3.2 后端推送实现

```python
# app/api/routes.py (修改)

async def _transcribe_audio_background(
    record_id: str,
    audio_path: str,
    auto_optimize: bool,
    manager: ConnectionManager
):
    """后台转录任务（增加WebSocket通知）"""
    try:
        # 更新状态：开始转录
        await record_manager.update_status(record_id, RecordStatus.TRANSCRIBING)

        # WebSocket通知：转录开始
        await manager.broadcast({
            "type": "transcription_started",
            "data": {"record_id": record_id}
        })

        # 执行转录
        result = await transcriber.transcribe(audio_path)

        # WebSocket通知：转录完成
        await manager.broadcast({
            "type": "transcription_completed",
            "data": {
                "record_id": record_id,
                "text": result.text,
                "duration": result.duration,
                "processing_time": result.processing_time
            }
        })

        # 更新记录
        await record_manager.update_transcription(record_id, result)

        # 如果开启自动优化
        if auto_optimize:
            await record_manager.update_status(record_id, RecordStatus.OPTIMIZING)

            await manager.broadcast({
                "type": "optimization_started",
                "data": {"record_id": record_id}
            })

            optimized = await optimizer.optimize(result.text)

            await manager.broadcast({
                "type": "optimization_completed",
                "data": {
                    "record_id": record_id,
                    "text": optimized
                }
            })

            await record_manager.update_optimization(record_id, optimized)

    except Exception as e:
        await manager.broadcast({
            "type": "error",
            "data": {
                "record_id": record_id,
                "error": str(e)
            }
        })
        await record_manager.update_status(record_id, RecordStatus.FAILED)
```

#### 2.3.3 前端处理

```javascript
// static/js/websocket.js (扩展)

class WebSocketClient {
    constructor(url) {
        this.url = url;
        this.ws = null;
        this.handlers = {};
        this.fallbackToPolling = false;

        // 注册默认处理器
        this.on('transcription_started', this._handleTranscriptionStarted.bind(this));
        this.on('transcription_completed', this._handleTranscriptionCompleted.bind(this));
        this.on('optimization_started', this._handleOptimizationStarted.bind(this));
        this.on('optimization_completed', this._handleOptimizationCompleted.bind(this));
        this.on('error', this._handleError.bind(this));
    }

    connect() {
        try {
            this.ws = new WebSocket(this.url);

            this.ws.onmessage = (event) => {
                const message = JSON.parse(event.data);
                const handler = this.handlers[message.type];
                if (handler) {
                    handler(message.data);
                }
            };

            this.ws.onclose = () => {
                // 降级到轮询
                if (!this.fallbackToPolling) {
                    this.fallbackToPolling = true;
                    this._startPolling();
                }
                // 尝试重连
                this._reconnect();
            };

        } catch (error) {
            this.fallbackToPolling = true;
            this._startPolling();
        }
    }

    _startPolling() {
        // 轮询降级逻辑
        this.pollingInterval = setInterval(async () => {
            if (this.currentRecordId) {
                const status = await fetch(`/api/records/${this.currentRecordId}/status`);
                this._handleStatusUpdate(await status.json());
            }
        }, 2000);
    }

    _handleTranscriptionStarted(data) {
        this.app.updateRecordStatus(data.record_id, 'transcribing');
        this.app.showProgress(data.record_id, 0);
    }

    _handleTranscriptionCompleted(data) {
        this.app.updateRecordText(data.record_id, data.text);
        this.app.hideProgress(data.record_id);
    }

    // ... 其他处理器
}
```

---

### 2.4 多LLM服务商模块

#### 2.4.1 架构设计

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                              LLM Gateway                                     │
├─────────────────────────────────────────────────────────────────────────────┤
│  ┌──────────────────────────────────────────────────────────────────────┐  │
│  │                        LLMProviderFactory                            │  │
│  │  (根据配置创建对应Provider实例)                                      │  │
│  └──────────────────────────────────────────────────────────────────────┘  │
│                                    │                                        │
│         ┌──────────────────────────┼──────────────────────────┐            │
│         │                          │                          │            │
│  ┌──────┴──────┐  ┌──────┴──────┐  ┌──────┴──────┐  ┌──────┴──────┐      │
│  │ DeepSeek    │  │ Claude      │  │ OpenAI      │  │ Doubao      │      │
│  │ Provider    │  │ Provider    │  │ Provider    │  │ Provider    │      │
│  └─────────────┘  └─────────────┘  └─────────────┘  └─────────────┘      │
└─────────────────────────────────────────────────────────────────────────────┘
```

#### 2.4.2 数据模型

```python
# app/llm/models.py

from pydantic import BaseModel
from typing import Optional, Dict, Any
from enum import Enum
from datetime import datetime
import secrets

class LLMProvider(str, Enum):
    DEEPSEEK = "deepseek"
    CLAUDE = "claude"
    OPENAI = "openai"
    DOUBAO = "doubao"       # 豆包
    QWEN = "qwen"           # 通义千问
    CUSTOM = "custom"       # 自定义

class LLMConfig(BaseModel):
    """LLM服务商配置"""
    id: str
    provider: LLMProvider
    name: str                           # 用户定义的名称
    api_key_encrypted: str              # 加密后的API密钥
    api_base: Optional[str] = None      # 自定义API端点
    model: str                          # 模型名称
    temperature: float = 0.7
    max_tokens: int = 4096
    extra_params: Dict[str, Any] = {}   # 额外参数
    is_default: bool = False
    created_at: datetime
    updated_at: Optional[datetime]

class LLMConfigCreate(BaseModel):
    provider: LLMProvider
    name: str
    api_key: str
    api_base: Optional[str] = None
    model: str
    temperature: float = 0.7
    max_tokens: int = 4096
    extra_params: Dict[str, Any] = {}

class LLMConfigUpdate(BaseModel):
    name: Optional[str] = None
    api_key: Optional[str] = None
    api_base: Optional[str] = None
    model: Optional[str] = None
    temperature: Optional[float] = None
    max_tokens: Optional[int] = None
    extra_params: Optional[Dict[str, Any]] = None
```

#### 2.4.3 Provider接口

```python
# app/llm/base.py

from abc import ABC, abstractmethod
from typing import Optional

class BaseLLMProvider(ABC):
    """LLM Provider抽象基类"""

    def __init__(self, config: LLMConfig, decrypted_key: str):
        self.config = config
        self.api_key = decrypted_key

    @abstractmethod
    async def optimize(self, text: str, prompt_template: str) -> str:
        """优化文本"""
        pass

    @abstractmethod
    async def test_connection(self) -> bool:
        """测试连接"""
        pass

    @abstractmethod
    def get_available_models(self) -> list[str]:
        """获取可用模型列表"""
        pass

# app/llm/providers/deepseek.py

from openai import AsyncOpenAI

class DeepSeekProvider(BaseLLMProvider):
    def __init__(self, config: LLMConfig, decrypted_key: str):
        super().__init__(config, decrypted_key)
        self.client = AsyncOpenAI(
            api_key=self.api_key,
            base_url=config.api_base or "https://api.deepseek.com/v1"
        )

    async def optimize(self, text: str, prompt_template: str) -> str:
        prompt = prompt_template.format(text=text)

        response = await self.client.chat.completions.create(
            model=self.config.model,
            messages=[
                {"role": "user", "content": prompt}
            ],
            temperature=self.config.temperature,
            max_tokens=self.config.max_tokens
        )

        return response.choices[0].message.content

    async def test_connection(self) -> bool:
        try:
            await self.client.models.list()
            return True
        except:
            return False

    def get_available_models(self) -> list[str]:
        return ["deepseek-chat", "deepseek-coder"]

# app/llm/providers/claude.py

from anthropic import AsyncAnthropic

class ClaudeProvider(BaseLLMProvider):
    def __init__(self, config: LLMConfig, decrypted_key: str):
        super().__init__(config, decrypted_key)
        self.client = AsyncAnthropic(
            api_key=self.api_key,
            base_url=config.api_base
        )

    async def optimize(self, text: str, prompt_template: str) -> str:
        prompt = prompt_template.format(text=text)

        response = await self.client.messages.create(
            model=self.config.model,
            max_tokens=self.config.max_tokens,
            messages=[
                {"role": "user", "content": prompt}
            ]
        )

        return response.content[0].text

    async def test_connection(self) -> bool:
        try:
            await self.client.messages.create(
                model=self.config.model,
                max_tokens=10,
                messages=[{"role": "user", "content": "Hi"}]
            )
            return True
        except:
            return False

    def get_available_models(self) -> list[str]:
        return ["claude-3-5-sonnet-20241022", "claude-3-opus-20240229", "claude-3-haiku-20240307"]
```

#### 2.4.4 密钥加密存储

```python
# app/utils/encryption.py

from cryptography.fernet import Fernet
from cryptography.hazmat.primitives import hashes
from cryptography.hazmat.primitives.kdf.pbkdf2 import PBKDF2HMAC
import base64
import os

class APIKeyEncryption:
    """API密钥加密工具"""

    def __init__(self):
        # 从环境变量或生成密钥
        self._key = self._get_or_create_key()
        self._fernet = Fernet(self._key)

    def _get_or_create_key(self) -> bytes:
        """获取或创建加密密钥"""
        key_file = "data/.encryption_key"

        if os.path.exists(key_file):
            with open(key_file, "rb") as f:
                return f.read()
        else:
            key = Fernet.generate_key()
            os.makedirs("data", exist_ok=True)
            with open(key_file, "wb") as f:
                f.write(key)
            return key

    def encrypt(self, api_key: str) -> str:
        """加密API密钥"""
        return self._fernet.encrypt(api_key.encode()).decode()

    def decrypt(self, encrypted: str) -> str:
        """解密API密钥"""
        return self._fernet.decrypt(encrypted.encode()).decode()

# 使用
encryption = APIKeyEncryption()

# 存储
config.api_key_encrypted = encryption.encrypt(user_api_key)

# 使用
decrypted_key = encryption.decrypt(config.api_key_encrypted)
```

#### 2.4.5 API设计

```
GET    /api/llm/configs             # 获取所有LLM配置
POST   /api/llm/configs             # 创建LLM配置
PUT    /api/llm/configs/{id}        # 更新LLM配置
DELETE /api/llm/configs/{id}        # 删除LLM配置
PUT    /api/llm/configs/{id}/default # 设置默认配置
POST   /api/llm/configs/{id}/test   # 测试连接
GET    /api/llm/providers           # 获取支持的Provider列表
GET    /api/llm/providers/{provider}/models  # 获取Provider的可用模型
```

---

### 2.5 Electron桌面应用

#### 2.5.1 项目结构

```
electron-app/
├── package.json
├── electron-builder.yml           # 打包配置
├── src/
│   ├── main/                      # 主进程
│   │   ├── index.ts               # 入口
│   │   ├── tray.ts                # 托盘管理
│   │   ├── hotkey.ts              # 全局热键
│   │   ├── ipc.ts                 # IPC通信
│   │   └── audio.ts               # 音频控制
│   ├── renderer/                  # 渲染进程（前端）
│   │   ├── index.html
│   │   ├── App.tsx
│   │   └── components/
│   ├── preload/                   # 预加载脚本
│   │   └── index.ts
│   └── shared/                    # 共享代码
│       └── types.ts
└── resources/
    ├── icons/
    │   ├── icon.ico               # Windows
    │   ├── icon.icns              # macOS
    │   └── tray/                  # 托盘图标
    │       ├── idle.png
    │       ├── recording.png
    │       ├── transcribing.png
    │       └── disconnected.png
    └── installer/
```

#### 2.5.2 托盘实现

```typescript
// src/main/tray.ts

import { Tray, Menu, nativeImage, app, BrowserWindow } from 'electron';
import path from 'path';

export class TrayManager {
  private tray: Tray | null = null;
  private mainWindow: BrowserWindow | null = null;
  private status: 'idle' | 'recording' | 'transcribing' | 'disconnected' = 'idle';

  constructor(mainWindow: BrowserWindow) {
    this.mainWindow = mainWindow;
  }

  create() {
    const icon = this.getIcon('idle');
    this.tray = new Tray(icon);

    this.updateMenu();
    this.setupEvents();
  }

  private getIcon(status: string): nativeImage {
    const iconPath = path.join(
      app.getAppPath(),
      'resources/icons/tray',
      `${status}.png`
    );
    return nativeImage.createFromPath(iconPath).resize({ width: 22, height: 22 });
  }

  setStatus(status: 'idle' | 'recording' | 'transcribing' | 'disconnected') {
    this.status = status;
    if (this.tray) {
      this.tray.setImage(this.getIcon(status));
      this.updateMenu();
    }
  }

  private updateMenu() {
    if (!this.tray) return;

    const contextMenu = Menu.buildFromTemplate([
      {
        label: this.getStatusLabel(),
        enabled: false
      },
      { type: 'separator' },
      {
        label: '开始录音',
        click: () => this.startRecording()
      },
      { type: 'separator' },
      {
        label: '历史记录',
        submenu: this.getHistorySubmenu()
      },
      {
        label: '打开主窗口',
        click: () => this.showMainWindow()
      },
      { type: 'separator' },
      {
        label: '设置',
        click: () => this.openSettings()
      },
      { type: 'separator' },
      {
        label: '退出',
        click: () => app.quit()
      }
    ]);

    this.tray.setContextMenu(contextMenu);
  }

  private getHistorySubmenu(): MenuItemConstructorOptions[] {
    // 从本地存储获取最近5条记录
    return [
      { label: '加载中...', enabled: false }
    ];
  }

  private showMainWindow() {
    if (this.mainWindow) {
      this.mainWindow.show();
      this.mainWindow.focus();
    }
  }
}
```

#### 2.5.3 全局热键

```typescript
// src/main/hotkey.ts

import { globalShortcut, ipcMain } from 'electron';
import { uIOhook, UiohookKey } from 'uiohook-napi';

export class HotkeyManager {
  private currentHotkey: { modifiers: string[], key: string | null } | null = null;
  private isRecording = false;
  private onRecordingStart: () => void;
  private onRecordingStop: () => void;

  constructor(
    onRecordingStart: () => void,
    onRecordingStop: () => void
  ) {
    this.onRecordingStart = onRecordingStart;
    this.onRecordingStop = onRecordingStop;
    this.setupNativeHook();
  }

  private setupNativeHook() {
    // 使用uiohook支持按下和释放检测
    uIOhook.on('keydown', (e) => {
      if (this.matchesHotkey(e) && !this.isRecording) {
        this.isRecording = true;
        this.onRecordingStart();
      }
    });

    uIOhook.on('keyup', (e) => {
      if (this.matchesHotkey(e) && this.isRecording) {
        this.isRecording = false;
        this.onRecordingStop();
      }
    });

    uIOhook.start();
  }

  private matchesHotkey(e: any): boolean {
    if (!this.currentHotkey) return false;

    const pressedModifiers = [];
    if (e.altKey) pressedModifiers.push('alt');
    if (e.ctrlKey) pressedModifiers.push('ctrl');
    if (e.metaKey) pressedModifiers.push('cmd');
    if (e.shiftKey) pressedModifiers.push('shift');

    return JSON.stringify(pressedModifiers.sort()) ===
           JSON.stringify(this.currentHotkey.modifiers.sort());
  }

  setHotkey(hotkey: { modifiers: string[], key: string | null }) {
    this.currentHotkey = hotkey;
  }
}
```

#### 2.5.4 滑词翻译功能

```typescript
// src/main/selection-translate.ts

import { BrowserWindow, clipboard } from 'electron';

export class SelectionTranslate {
  private translateWindow: BrowserWindow | null = null;

  constructor() {
    this.createTranslateWindow();
    this.setupSelectionListener();
  }

  private createTranslateWindow() {
    this.translateWindow = new BrowserWindow({
      width: 300,
      height: 200,
      frame: false,
      transparent: true,
      alwaysOnTop: true,
      show: false,
      webPreferences: {
        preload: path.join(__dirname, 'preload.js')
      }
    });

    this.translateWindow.loadFile('translate-popup.html');
  }

  private setupSelectionListener() {
    // 监听选中文本事件
    ipcMain.on('selection-translate', async (event, text) => {
      const translation = await this.translate(text);
      this.showTranslation(translation);
    });
  }

  async showTranslation(translation: string) {
    // 在鼠标位置显示翻译窗口
    const { x, y } = screen.getCursorScreenPoint();

    this.translateWindow?.setPosition(x + 10, y + 10);
    this.translateWindow?.webContents.send('translation-result', translation);
    this.translateWindow?.show();
  }
}
```

---

### 2.6 移动端应用

#### 2.6.1 连接协议

```typescript
// 连接方式1: 扫描二维码
interface QRCodeData {
  host: string;          // 服务端地址
  port: number;          // 端口
  token: string;         // 认证令牌
  expires_at: string;    // 过期时间
}

// 连接方式2: 输入密钥
// 用户输入密钥后，客户端在局域网内广播发现请求
interface DiscoveryRequest {
  type: 'discovery';
  token: string;
}

interface DiscoveryResponse {
  host: string;
  port: number;
  server_name: string;
}
```

#### 2.6.2 API认证

```python
# app/api/auth.py

from fastapi import Header, HTTPException
from app.config import settings
import secrets

# 生成的访问令牌
ACCESS_TOKENS: dict[str, dict] = {}

def generate_access_token(expires_in: int = 3600) -> dict:
    """生成访问令牌"""
    token = secrets.token_urlsafe(32)
    ACCESS_TOKENS[token] = {
        "created_at": datetime.now(),
        "expires_at": datetime.now() + timedelta(seconds=expires_in)
    }
    return {
        "token": token,
        "expires_in": expires_in
    }

async def verify_token(authorization: str = Header(None)):
    """验证访问令牌"""
    if not authorization or not authorization.startswith("Bearer "):
        raise HTTPException(status_code=401, detail="Invalid authorization")

    token = authorization.replace("Bearer ", "")

    if token not in ACCESS_TOKENS:
        raise HTTPException(status_code=401, detail="Invalid token")

    token_info = ACCESS_TOKENS[token]
    if datetime.now() > token_info["expires_at"]:
        del ACCESS_TOKENS[token]
        raise HTTPException(status_code=401, detail="Token expired")

    return token
```

---

## 3. 迁移计划

### 3.1 第一阶段：核心功能增强

**目标：** 完成AI提示词自定义、多段录音模式、WebSocket状态通知

**文件变更：**

| 操作 | 文件 | 说明 |
|------|------|------|
| 新增 | `app/prompts/` | 提示词预设模块 |
| 新增 | `data/prompts/` | 预设数据存储 |
| 修改 | `app/llm/optimizer.py` | 支持预设选择 |
| 修改 | `app/api/routes.py` | 新增预设API、WebSocket推送 |
| 修改 | `app/services/websocket.py` | 扩展消息类型 |
| 修改 | `app/records/models.py` | 新增多段录音模型 |
| 修改 | `static/js/` | 前端录音模式、WebSocket处理 |
| 新增 | `static/js/multi_segment_recorder.js` | 多段录音控制器 |
| 新增 | `static/js/silence_detector.js` | 静音检测器 |

### 3.2 第二阶段：多LLM支持

**目标：** 支持多个LLM服务商配置

**文件变更：**

| 操作 | 文件 | 说明 |
|------|------|------|
| 新增 | `app/llm/base.py` | Provider抽象基类 |
| 新增 | `app/llm/models.py` | LLM配置模型 |
| 新增 | `app/llm/providers/` | 各Provider实现 |
| 新增 | `app/utils/encryption.py` | 密钥加密工具 |
| 修改 | `app/config.py` | 扩展配置结构 |
| 修改 | `static/js/settings.js` | LLM配置UI |

### 3.3 第三阶段：Electron应用

**目标：** 开发Electron桌面应用替代PySide6

**文件变更：**

| 操作 | 文件 | 说明 |
|------|------|------|
| 新增 | `electron-app/` | Electron项目 |
| 删除 | `tray/` | 移除PySide6托盘（完成后） |

---

## 4. 测试策略

### 4.1 单元测试

```python
# tests/test_prompts.py

def test_create_preset():
    """测试创建自定义预设"""
    pass

def test_builtin_presets_loaded():
    """测试内置预设加载"""
    pass

# tests/test_multi_segment.py

def test_silence_detection():
    """测试静音检测算法"""
    pass

def test_segment_upload():
    """测试片段上传"""
    pass

# tests/test_llm_gateway.py

def test_provider_factory():
    """测试Provider工厂"""
    pass

def test_api_key_encryption():
    """测试密钥加密"""
    pass
```

### 4.2 集成测试

```python
# tests/test_websocket_flow.py

async def test_transcription_notification():
    """测试转录完成WebSocket通知"""
    pass

async def test_multi_segment_session():
    """测试多段录音完整流程"""
    pass
```

---

## 5. 部署考量

### 5.1 环境变量

```bash
# .env.example (扩展)

# 现有
WHISPER_MODEL=large-v3
WHISPER_DEVICE=auto
DEEPSEEK_API_KEY=your-key

# 新增
ENCRYPTION_KEY_FILE=data/.encryption_key
LLM_CONFIGS_FILE=data/llm_configs.json
PROMPT_PRESETS_FILE=data/prompts/presets.json

# 移动端认证
MOBILE_AUTH_ENABLED=true
MOBILE_TOKEN_EXPIRE=86400

# Electron自动更新
ELECTRON_UPDATE_SERVER=https://updates.example.com
```

### 5.2 数据迁移

```python
# scripts/migrate_settings.py

def migrate_to_multi_llm():
    """迁移单一DeepSeek配置到多LLM配置"""
    # 读取现有.env中的DEEPSEEK_API_KEY
    # 创建默认LLM配置
    # 加密存储API密钥
    pass

def migrate_to_prompt_presets():
    """迁移硬编码提示词到预设系统"""
    pass
```
