# 第一阶段测试文档

## 测试环境准备

```bash
# 1. 切换到项目目录
cd /Users/lansheng/repo/att

# 2. 确保在正确的分支
git checkout feat/core-enhancement

# 3. 启动服务器
uvicorn app.main:app --reload --port 8000
```

---

## 单元测试

### 1. 提示词预设模块测试

```python
# tests/test_prompts.py

def test_builtin_presets_loaded():
    """测试内置预设加载"""
    from app.prompts import PromptManager
    pm = PromptManager()

    presets = pm.get_all()
    assert len(presets) >= 5  # 至少5个内置预设

    # 验证预设结构
    for preset in presets:
        assert preset.id
        assert preset.name
        assert preset.prompt_template
        assert '{text}' in preset.prompt_template


def test_create_custom_preset():
    """测试创建自定义预设"""
    from app.prompts import PromptManager, PromptPresetCreate, PromptCategory

    pm = PromptManager()

    create_data = PromptPresetCreate(
        name="测试预设",
        description="用于测试",
        category=PromptCategory.CUSTOM,
        prompt_template="测试模板: {text}"
    )

    preset = pm.create(create_data)
    assert preset.id.startswith("custom-")
    assert preset.name == "测试预设"

    # 清理
    pm.delete(preset.id)


def test_render_prompt():
    """测试提示词渲染"""
    from app.prompts import PromptManager

    pm = PromptManager()
    rendered = pm.render_prompt("builtin-general", "测试文本")

    assert "测试文本" in rendered
    assert rendered.startswith("你是一位语音转文字专家")


def test_default_preset():
    """测试默认预设"""
    from app.prompts import PromptManager

    pm = PromptManager()
    default = pm.get_default()

    assert default is not None
    assert default.id == "builtin-general"
```

### 2. WebSocket消息类型测试

```python
# tests/test_websocket.py

def test_message_types():
    """测试消息类型枚举"""
    from app.services.websocket import MessageType

    # 验证新增类型
    assert MessageType.TRANSCRIPTION_STARTED.value == "transcription_started"
    assert MessageType.TRANSCRIPTION_COMPLETED.value == "transcription_completed"
    assert MessageType.OPTIMIZATION_STARTED.value == "optimization_started"
    assert MessageType.OPTIMIZATION_COMPLETED.value == "optimization_completed"
    assert MessageType.SEGMENT_TRANSCRIBED.value == "segment_transcribed"
    assert MessageType.ERROR.value == "error"


def test_ws_message_model():
    """测试消息模型"""
    from app.services.websocket import WSMessage, MessageType
    from datetime import datetime

    msg = WSMessage(
        type=MessageType.TRANSCRIPTION_COMPLETED,
        data={"record_id": "test123", "text": "测试"}
    )

    d = msg.to_dict()
    assert d["type"] == "transcription_completed"
    assert d["data"]["record_id"] == "test123"
    assert "timestamp" in d
```

### 3. 多段录音模型测试

```python
# tests/test_session_models.py

def test_recording_mode_enum():
    """测试录音模式枚举"""
    from app.records.models import RecordingMode

    assert RecordingMode.SINGLE.value == "single"
    assert RecordingMode.MULTI.value == "multi"


def test_audio_segment_model():
    """测试音频片段模型"""
    from app.records.models import AudioSegment, RecordStatus
    from datetime import datetime

    segment = AudioSegment(
        id="seg123",
        session_id="sess456",
        sequence=0,
        audio_path="/tmp/test.wav",
        duration=5.0
    )

    d = segment.to_dict()
    assert d["id"] == "seg123"
    assert d["session_id"] == "sess456"

    # 从字典恢复
    restored = AudioSegment.from_dict(d)
    assert restored.id == segment.id


def test_recording_session_model():
    """测试录音会话模型"""
    from app.records.models import RecordingSession, RecordingMode

    session = RecordingSession(
        id="sess789",
        mode=RecordingMode.MULTI,
        status="recording"
    )

    d = session.to_dict()
    assert d["mode"] == "multi"
    assert d["status"] == "recording"
```

### 4. 用户设置测试

```python
# tests/test_settings.py

def test_new_settings_keys():
    """测试新增设置项"""
    from app.config import UserSettings
    from pathlib import Path
    import tempfile

    with tempfile.TemporaryDirectory() as tmpdir:
        us = UserSettings(Path(tmpdir) / "settings.json")

        # 验证默认值
        assert us.get("recording_mode") == "single"
        assert us.get("silence_threshold") == 1.5
        assert us.get("silence_level") == -40
        assert us.get("default_preset_id") == "builtin-general"
```

---

## API集成测试

### 1. 提示词API测试

```bash
# 获取所有预设
curl http://localhost:8000/api/prompts | jq

# 获取单个预设
curl http://localhost:8000/api/prompts/builtin-general | jq

# 创建自定义预设
curl -X POST http://localhost:8000/api/prompts \
  -H "Content-Type: application/json" \
  -d '{
    "name": "我的预设",
    "description": "自定义预设",
    "category": "custom",
    "prompt_template": "请优化: {text}"
  }' | jq

# 预览预设效果
curl -X POST "http://localhost:8000/api/prompts/builtin-general/preview?text=测试文本" | jq

# 设置默认预设
curl -X PUT "http://localhost:8000/api/prompts/default?preset_id=builtin-coding" | jq
```

### 2. 会话API测试

```bash
# 创建会话
SESSION_ID=$(curl -s -X POST http://localhost:8000/api/sessions | jq -r '.session_id')
echo "Session ID: $SESSION_ID"

# 获取会话详情
curl http://localhost:8000/api/sessions/$SESSION_ID | jq

# 上传片段 (需要实际音频文件)
curl -X POST http://localhost:8000/api/sessions/$SESSION_ID/segments \
  -F "file=@test_audio.webm" | jq

# 合并会话
curl -X POST http://localhost:8000/api/sessions/$SESSION_ID/merge | jq

# 删除会话
curl -X DELETE http://localhost:8000/api/sessions/$SESSION_ID | jq
```

### 3. 设置API测试

```bash
# 获取设置
curl http://localhost:8000/api/settings | jq

# 更新设置
curl -X PUT http://localhost:8000/api/settings \
  -H "Content-Type: application/json" \
  -d '{
    "recording_mode": "multi",
    "silence_threshold": 2.0,
    "default_preset_id": "builtin-coding"
  }' | jq
```

---

## 前端功能测试

### 1. 设置页面测试

**测试步骤**:
1. 打开 http://localhost:8000
2. 点击侧边栏"设置"
3. 验证显示以下选项:
   - [ ] 识别模型选择
   - [ ] 计算设备选择
   - [ ] 录音模式选择 (单段/多段)
   - [ ] 静音阈值输入 (选择多段模式后显示)
   - [ ] AI优化预设选择
   - [ ] 预设管理按钮

### 2. 预设管理测试

**测试步骤**:
1. 点击"管理预设"按钮
2. 验证预设列表显示5个内置预设
3. 点击"新建预设"
4. 填写:
   - 名称: "测试预设"
   - 描述: "测试描述"
   - 分类: "自定义"
   - 模板: "测试: {text}"
5. 点击"保存预设"
6. 验证新预设出现在列表中
7. 点击新预设的编辑按钮
8. 修改名称为"已修改测试预设"
9. 保存并验证
10. 点击删除按钮
11. 确认删除
12. 验证预设已从列表移除

### 3. 多段录音测试

**测试步骤**:
1. 设置页面选择"多段录音"
2. 设置静音阈值为1.5秒
3. 返回首页
4. 点击"开始录音"
5. 说话约3秒
6. 停止说话，等待1.5秒 (静音触发分割)
7. 观察界面是否显示"片段已上传"
8. 继续说话约3秒
9. 再次停止说话等待
10. 点击"停止录音"
11. 验证转写文本显示
12. 验证WebSocket状态通知

### 4. WebSocket通知测试

**浏览器控制台测试**:
```javascript
// 检查WebSocket连接
console.log(window.app?.wsClient?.isConnected);

// 监听转录通知
window.app.wsClient.on('transcription_started', (data) => {
    console.log('转录开始:', data);
});
window.app.wsClient.on('transcription_completed', (data) => {
    console.log('转录完成:', data);
});
window.app.wsClient.on('segment_transcribed', (data) => {
    console.log('片段转录:', data);
});
```

---

## 端到端测试流程

### 完整录音流程测试

```bash
# 1. 启动服务器
uvicorn app.main:app --reload

# 2. 打开浏览器 http://localhost:8000

# 3. 设置页面配置:
#    - 录音模式: 多段
#    - 静音阈值: 1.5秒
#    - 自动AI优化: 开启
#    - 预设: 程序员模式

# 4. 开始录音
# 5. 说话内容: "我想测试一下多段录音功能，看看实时转写效果如何"
# 6. 停止说话等待2秒
# 7. 继续说话: "第二段内容，验证片段分割"
# 8. 停止录音

# 9. 验证:
#    - 转写文本正确显示
#    - 文本经过AI优化
#    - 历史记录中出现新记录
```

---

## 测试检查清单

### 后端测试
- [ ] 提示词模块单元测试通过
- [ ] WebSocket消息类型测试通过
- [ ] 多段录音模型测试通过
- [ ] 用户设置测试通过
- [ ] 所有API端点响应正常

### 前端测试
- [ ] 设置页面UI正确显示
- [ ] 预设管理功能正常
- [ ] 多段录音流程正常
- [ ] WebSocket消息正确处理
- [ ] 轮询降级机制正常

### 集成测试
- [ ] 完整录音流程端到端测试通过
- [ ] AI优化预设选择生效
- [ ] 历史记录正确保存

---

## 已知问题与解决方案

### 1. Safari浏览器MediaRecorder兼容性

**问题**: Safari对webm格式支持有限

**解决方案**: 使用mp4格式或检测浏览器提供兼容格式

### 2. 静音检测灵敏度

**问题**: 不同麦克风灵敏度不同，-40dB可能不适合所有设备

**解决方案**: 提供静音级别可配置选项 (已实现)

### 3. WebSocket断线重连

**问题**: 网络不稳定时可能丢失消息

**解决方案**: 已实现轮询降级机制
