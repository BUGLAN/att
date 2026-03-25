"""
LLM Configuration Models
Data models for multi-provider LLM configuration
"""
from pydantic import BaseModel
from typing import Optional, Dict, Any, List
from enum import Enum
from datetime import datetime


class LLMProvider(str, Enum):
    """Supported LLM providers."""
    DEEPSEEK = "deepseek"
    CLAUDE = "claude"
    OPENAI = "openai"
    DOUBAO = "doubao"       # 豆包
    QWEN = "qwen"           # 通义千问
    CUSTOM = "custom"       # 自定义 OpenAI 兼容


class LLMConfig(BaseModel):
    """LLM provider configuration."""
    id: str
    provider: LLMProvider
    name: str                           # User-defined name
    api_key_encrypted: str              # Encrypted API key
    api_base: Optional[str] = None      # Custom API endpoint
    model: str                          # Model name
    temperature: float = 0.7
    max_tokens: int = 4096
    extra_params: Dict[str, Any] = {}   # Additional parameters
    is_default: bool = False
    created_at: datetime
    updated_at: Optional[datetime] = None

    class Config:
        json_encoders = {
            datetime: lambda v: v.isoformat()
        }


class LLMConfigCreate(BaseModel):
    """Model for creating a new LLM config."""
    provider: LLMProvider
    name: str
    api_key: str                        # Plain text, will be encrypted
    api_base: Optional[str] = None
    model: str
    temperature: float = 0.7
    max_tokens: int = 4096
    extra_params: Dict[str, Any] = {}


class LLMConfigUpdate(BaseModel):
    """Model for updating an existing LLM config."""
    name: Optional[str] = None
    api_key: Optional[str] = None       # Plain text, will be encrypted
    api_base: Optional[str] = None
    model: Optional[str] = None
    temperature: Optional[float] = None
    max_tokens: Optional[int] = None
    extra_params: Optional[Dict[str, Any]] = None


# Provider metadata
PROVIDER_INFO = {
    LLMProvider.DEEPSEEK: {
        "name": "DeepSeek",
        "default_base_url": "https://api.deepseek.com/v1",
        "models": ["deepseek-chat", "deepseek-coder"],
        "default_model": "deepseek-chat"
    },
    LLMProvider.CLAUDE: {
        "name": "Claude (Anthropic)",
        "default_base_url": "https://api.anthropic.com",
        "models": ["claude-3-5-sonnet-20241022", "claude-3-opus-20240229", "claude-3-haiku-20240307"],
        "default_model": "claude-3-5-sonnet-20241022"
    },
    LLMProvider.OPENAI: {
        "name": "OpenAI",
        "default_base_url": "https://api.openai.com/v1",
        "models": ["gpt-4o", "gpt-4-turbo", "gpt-3.5-turbo"],
        "default_model": "gpt-4o"
    },
    LLMProvider.DOUBAO: {
        "name": "豆包 (字节跳动)",
        "default_base_url": "https://ark.cn-beijing.volces.com/api/v3",
        "models": ["doubao-pro-32k", "doubao-lite-32k"],
        "default_model": "doubao-pro-32k"
    },
    LLMProvider.QWEN: {
        "name": "通义千问 (阿里云)",
        "default_base_url": "https://dashscope.aliyuncs.com/compatible-mode/v1",
        "models": ["qwen-turbo", "qwen-plus", "qwen-max"],
        "default_model": "qwen-turbo"
    },
    LLMProvider.CUSTOM: {
        "name": "自定义 (OpenAI兼容)",
        "default_base_url": "",
        "models": [],
        "default_model": ""
    }
}


def get_provider_models(provider: LLMProvider) -> List[str]:
    """Get available models for a provider."""
    return PROVIDER_INFO.get(provider, {}).get("models", [])


def get_default_model(provider: LLMProvider) -> str:
    """Get default model for a provider."""
    return PROVIDER_INFO.get(provider, {}).get("default_model", "")


def get_default_base_url(provider: LLMProvider) -> str:
    """Get default base URL for a provider."""
    return PROVIDER_INFO.get(provider, {}).get("default_base_url", "")
