# 第一阶段实现文档

## 概述

**完成日期**: 2026-03-24
**分支**: `feat/core-enhancement`
**状态**: ✅ 已完成

## 实现功能

### 1. AI提示词预设模块

**目标**: 支持多场景AI优化提示词管理

**实现文件**:
```
app/prompts/
├── __init__.py           # 模块导出
├── models.py             # 数据模型 (PromptCategory, PromptPreset)
├── builtin_presets.py    # 5个内置预设
└── manager.py            # 预设管理器 (CRUD操作)
```

**API端点**:
```
GET    /api/prompts              # 获取所有预设
GET    /api/prompts/{id}         # 获取单个预设
POST   /api/prompts              # 创建自定义预设
PUT    /api/prompts/{id}         # 更新预设
DELETE /api/prompts/{id}         # 删除预设
POST   /api/prompts/{id}/preview # 预览效果
PUT    /api/prompts/default      # 设置默认预设
```

**内置预设**:
| ID | 名称 | 分类 | 用途 |
|----|------|------|------|
| builtin-general | 通用优化 | general | 日常语音转文字 |
| builtin-coding | 程序员模式 | coding | 技术内容、代码术语 |
| builtin-novel | 小说创作 | novel | 文学创作、保持风格 |
| builtin-meeting | 会议记录 | meeting | 会议纪要、提取要点 |
| builtin-marketing | 文案优化 | marketing | 营销文案增强 |

**数据存储**: `data/prompts/presets.json`

---

### 2. 多段录音模式

**目标**: 实时流式转写，静音自动分割

**实现文件**:
```
后端:
app/records/models.py          # RecordingMode, AudioSegment, RecordingSession
app/records/session_manager.py # 会话管理器
app/api/routes.py              # 会话API端点

前端:
static/js/silence_detector.js       # 静音检测器
static/js/multi_segment_recorder.js # 多段录音控制器
```

**API端点**:
```
POST   /api/sessions              # 创建录音会话
GET    /api/sessions/{id}         # 获取会话详情
POST   /api/sessions/{id}/segments # 上传片段
POST   /api/sessions/{id}/merge   # 合并会话
DELETE /api/sessions/{id}         # 删除会话
```

**用户设置扩展**:
```json
{
  "recording_mode": "single",      // single | multi
  "silence_threshold": 1.5,        // 静音阈值(秒)
  "silence_level": -40             // 静音音量阈值(dB)
}
```

**工作流程**:
```
1. 用户选择多段录音模式
2. 开始录音 → 静音检测器监控音频
3. 静音超过阈值 → 自动分割，上传片段
4. 后端转录片段 → WebSocket推送结果
5. 前端实时展示转写文本
6. 停止录音 → 合并所有片段
```

---

### 3. WebSocket状态通知

**目标**: 替代轮询，实时推送转录状态

**实现文件**:
```
app/services/websocket.py      # 扩展MessageType枚举
static/js/websocket.js         # 消息处理器、轮询降级
```

**新增消息类型**:
```
transcription_started   - 转录开始
transcription_completed - 转录完成
optimization_started    - AI优化开始
optimization_completed  - AI优化完成
segment_transcribed     - 片段转录完成 (多段模式)
error                   - 错误通知
```

**消息格式**:
```json
{
  "type": "transcription_completed",
  "data": {
    "record_id": "abc123",
    "text": "转录文本...",
    "duration": 30.5,
    "processing_time": 2.3
  },
  "timestamp": "2026-03-24T14:00:00"
}
```

**轮询降级机制**:
- WebSocket断开 → 自动启动HTTP轮询
- 重连成功 → 停止轮询
- 轮询间隔: 2秒

---

### 4. LLM优化器增强

**目标**: 支持预设选择

**修改文件**: `app/llm/optimizer.py`

**API签名变更**:
```python
# 旧
async def optimize(text: str, style: str = "default")

# 新
async def optimize(text: str, style: str = "default", preset_id: str = None)
```

---

## 配置变更

**用户设置** (`data/settings.json`):
```json
{
  "whisper_model": "large-v3",
  "whisper_device": "auto",
  "pause_audio_on_recording": true,
  "auto_optimize": false,
  "hotkey": {...},
  "recording_mode": "single",
  "silence_threshold": 1.5,
  "silence_level": -40,
  "default_preset_id": "builtin-general",
  "last_used_preset_id": null
}
```

---

## 前端UI变更

**设置页面新增**:
- 录音模式选择 (单段/多段)
- 静音阈值配置 (多段模式显示)
- AI优化预设选择
- 预设管理按钮

**预设管理弹窗**:
- 预设列表展示
- 新建/编辑/删除预设
- 预设预览测试

**新增JS模块**:
```
static/js/silence_detector.js       - 静音检测
static/js/multi_segment_recorder.js - 多段录音
```

---

## 依赖关系

```
提示词预设模块 (独立)
    ↓
LLM优化器增强
    ↓
WebSocket通知 ← 多段录音模式
    ↓
前端UI更新
```

---

## 文件变更清单

| 操作 | 文件 | 说明 |
|------|------|------|
| 新增 | `app/prompts/` | 提示词预设模块 |
| 新增 | `app/records/session_manager.py` | 会话管理 |
| 新增 | `static/js/silence_detector.js` | 静音检测 |
| 新增 | `static/js/multi_segment_recorder.js` | 多段录音 |
| 修改 | `app/api/routes.py` | 新增API端点 |
| 修改 | `app/config.py` | 用户设置扩展 |
| 修改 | `app/llm/optimizer.py` | 预设支持 |
| 修改 | `app/records/models.py` | 多段录音模型 |
| 修改 | `app/services/websocket.py` | 消息类型扩展 |
| 修改 | `static/index.html` | UI元素 |
| 修改 | `static/js/settings.js` | 设置管理、预设管理 |
| 修改 | `static/js/websocket.js` | 消息处理、轮询降级 |
