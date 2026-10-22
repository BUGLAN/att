"""LLM v2 数据模型：供应商档案（ProviderProfile）+ 模型条目（ModelEntry）。

设计要点（docs/LLM配置模块重设计.md §5）：
- 密钥只存在档案级（api_key_encrypted），模型条目通过 profile_id 挂靠，不重复存密钥；
- 模型名是**原样透传**给 LiteLLM 的完整模型串（如 deepseek/deepseek-flash），
  服务商发布新模型无需改代码；
- status 只记录最近一次验证结果，状态字段全部可缺省，容忍旧数据缺字段。

注意：本文件全部使用 Pydantic v2 风格（model_dump / Field），不要回退 v1 的
`class Config` 写法（Pydantic 3 移除后会在 import 期崩掉整个后端）。
"""
from __future__ import annotations

from typing import Any, Dict, List, Optional

from pydantic import BaseModel, Field

# 档案/模型最近一次验证的状态值
STATUS_UNKNOWN = "unknown"
STATUS_OK = "ok"
STATUS_FAILED = "failed"
STATUS_DECRYPT_FAILED = "decrypt_failed"


class VerifyStatus(BaseModel):
    """最近一次验证结果的公共结构（档案与模型共用）。"""

    state: str = STATUS_UNKNOWN
    checked_at: Optional[str] = None      # ISO 时间字符串，避免 datetime 序列化差异
    latency_ms: Optional[int] = None
    error_code: Optional[str] = None      # 见 app/llm/errors.py 的错误分级
    error: Optional[str] = None           # 已脱敏的失败原因（面向用户展示）


class ModelParams(BaseModel):
    """模型级调用参数。timeout_s / retries 由网关在每次请求强制注入。"""

    temperature: float = 0.3
    max_tokens: int = 4096
    timeout_s: int = Field(default=60, ge=5, le=600)
    retries: int = Field(default=2, ge=0, le=5)


class ProviderProfile(BaseModel):
    """供应商档案：一家服务商的凭据与端点，密钥只存这一份。"""

    id: str
    provider: str                          # 注册表 id（app/llm/registry.py）
    name: str
    api_key_encrypted: str = ""            # Fernet 密文；本地 Ollama 等允许为空
    api_key_hint: str = ""                 # 尾号提示（如 sk-****7f3a），非敏感
    api_base: Optional[str] = None         # None = 使用注册表/LiteLLM 默认端点
    extra_headers: Dict[str, str] = Field(default_factory=dict)
    extra_query: Dict[str, str] = Field(default_factory=dict)
    status: VerifyStatus = Field(default_factory=VerifyStatus)


class ModelEntry(BaseModel):
    """模型条目：挂在某个档案下的可调用模型。"""

    id: str
    profile_id: str
    model: str                             # 完整 LiteLLM 模型串，原样透传
    display_name: str = ""                 # 界面显示名；空则回退用模型串
    params: ModelParams = Field(default_factory=ModelParams)
    use_cases: Dict[str, bool] = Field(default_factory=lambda: {"optimize": True})
    status: VerifyStatus = Field(default_factory=VerifyStatus)

    def display_label(self) -> str:
        return self.display_name or self.model


class LLMStoreData(BaseModel):
    """data/llm_v2.json 的顶层结构。"""

    version: int = 2
    profiles: List[ProviderProfile] = Field(default_factory=list)
    models: List[ModelEntry] = Field(default_factory=list)
    default_model_id: Optional[str] = None
    fallback_model_id: Optional[str] = None


# ==================== API 请求模型 ====================


class ProfileCreate(BaseModel):
    provider: str
    name: str
    api_key: str = ""                      # 明文仅出现在请求里，落盘前加密
    api_base: Optional[str] = None
    extra_headers: Dict[str, str] = Field(default_factory=dict)
    extra_query: Dict[str, str] = Field(default_factory=dict)


class ProfileUpdate(BaseModel):
    name: Optional[str] = None
    api_key: Optional[str] = None          # None / 缺省 = 保持现有密钥
    api_base: Optional[str] = None
    extra_headers: Optional[Dict[str, str]] = None
    extra_query: Optional[Dict[str, str]] = None


class ModelItemCreate(BaseModel):
    """批量添加模型时的单条描述。model 可传裸名（自动按档案前缀组装）或完整模型串。"""

    profile_id: str
    model: str
    display_name: str = ""
    params: Optional[ModelParams] = None
    set_default: bool = False


class ModelsBatchCreate(BaseModel):
    items: List[ModelItemCreate] = Field(min_length=1, max_length=50)


class ModelUpdate(BaseModel):
    display_name: Optional[str] = None
    model: Optional[str] = None
    params: Optional[ModelParams] = None


class DraftVerifyRequest(BaseModel):
    """保存前验证：草稿参数只活在本次请求的内存里，验证后即弃，不落盘。"""

    provider: str
    api_key: str = ""
    api_base: Optional[str] = None
    extra_headers: Dict[str, str] = Field(default_factory=dict)
    model_name: Optional[str] = None       # 提供时验证到模型级，否则只验证档案级


class LLMSettingsUpdate(BaseModel):
    default_model_id: Optional[str] = None
    fallback_model_id: Optional[str] = None


# 任何对外响应都不得包含 api_key / api_key_encrypted 字段（硬约束 §3.4）
def profile_public_dict(profile: ProviderProfile) -> dict:
    """档案的安全序列化：只含非敏感字段。"""
    return profile.model_dump(exclude={"api_key_encrypted"})


def model_public_dict(entry: ModelEntry) -> dict:
    return entry.model_dump()


def store_public_dict(data: LLMStoreData, skipped: List[str] | None = None) -> dict:
    """GET /api/llm/state 的载荷：档案 + 模型 + 默认绑定，一次拉全。"""
    return {
        "version": data.version,
        "profiles": [profile_public_dict(p) for p in data.profiles],
        "models": [model_public_dict(m) for m in data.models],
        "default_model_id": data.default_model_id,
        "fallback_model_id": data.fallback_model_id,
        "skipped_entries": skipped or [],
    }


def to_jsonable(data: LLMStoreData) -> Dict[str, Any]:
    """落盘用的完整序列化（含密文字段），必须经 json_store 原子写。"""
    return data.model_dump(mode="json")
