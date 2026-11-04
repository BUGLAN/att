//! LLM 优化的词表 A/B 对比（默认忽略，会真实调用默认 LLM 模型并计费）。
//!
//! 取本机记录的识别原文，按当前默认预设分别「不带词表」与「带设置里的词表」整理一遍，并排输出，
//! 用来确认词表能纠正同音 / 音译错字，又不会把原文没提到的词硬塞进去。只读记录，不改写任何结果。
//!
//! 只读真实 `data/`：`settings.json`、`llm_v2.json` 与 `.encryption_key` 复制到临时目录再初始化。
//!
//! 跑法（在 `src-tauri/` 下）：
//!
//! ```bash
//! ATT_AB_IDS=4b74b881,8172c6dd cargo test -p att-core --test llm_glossary_ab -- --ignored --nocapture
//! ```
//!
//! 环境变量：`ATT_DATA_DIR` 数据目录（默认仓库 `data/`）；`ATT_AB_IDS` 要跑的记录（逗号分隔的 id 前缀，必填）；
//! `ATT_AB_REPEAT` 每条重复几轮（默认 1，看随机波动）；`ATT_AB_TEMPERATURE` 覆盖模型温度。

use att_core::llm::gateway::{self, ChatOptions, Endpoint};
use att_core::llm::{cleaning, prompt, registry};
use att_core::state::CoreState;
use att_core::Platform;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::time::Duration;

fn data_dir() -> PathBuf {
    match std::env::var("ATT_DATA_DIR") {
        Ok(dir) if !dir.trim().is_empty() => PathBuf::from(dir),
        _ => Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../data"),
    }
}

/// 按 id 前缀取记录的识别原文（保持传入顺序）
fn raw_texts(data: &Path, ids: &[String]) -> Vec<(String, String)> {
    let records: Vec<Value> = std::fs::read_dir(data.join("records"))
        .expect("读取 records 目录")
        .filter_map(Result::ok)
        .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "json"))
        .filter_map(|entry| serde_json::from_str(&std::fs::read_to_string(entry.path()).ok()?).ok())
        .collect();
    ids.iter()
        .filter_map(|prefix| {
            let record = records.iter().find(|r| r["id"].as_str().is_some_and(|id| id.starts_with(prefix.as_str())))?;
            Some((prefix.clone(), record["raw_text"].as_str()?.to_string()))
        })
        .collect()
}

#[tokio::test]
#[ignore = "真实调用默认 LLM 模型（计费），需要本机 data/ 里有记录与 LLM 配置"]
async fn llm_glossary_ab() {
    let data = data_dir();
    let ids: Vec<String> = std::env::var("ATT_AB_IDS")
        .expect("请用 ATT_AB_IDS 指定记录（逗号分隔的 id 前缀）")
        .split(',')
        .map(|id| id.trim().to_string())
        .filter(|id| !id.is_empty())
        .collect();

    let sandbox = std::env::temp_dir().join(format!("att-llm-ab-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&sandbox);
    std::fs::create_dir_all(&sandbox).expect("创建临时目录");
    for name in ["settings.json", "llm_v2.json", ".encryption_key"] {
        std::fs::copy(data.join(name), sandbox.join(name)).unwrap_or_else(|e| panic!("复制 {name} 失败：{e}"));
    }
    let core = CoreState::init(&sandbox, Platform::Macos).expect("初始化核心状态");
    let settings = core.settings();
    let glossary = settings.asr_hotwords.clone();
    assert!(!glossary.is_empty(), "设置里的词表为空");

    let repo_prompts = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../prompts");
    let wanted = [settings.default_preset_id.as_deref()];
    let template = core.prompt_library().resolve_template(&wanted)
        .or_else(|| att_core::prompts::PromptLibrary::new(&repo_prompts, &sandbox).resolve_template(&wanted))
        .unwrap_or_else(|| prompt::DEFAULT_PROMPT.to_string());

    let target = core.llm.resolve_runtime(None).expect("默认 LLM 模型");
    let spec = registry::get(&target.profile.provider).expect("服务商");
    let endpoint = Endpoint::resolve(spec, target.profile.api_base.as_deref(), &target.api_key, &target.profile.extra_headers)
        .expect("接入点");
    let options = ChatOptions {
        max_tokens: target.model.params.max_tokens,
        temperature: Some(
            std::env::var("ATT_AB_TEMPERATURE").ok().and_then(|v| v.parse().ok()).unwrap_or(target.model.params.temperature),
        ),
        timeout: Duration::from_secs(target.model.params.timeout_s.max(30) as u64),
        retries: 1,
        reasoning_effort: target.model.params.reasoning_effort.clone(),
    };
    let repeat: usize = std::env::var("ATT_AB_REPEAT").ok().and_then(|v| v.parse().ok()).unwrap_or(1).max(1);
    println!(
        "模型 {}（温度 {:?}），预设 {:?}，词表 {} 个，每条 {repeat} 轮\n",
        target.model.model,
        options.temperature,
        settings.default_preset_id,
        glossary.len()
    );

    let samples = raw_texts(&data, &ids);
    for (id, raw) in samples.iter().flat_map(|sample| std::iter::repeat_n(sample, repeat)) {
        let mut outputs = Vec::new();
        for words in [&[][..], &glossary[..]] {
            let messages = prompt::build_messages(raw, &template, words);
            let text = match gateway::chat(&endpoint, &target.model.model, &messages, &options).await {
                Ok(result) => cleaning::clean_output(&result.text),
                Err(error) => format!("【失败】{}", error.user_message),
            };
            outputs.push(text);
        }
        println!(
            "[{id}] {}\n  原文   : {raw}\n  无词表 : {}\n  带词表 : {}\n",
            if outputs[0] == outputs[1] { "=" } else { "≠" },
            outputs[0],
            outputs[1]
        );
    }
    let _ = std::fs::remove_dir_all(&sandbox);
}
