# 第二阶段实施计划：多LLM服务商支持

## 概述

**优先级**: P1 (中优先级)
**预计工作量**: 中等
**前置条件**: 第一阶段已完成

## 目标

将单一DeepSeek配置扩展为支持多个LLM服务商，用户可：
- 配置多个API密钥
- 快速切换服务商
- 自定义API端点
- 安全存储密钥

## 支持的服务商

| 服务商 | Provider ID | API格式 | 模型示例 |
|--------|-------------|---------|----------|
| DeepSeek | deepseek | OpenAI兼容 | deepseek-chat, deepseek-coder |
| Claude | claude | Anthropic | claude-3-5-sonnet, claude-3-opus |
| OpenAI | openai | OpenAI | gpt-4o, gpt-4-turbo |
| 豆包 | doubao | OpenAI兼容 | doubao-pro-32k |
| 通义千问 | qwen | OpenAI兼容 | qwen-turbo, qwen-max |

---

## 任务分解

### Task 1: 创建LLM配置数据模型

**文件**: `app/llm/models.py`

**内容**:
```python
class LLMProvider(str, Enum):
    DEEPSEEK = "deepseek"
    CLAUDE = "claude"
    OPENAI = "openai"
    DOUBAO = "doubao"
    QWEN = "qwen"
    CUSTOM = "custom"

class LLMConfig(BaseModel):
    id: str
    provider: LLMProvider
    name: str                      # 用户定义名称
    api_key_encrypted: str         # 加密后的密钥
    api_base: Optional[str]        # 自定义端点
    model: str
    temperature: float = 0.7
    max_tokens: int = 4096
    extra_params: Dict[str, Any] = {}
    is_default: bool = False
    created_at: datetime
    updated_at: Optional[datetime]

class LLMConfigCreate(BaseModel):
    provider: LLMProvider
    name: str
    api_key: str                   # 明文，存储时加密
    api_base: Optional[str] = None
    model: str
    temperature: float = 0.7
    max_tokens: int = 4096

class LLMConfigUpdate(BaseModel):
    name: Optional[str] = None
    api_key: Optional[str] = None
    api_base: Optional[str] = None
    model: Optional[str] = None
    temperature: Optional[float] = None
    max_tokens: Optional[int] = None
```

**验收标准**:
- Pydantic模型验证通过
- 支持序列化/反序列化

---

### Task 2: 创建API密钥加密工具

**文件**: `app/utils/encryption.py`

**功能**:
- 使用Fernet对称加密
- 密钥文件存储在 `data/.encryption_key`
- 提供 encrypt/decrypt 方法

**验收标准**:
- 加密后的密钥无法直接读取
- 解密后得到原始密钥

---

### Task 3: 创建Provider抽象基类

**文件**: `app/llm/base.py`

**内容**:
```python
class BaseLLMProvider(ABC):
    @abstractmethod
    async def optimize(self, text: str, prompt_template: str) -> str:
        """优化文本"""
        pass

    @abstractmethod
    async def test_connection(self) -> bool:
        """测试连接"""
        pass

    @abstractmethod
    def get_available_models(self) -> List[str]:
        """获取可用模型列表"""
        pass
```

**验收标准**:
- 抽象方法定义清晰
- 支持子类继承

---

### Task 4: 实现各Provider

**文件结构**:
```
app/llm/providers/
├── __init__.py
├── deepseek.py
├── claude.py
├── openai.py
├── doubao.py
└── qwen.py
```

**每个Provider实现**:
- 继承 BaseLLMProvider
- 实现 optimize 方法
- 实现 test_connection 方法
- 实现 get_available_models 方法

**验收标准**:
- 每个Provider可独立工作
- 支持自定义API端点

---

### Task 5: 创建LLM配置管理器

**文件**: `app/llm/config_manager.py`

**功能**:
- 加载/保存配置到 `data/llm_configs.json`
- 加密/解密API密钥
- CRUD操作
- 设置默认配置

**验收标准**:
- 配置持久化正常
- 密钥加密存储

---

### Task 6: 创建LLM Gateway

**文件**: `app/llm/gateway.py`

**功能**:
- 根据配置ID获取Provider实例
- 工厂模式创建Provider
- 统一的优化接口

**验收标准**:
- 根据配置动态创建Provider
- 支持热切换

---

### Task 7: 添加LLM配置API端点

**文件**: `app/api/routes.py`

**API设计**:
```
GET    /api/llm/configs              # 获取所有配置
POST   /api/llm/configs              # 创建配置
PUT    /api/llm/configs/{id}         # 更新配置
DELETE /api/llm/configs/{id}         # 删除配置
PUT    /api/llm/configs/{id}/default # 设置默认
POST   /api/llm/configs/{id}/test    # 测试连接
GET    /api/llm/providers            # 获取支持的Provider
GET    /api/llm/providers/{provider}/models  # 获取模型列表
```

**验收标准**:
- 所有端点正常工作
- 密钥不在响应中暴露

---

### Task 8: 修改TextOptimizer使用Gateway

**文件**: `app/llm/optimizer.py`

**修改**:
- 支持通过config_id选择LLM
- 保持向后兼容 (无config_id时使用DeepSeek)

**验收标准**:
- 可指定LLM配置
- 默认行为不变

---

### Task 9: 添加前端LLM配置界面

**文件**: `static/index.html`, `static/js/settings.js`

**UI元素**:
- LLM配置列表
- 添加/编辑/删除配置
- 测试连接按钮
- 模型选择下拉框
- API密钥输入框

**验收标准**:
- UI清晰易用
- 实时反馈测试结果

---

### Task 10: 数据迁移脚本

**文件**: `scripts/migrate_to_multi_llm.py`

**功能**:
- 读取 `.env` 中的 `DEEPSEEK_API_KEY`
- 创建默认DeepSeek配置
- 加密存储密钥

**验收标准**:
- 迁移后配置可用
- 无数据丢失

---

## 依赖关系图

```
Task 1 (数据模型)
    │
    ├──→ Task 2 (加密工具)
    │        │
    │        └──→ Task 5 (配置管理器)
    │                   │
    ├──→ Task 3 (基类) ──┤
    │        │          │
    │        └──→ Task 4 (Providers)
    │                   │
    └──────────────────→ Task 6 (Gateway)
                            │
                            ├──→ Task 7 (API)
                            │        │
                            └──→ Task 8 (Optimizer修改)
                                     │
                                     └──→ Task 9 (前端UI)

Task 10 (迁移) 独立执行
```

---

## 文件变更清单

| 操作 | 文件 | 说明 |
|------|------|------|
| 新增 | `app/llm/models.py` | LLM配置数据模型 |
| 新增 | `app/llm/base.py` | Provider抽象基类 |
| 新增 | `app/llm/providers/` | 各Provider实现 |
| 新增 | `app/llm/config_manager.py` | 配置管理器 |
| 新增 | `app/llm/gateway.py` | LLM网关 |
| 新增 | `app/utils/encryption.py` | 密钥加密工具 |
| 新增 | `scripts/migrate_to_multi_llm.py` | 迁移脚本 |
| 修改 | `app/api/routes.py` | 新增LLM配置API |
| 修改 | `app/llm/optimizer.py` | 支持多LLM |
| 修改 | `static/index.html` | LLM配置UI |
| 修改 | `static/js/settings.js` | LLM配置管理 |

---

## 测试计划

### 单元测试

```python
# tests/test_llm_config.py

def test_llm_config_model():
    """测试配置模型"""
    pass

def test_api_key_encryption():
    """测试密钥加密"""
    pass

def test_provider_factory():
    """测试Provider工厂"""
    pass
```

### 集成测试

```python
# tests/test_llm_gateway.py

async def test_deepseek_provider():
    """测试DeepSeek Provider"""
    pass

async def test_claude_provider():
    """测试Claude Provider"""
    pass

async def test_config_switching():
    """测试配置切换"""
    pass
```

### API测试

```bash
# 创建配置
curl -X POST http://localhost:8000/api/llm/configs \
  -H "Content-Type: application/json" \
  -d '{
    "provider": "deepseek",
    "name": "我的DeepSeek",
    "api_key": "sk-xxx",
    "model": "deepseek-chat"
  }'

# 测试连接
curl -X POST http://localhost:8000/api/llm/configs/{id}/test

# 设置默认
curl -X PUT http://localhost:8000/api/llm/configs/{id}/default
```

---

## 风险与缓解

| 风险 | 影响 | 缓解措施 |
|------|------|----------|
| API密钥泄露 | 高 | Fernet加密 + 文件权限限制 |
| Provider API变更 | 中 | 抽象层隔离 + 版本检测 |
| 网络超时 | 中 | 合理设置超时 + 重试机制 |

---

## 完成标准

- [ ] 所有任务完成
- [ ] 单元测试通过
- [ ] API测试通过
- [ ] 前端UI可用
- [ ] 迁移脚本验证
- [ ] 文档更新
