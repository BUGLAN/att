"""服务商注册表：只登记「协议前缀 + 元信息」，不登记模型清单。

与 v1 的本质区别（见 docs/LLM配置模块重设计.md §6.3）：
- v1 的 `PROVIDER_INFO` 把每家服务商的模型清单硬编码进代码，随时间腐烂；
- v2 只登记 LiteLLM 协议前缀，模型名由用户拉取/手输后**原样透传**，
  服务商发布新模型无需修改本文件。

所有前缀的可用性以锁定版本的 LiteLLM 实际支持列表为准；未收录的服务商
统一走 custom（openai/ + 自定义端点）兜底。
"""
from __future__ import annotations

from dataclasses import dataclass, field


@dataclass(frozen=True)
class ProviderSpec:
    """单个服务商的接入元信息。"""

    id: str
    name: str
    litellm_prefix: str                  # 拼进模型串的前缀，如 deepseek/xxx
    default_base_url: str = ""           # 空 = 使用 LiteLLM 内置默认端点
    requires_key: bool = True
    supports_model_listing: bool = True  # 上游是否提供 OpenAI 兼容 GET /models
    docs_url: str = ""
    notes: str = ""
    suggested_models: tuple[str, ...] = field(default=())
    # 前缀在当前 LiteLLM 版本未收录时，自动回退到 openai/ + 该端点（网关内实现）
    openai_compat_base: str = ""


PROVIDER_SPECS: tuple[ProviderSpec, ...] = (
    ProviderSpec(
        id="deepseek",
        name="DeepSeek",
        litellm_prefix="deepseek",
        default_base_url="https://api.deepseek.com",
        docs_url="https://api-docs.deepseek.com/zh-cn/",
        suggested_models=("deepseek-flash", "deepseek-v4-pro"),
    ),
    ProviderSpec(
        id="openai",
        name="OpenAI",
        litellm_prefix="openai",
        default_base_url="https://api.openai.com/v1",
        docs_url="https://platform.openai.com/docs",
    ),
    ProviderSpec(
        id="anthropic",
        name="Anthropic",
        litellm_prefix="anthropic",
        default_base_url="https://api.anthropic.com",
        # Anthropic 的模型列表接口不是 OpenAI 兼容 /models，先引导手输
        supports_model_listing=False,
        docs_url="https://docs.anthropic.com/",
    ),
    ProviderSpec(
        id="gemini",
        name="Google Gemini",
        litellm_prefix="gemini",
        requires_key=True,
        supports_model_listing=False,
        docs_url="https://ai.google.dev/gemini-api/docs",
    ),
    ProviderSpec(
        id="dashscope",
        name="通义千问（阿里云百炼）",
        litellm_prefix="dashscope",
        default_base_url="https://dashscope.aliyuncs.com/compatible-mode/v1",
        docs_url="https://help.aliyun.com/zh/model-studio/",
    ),
    ProviderSpec(
        id="doubao",
        name="豆包（火山方舟）",
        litellm_prefix="doubao",
        default_base_url="https://ark.cn-beijing.volces.com/api/v3",
        notes="若当前 LiteLLM 版本未收录 doubao 前缀，网关会自动回退为 openai/ + 方舟端点。",
        openai_compat_base="https://ark.cn-beijing.volces.com/api/v3",
        docs_url="https://www.volcengine.com/docs/82379",
    ),
    ProviderSpec(
        id="moonshot",
        name="Moonshot Kimi",
        litellm_prefix="moonshot",
        default_base_url="https://api.moonshot.cn/v1",
        docs_url="https://platform.moonshot.cn/docs",
    ),
    ProviderSpec(
        id="zhipu",
        name="智谱 GLM",
        litellm_prefix="zhipu",
        docs_url="https://open.bigmodel.cn/dev/api",
    ),
    ProviderSpec(
        id="groq",
        name="Groq",
        litellm_prefix="groq",
        docs_url="https://console.groq.com/docs",
    ),
    ProviderSpec(
        id="mistral",
        name="Mistral",
        litellm_prefix="mistral",
        docs_url="https://docs.mistral.ai/",
    ),
    ProviderSpec(
        id="xai",
        name="xAI",
        litellm_prefix="xai",
        docs_url="https://docs.x.ai/",
    ),
    ProviderSpec(
        id="openrouter",
        name="OpenRouter",
        litellm_prefix="openrouter",
        default_base_url="https://openrouter.ai/api/v1",
        docs_url="https://openrouter.ai/docs",
    ),
    ProviderSpec(
        id="ollama",
        name="Ollama（本地）",
        litellm_prefix="ollama_chat",
        default_base_url="http://127.0.0.1:11434",
        requires_key=False,
        docs_url="https://github.com/ollama/ollama/blob/main/docs/api.md",
    ),
    ProviderSpec(
        id="custom",
        name="自定义 OpenAI 兼容",
        litellm_prefix="openai",
        requires_key=False,
        notes="任何 OpenAI 兼容端点（硅基流动、第三方中转、vLLM/LM Studio 等）都走这里。",
        docs_url="",
    ),
)

PROVIDER_MAP: dict[str, ProviderSpec] = {spec.id: spec for spec in PROVIDER_SPECS}

# v1 配置的 provider 枚举值 → v2 注册表 id（迁移用，见 app/llm/migration.py）
V1_PROVIDER_MAP: dict[str, str] = {
    "deepseek": "deepseek",
    "openai": "openai",
    "claude": "anthropic",
    "qwen": "dashscope",
    # 火山方舟 100% OpenAI 兼容，直接走 openai/ + 方舟端点最稳
    "doubao": "custom",
    "custom": "custom",
}


def get_provider_spec(provider_id: str) -> ProviderSpec | None:
    """按 id 取服务商元信息；未知 id 返回 None。"""
    return PROVIDER_MAP.get(provider_id)


def compose_model_string(provider_id: str, model_name: str, api_base: str | None = None) -> str:
    """把「模型名」组装成 LiteLLM 模型串。

    规则：
    - **custom 必须最先判断**：本地网关 / 中转（vLLM、one-api 类、LM Studio 等）的
      模型 ID 常自带斜杠（如 `group/deepseek-v4.1-flash`）。若按「含 / 原样透传」
      处理，LiteLLM 会把 `group` 当成服务商前缀而报
      "LLM Provider NOT provided"。因此 custom 一律组装为
      `openai/<模型ID>`（`openai/` 前缀让 LiteLLM 走 OpenAI 兼容通道，
      调用时会剥掉这一层、把完整模型 ID 原样交给你的端点）；
    - 其余服务商：已带 "/" 的名字视为完整模型串原样透传，裸名按注册表前缀组装。
    """
    name = (model_name or "").strip()
    if not name:
        raise ValueError("模型名不能为空")
    spec = get_provider_spec(provider_id)
    if spec is None:
        raise ValueError(f"未知服务商：{provider_id}")
    if spec.id == "custom":
        if not (api_base or "").strip():
            raise ValueError("自定义 OpenAI 兼容端点必须填写 API Base")
        # removeprefix 保证幂等：上游模型 ID 本身叫 openai/xxx 时不会拼成 openai/openai/xxx
        return f"openai/{name.removeprefix('openai/')}"
    if "/" in name:
        return name
    return f"{spec.litellm_prefix}/{name}"


def provider_catalog() -> list[dict]:
    """输出给 GET /api/llm/providers 的目录列表。"""
    return [
        {
            "id": spec.id,
            "name": spec.name,
            "litellm_prefix": spec.litellm_prefix,
            "default_base_url": spec.default_base_url,
            "requires_key": spec.requires_key,
            "supports_model_listing": spec.supports_model_listing,
            "docs_url": spec.docs_url,
            "notes": spec.notes,
            "suggested_models": list(spec.suggested_models),
        }
        for spec in PROVIDER_SPECS
    ]
