//! 服务商注册表（移植自 Python `app/llm/registry.py`）：只登记「协议前缀 + 元信息」，不登记模型清单。
//!
//! 模型名由用户拉取/手输后原样保存为「前缀/模型名」形式的模型串（与 Python 版 LiteLLM 模型串
//! 格式一致，`data/llm_v2.json` 两版通用）；调用时由网关按档案的服务商解析出线上端点与模型 ID。

use serde_json::{json, Value};

/// 调用协议：除 Anthropic 外全部走 OpenAI 兼容 `/chat/completions`。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Protocol {
    OpenAiCompat,
    Anthropic,
}

#[derive(Debug, Clone)]
pub struct ProviderSpec {
    pub id: &'static str,
    pub name: &'static str,
    /// 拼进模型串的前缀，如 deepseek/xxx
    pub prefix: &'static str,
    /// 目录展示用的默认端点（空 = 由接入层内置，见 `wire_base`）
    pub default_base_url: &'static str,
    pub requires_key: bool,
    /// 上游是否提供 OpenAI 兼容 GET /models
    pub supports_model_listing: bool,
    pub docs_url: &'static str,
    pub notes: &'static str,
    pub suggested_models: &'static [&'static str],
    /// 实际调用的内置端点（用户未填 API Base 时使用）；custom 为空，必须由用户提供
    pub wire_base: &'static str,
    pub protocol: Protocol,
}

const fn spec(id: &'static str, name: &'static str, prefix: &'static str) -> ProviderSpec {
    ProviderSpec {
        id,
        name,
        prefix,
        default_base_url: "",
        requires_key: true,
        supports_model_listing: true,
        docs_url: "",
        notes: "",
        suggested_models: &[],
        wire_base: "",
        protocol: Protocol::OpenAiCompat,
    }
}

/// 与 Python 版 PROVIDER_SPECS 逐项对齐（顺序即设置页展示顺序）。
pub static PROVIDERS: &[ProviderSpec] = &[
    ProviderSpec {
        default_base_url: "https://api.deepseek.com",
        wire_base: "https://api.deepseek.com",
        docs_url: "https://api-docs.deepseek.com/zh-cn/",
        suggested_models: &["deepseek-flash", "deepseek-v4-pro"],
        ..spec("deepseek", "DeepSeek", "deepseek")
    },
    ProviderSpec {
        default_base_url: "https://api.openai.com/v1",
        wire_base: "https://api.openai.com/v1",
        docs_url: "https://platform.openai.com/docs",
        ..spec("openai", "OpenAI", "openai")
    },
    ProviderSpec {
        default_base_url: "https://api.anthropic.com",
        wire_base: "https://api.anthropic.com",
        // Anthropic 的模型列表接口不是 OpenAI 兼容 /models，先引导手输
        supports_model_listing: false,
        docs_url: "https://docs.anthropic.com/",
        protocol: Protocol::Anthropic,
        ..spec("anthropic", "Anthropic", "anthropic")
    },
    ProviderSpec {
        // Gemini 官方提供 OpenAI 兼容端点
        wire_base: "https://generativelanguage.googleapis.com/v1beta/openai",
        supports_model_listing: false,
        docs_url: "https://ai.google.dev/gemini-api/docs",
        ..spec("gemini", "Google Gemini", "gemini")
    },
    ProviderSpec {
        default_base_url: "https://dashscope.aliyuncs.com/compatible-mode/v1",
        wire_base: "https://dashscope.aliyuncs.com/compatible-mode/v1",
        docs_url: "https://help.aliyun.com/zh/model-studio/",
        ..spec("dashscope", "通义千问（阿里云百炼）", "dashscope")
    },
    ProviderSpec {
        default_base_url: "https://ark.cn-beijing.volces.com/api/v3",
        wire_base: "https://ark.cn-beijing.volces.com/api/v3",
        notes: "火山方舟为 OpenAI 兼容接口，模型名填推理接入点 ID 或模型 ID。",
        docs_url: "https://www.volcengine.com/docs/82379",
        ..spec("doubao", "豆包（火山方舟）", "doubao")
    },
    ProviderSpec {
        default_base_url: "https://api.moonshot.cn/v1",
        wire_base: "https://api.moonshot.cn/v1",
        docs_url: "https://platform.moonshot.cn/docs",
        ..spec("moonshot", "Moonshot Kimi", "moonshot")
    },
    ProviderSpec {
        wire_base: "https://open.bigmodel.cn/api/paas/v4",
        docs_url: "https://open.bigmodel.cn/dev/api",
        ..spec("zhipu", "智谱 GLM", "zhipu")
    },
    ProviderSpec {
        wire_base: "https://api.groq.com/openai/v1",
        docs_url: "https://console.groq.com/docs",
        ..spec("groq", "Groq", "groq")
    },
    ProviderSpec {
        wire_base: "https://api.mistral.ai/v1",
        docs_url: "https://docs.mistral.ai/",
        ..spec("mistral", "Mistral", "mistral")
    },
    ProviderSpec {
        wire_base: "https://api.x.ai/v1",
        docs_url: "https://docs.x.ai/",
        ..spec("xai", "xAI", "xai")
    },
    ProviderSpec {
        default_base_url: "https://openrouter.ai/api/v1",
        wire_base: "https://openrouter.ai/api/v1",
        docs_url: "https://openrouter.ai/docs",
        ..spec("openrouter", "OpenRouter", "openrouter")
    },
    ProviderSpec {
        default_base_url: "http://127.0.0.1:11434",
        wire_base: "http://127.0.0.1:11434",
        requires_key: false,
        docs_url: "https://github.com/ollama/ollama/blob/main/docs/api.md",
        ..spec("ollama", "Ollama（本地）", "ollama_chat")
    },
    ProviderSpec {
        requires_key: false,
        notes: "任何 OpenAI 兼容端点（硅基流动、第三方中转、vLLM/LM Studio 等）都走这里。",
        ..spec("custom", "自定义 OpenAI 兼容", "openai")
    },
];

/// v1 配置的 provider 枚举值 → v2 注册表 id（迁移用）；未知值保守映射为 custom。
pub fn v1_provider_id(v1_provider: &str) -> &'static str {
    match v1_provider.trim().to_ascii_lowercase().as_str() {
        "deepseek" => "deepseek",
        "openai" => "openai",
        "claude" => "anthropic",
        "qwen" => "dashscope",
        // 火山方舟 100% OpenAI 兼容，直接走 openai/ + 方舟端点最稳
        _ => "custom",
    }
}

pub fn get(provider_id: &str) -> Option<&'static ProviderSpec> {
    PROVIDERS.iter().find(|spec| spec.id == provider_id)
}

/// 把「模型名」组装成模型串（规则与 Python `compose_model_string` 一致）。
///
/// - custom 必须最先判断：网关/中转的模型 ID 常自带斜杠（如 `group/deepseek-v4.1-flash`），
///   一律组装为 `openai/<模型ID>`，且幂等（不会拼成 openai/openai/xxx）；
/// - 其余服务商：已带 "/" 的名字视为完整模型串原样保存，裸名按注册表前缀组装。
pub fn compose_model_string(provider_id: &str, model_name: &str, api_base: Option<&str>) -> Result<String, String> {
    let name = model_name.trim();
    if name.is_empty() {
        return Err("模型名不能为空".into());
    }
    let spec = get(provider_id).ok_or_else(|| format!("未知服务商：{provider_id}"))?;
    if spec.id == "custom" {
        if api_base.map(str::trim).unwrap_or("").is_empty() {
            return Err("自定义 OpenAI 兼容端点必须填写 API Base".into());
        }
        return Ok(format!("openai/{}", name.strip_prefix("openai/").unwrap_or(name)));
    }
    if name.contains('/') {
        return Ok(name.to_string());
    }
    Ok(format!("{}/{name}", spec.prefix))
}

/// 模型串 → 发给上游的模型 ID：剥掉本服务商的前缀（custom 剥 `openai/`），其余原样。
pub fn wire_model_name<'a>(spec: &ProviderSpec, model_string: &'a str) -> &'a str {
    model_string
        .strip_prefix(spec.prefix)
        .and_then(|rest| rest.strip_prefix('/'))
        .unwrap_or(model_string)
}

/// GET /api/llm/providers 的目录项（字段与 Python 版一致）。
pub fn catalog() -> Value {
    Value::Array(
        PROVIDERS
            .iter()
            .map(|spec| {
                json!({
                    "id": spec.id,
                    "name": spec.name,
                    "litellm_prefix": spec.prefix,
                    "default_base_url": spec.default_base_url,
                    "requires_key": spec.requires_key,
                    "supports_model_listing": spec.supports_model_listing,
                    "docs_url": spec.docs_url,
                    "notes": spec.notes,
                    "suggested_models": spec.suggested_models,
                })
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 模型串组装_与_python_规则一致() {
        assert_eq!(compose_model_string("deepseek", "deepseek-flash", None).unwrap(), "deepseek/deepseek-flash");
        assert_eq!(compose_model_string("openrouter", "anthropic/claude", None).unwrap(), "anthropic/claude");
        // custom：斜杠模型 ID 也要包一层 openai/，且幂等
        assert_eq!(compose_model_string("custom", "group/x", Some("http://h")).unwrap(), "openai/group/x");
        assert_eq!(compose_model_string("custom", "openai/x", Some("http://h")).unwrap(), "openai/x");
        assert!(compose_model_string("custom", "x", None).is_err());
        assert!(compose_model_string("deepseek", "  ", None).is_err());
        assert!(compose_model_string("nope", "x", None).is_err());
    }

    #[test]
    fn 线上模型名_剥掉本服务商前缀() {
        let custom = get("custom").unwrap();
        assert_eq!(wire_model_name(custom, "openai/group/x"), "group/x");
        let deepseek = get("deepseek").unwrap();
        assert_eq!(wire_model_name(deepseek, "deepseek/deepseek-chat"), "deepseek-chat");
        let openrouter = get("openrouter").unwrap();
        assert_eq!(wire_model_name(openrouter, "anthropic/claude"), "anthropic/claude");
        let ollama = get("ollama").unwrap();
        assert_eq!(wire_model_name(ollama, "ollama_chat/qwen3"), "qwen3");
    }

    #[test]
    fn 目录_14_家且字段齐全() {
        let catalog = catalog();
        let items = catalog.as_array().unwrap();
        assert_eq!(items.len(), 14);
        assert_eq!(items[0]["id"], "deepseek");
        assert_eq!(items[0]["suggested_models"][0], "deepseek-flash");
        assert_eq!(v1_provider_id("Claude"), "anthropic");
        assert_eq!(v1_provider_id("doubao"), "custom");
    }
}
