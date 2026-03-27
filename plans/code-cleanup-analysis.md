# 代码清理实施计划

## 背景

项目经过多次重构后存在一些未使用的代码，需要清理以提高代码质量和可维护性。本计划将删除确认未使用的代码，同时保留用户需要的剪贴板模块。

---

## 清理范围

### 保留的模块
- **剪贴板模块 (`app/clipboard/`)** - 用户自己实现的部分代码，需要保留

### 删除的内容

#### 1. 前端未使用文件
- `static/js/multi_segment_recorder.js` - 多段录音器（未完成集成）
- `static/js/silence_detector.js` - 静音检测器（仅被 multi_segment_recorder 使用）

#### 2. 前端 HTML 修改
- `static/index.html` - 移除对上述 JS 文件的引用
- `static/index.html` - 移除"多段录音"选项（或标记为"即将推出"）

#### 3. 后端未使用模块
- `app/prompts/manager.py` - 重复实现，实际使用的是 `file_manager.py`

#### 4. 后端未使用的类和枚举
- `app/services/websocket.py` - 删除 `WSMessage` 类（第 44-56 行）
- `app/stt/transcriber.py` - 删除 `ModelSize` 枚举（第 22-29 行）

#### 5. 后端未使用的方法
- `app/stt/transcriber.py`:
  - `transcribe_chunk()` 方法（第 222-247 行）
  - `transcribe_chunks()` 方法（第 249-290 行）
  - `_merge_overlapping_segments()` 方法（第 292-326 行）

- `app/audio/processor.py`:
  - `chunk_audio()` 方法（第 311-387 行）
  - `save_permanent()` 方法（第 389-412 行）
  - `cleanup_temp_files()` 方法（第 523-534 行）

- `app/records/manager.py`:
  - `get_audio_file_path()` 方法（第 199-201 行）
  - `get_audio_url()` 方法（第 203-205 行）

---

## 实施步骤

### 步骤 1：删除前端未使用文件
```
删除文件: static/js/multi_segment_recorder.js
删除文件: static/js/silence_detector.js
```

### 步骤 2：修改 index.html
1. 移除以下 script 标签（约第 467-468 行）:
```html
<script src="/static/js/silence_detector.js"></script>
<script src="/static/js/multi_segment_recorder.js"></script>
```

2. 修改"录音模式"选项（约第 175-178 行），移除多段录音选项:
```html
<!-- 原代码 -->
<select class="setting-select" id="settingRecordingMode">
    <option value="single">单段录音</option>
    <option value="multi">多段录音（实时转写）</option>
</select>

<!-- 改为 -->
<select class="setting-select" id="settingRecordingMode">
    <option value="single">单段录音</option>
</select>
```

3. 在 `settings.js` 中移除对 `silenceThreshold` 和 `silenceThresholdSetting` 的引用（如果有的话）

### 步骤 3：删除后端未使用模块
```
删除文件: app/prompts/manager.py
```

### 步骤 4：清理 websocket.py
删除 `WSMessage` 类定义（第 44-56 行）:
```python
# 删除以下代码
class WSMessage(BaseModel):
    """WebSocket message structure."""
    type: MessageType
    data: Optional[Dict[str, Any]] = None
    timestamp: datetime = Field(default_factory=datetime.now)

    def to_dict(self) -> Dict[str, Any]:
        """Convert to dictionary for JSON serialization."""
        return {
            "type": self.type.value,
            "data": self.data,
            "timestamp": self.timestamp.isoformat()
        }
```

### 步骤 5：清理 transcriber.py
1. 删除 `ModelSize` 枚举（第 22-29 行）:
```python
# 删除以下代码
class ModelSize(str, Enum):
    """Whisper model sizes"""
    TINY = "tiny"
    BASE = "base"
    SMALL = "small"
    MEDIUM = "medium"
    LARGE_V2 = "large-v2"
    LARGE_V3 = "large-v3"
```

2. 删除 `transcribe_chunk()` 方法（第 222-247 行）
3. 删除 `transcribe_chunks()` 方法（第 249-290 行）
4. 删除 `_merge_overlapping_segments()` 方法（第 292-326 行）

### 步骤 6：清理 processor.py
1. 删除 `chunk_audio()` 方法（第 311-387 行）
2. 删除 `save_permanent()` 方法（第 389-412 行）
3. 删除 `cleanup_temp_files()` 方法（第 523-534 行）

### 步骤 7：清理 manager.py
1. 删除 `get_audio_file_path()` 方法（第 199-201 行）
2. 删除 `get_audio_url()` 方法（第 203-205 行）

---

## 验证步骤

### 1. 运行测试
```bash
python tests/test_basic.py
```

### 2. 启动服务验证
```bash
uvicorn app.main:app --reload
```

### 3. 功能验证清单
- [ ] 首页加载正常
- [ ] 录音功能正常（单段录音）
- [ ] 转写功能正常
- [ ] 文本优化功能正常
- [ ] 设置页面加载正常
- [ ] LLM 配置管理正常
- [ ] Prompt 预设管理正常
- [ ] WebSocket 连接正常

---

## 文件修改清单

| 操作 | 文件路径 |
|------|----------|
| 删除 | `static/js/multi_segment_recorder.js` |
| 删除 | `static/js/silence_detector.js` |
| 删除 | `app/prompts/manager.py` |
| 修改 | `static/index.html` |
| 修改 | `static/js/settings.js` |
| 修改 | `app/services/websocket.py` |
| 修改 | `app/stt/transcriber.py` |
| 修改 | `app/audio/processor.py` |
| 修改 | `app/records/manager.py` |

---

## 风险评估

| 风险 | 等级 | 缓解措施 |
|------|------|----------|
| 删除代码后功能异常 | 低 | 已确认这些代码无调用，运行测试验证 |
| 遗漏依赖关系 | 低 | 全局搜索确认后再删除 |
| 设置页面选项移除影响 | 低 | 移除的是未实现的功能选项 |
