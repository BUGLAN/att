//! 豆包识别 A/B 对比（默认忽略，会真实调用豆包并计费）。
//!
//! 用本机已存的录音（`data/audio/records/` 下的 `.flac` / 旧 `.wav`）重跑豆包两遍并排输出：
//! - 默认对比识别语言 `zh` 与留空（`auto`），用来判断中英混说时是否应该放开语言限制；
//! - 设了 `ATT_AB_HOTWORDS` 时改为对比「无热词」与「带热词」（语言固定 `zh`）。
//!
//! 只读真实 `data/`：`settings.json` 与 `.encryption_key` 会复制到临时目录再初始化，不改动原数据。
//!
//! 跑法（在 `src-tauri/` 下）：
//!
//! ```bash
//! cargo test -p att-core --test doubao_language_ab -- --ignored --nocapture
//! ```
//!
//! 可选环境变量：
//! - `ATT_DATA_DIR`：数据目录，默认仓库 `data/`；
//! - `ATT_AB_LIMIT`：最多对比多少条，默认 40（按创建时间倒序）；
//! - `ATT_AB_ALL=1`：不过滤，所有有录音的记录都参与（默认只挑原文或优化稿里出现英文单词的）；
//! - `ATT_AB_IDS`：只跑这些记录（逗号分隔的 id 前缀），此时不按英文过滤；
//! - `ATT_AB_HOTWORDS`：热词（逗号或换行分隔），设了就改为对比有无热词；
//! - `ATT_AB_OUT`：把对比表写成 Markdown 文件。

use att_core::state::CoreState;
use att_core::stt::{self, doubao};
use att_core::Platform;
use serde_json::Value;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

/// 一组对比：(标签, 识别语言, 热词)
type Variant = (&'static str, &'static str, Vec<String>);

fn variants() -> [Variant; 2] {
    match std::env::var("ATT_AB_HOTWORDS") {
        Ok(text) if !text.trim().is_empty() => {
            let words = stt::hotwords::normalize(text.split([',', '\n']));
            [("无热词", "zh", Vec::new()), ("热词", "zh", words)]
        }
        _ => [("zh", "zh", Vec::new()), ("auto", "auto", Vec::new())],
    }
}

struct Sample {
    id: String,
    created_at: String,
    wav: PathBuf,
    raw_text: String,
    optimized_text: String,
}

fn data_dir() -> PathBuf {
    match std::env::var("ATT_DATA_DIR") {
        Ok(dir) if !dir.trim().is_empty() => PathBuf::from(dir),
        _ => Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../data"),
    }
}

/// 英文单词判据：连续两个及以上 ASCII 字母
fn has_english_word(text: &str) -> bool {
    let mut run = 0;
    for c in text.chars() {
        run = if c.is_ascii_alphabetic() { run + 1 } else { 0 };
        if run >= 2 {
            return true;
        }
    }
    false
}

fn collect_samples(data: &Path, limit: usize, all: bool, ids: &[String]) -> Vec<Sample> {
    let text_of = |record: &Value, key: &str| record[key].as_str().unwrap_or_default().to_string();
    let mut samples: Vec<Sample> = std::fs::read_dir(data.join("records"))
        .expect("读取 records 目录")
        .filter_map(Result::ok)
        .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "json"))
        .filter_map(|entry| serde_json::from_str::<Value>(&std::fs::read_to_string(entry.path()).ok()?).ok())
        .filter_map(|record| {
            let wav = data.join("audio/records").join(record["audio_path"].as_str()?);
            let sample = Sample {
                id: text_of(&record, "id"),
                created_at: text_of(&record, "created_at"),
                wav,
                raw_text: text_of(&record, "raw_text"),
                optimized_text: text_of(&record, "optimized_text"),
            };
            let mixed = has_english_word(&sample.raw_text) || has_english_word(&sample.optimized_text);
            let wanted = if ids.is_empty() { all || mixed } else { ids.iter().any(|id| sample.id.starts_with(id.as_str())) };
            (sample.wav.is_file() && wanted).then_some(sample)
        })
        .collect();
    samples.sort_by(|a, b| b.created_at.cmp(&a.created_at));
    samples.truncate(limit);
    samples
}

/// 表格单元格里不能有竖线和换行
fn cell(text: &str) -> String {
    text.replace('|', "\\|").replace('\n', " ")
}

#[tokio::test]
#[ignore = "真实调用豆包（计费），需要本机 data/ 里有录音与豆包密钥"]
async fn doubao_ab() {
    let data = data_dir();
    let limit = std::env::var("ATT_AB_LIMIT").ok().and_then(|v| v.parse().ok()).unwrap_or(40);
    let all = std::env::var("ATT_AB_ALL").is_ok_and(|v| v == "1");
    let ids: Vec<String> = std::env::var("ATT_AB_IDS")
        .unwrap_or_default()
        .split(',')
        .map(|id| id.trim().to_string())
        .filter(|id| !id.is_empty())
        .collect();
    let variants = variants();
    let labels = [variants[0].0, variants[1].0];

    let sandbox = std::env::temp_dir().join(format!("att-doubao-ab-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&sandbox);
    std::fs::create_dir_all(&sandbox).expect("创建临时目录");
    for name in ["settings.json", ".encryption_key"] {
        std::fs::copy(data.join(name), sandbox.join(name)).unwrap_or_else(|e| panic!("复制 {name} 失败：{e}"));
    }
    let core = CoreState::init(&sandbox, Platform::Macos).expect("初始化核心状态");
    let mut config = stt::doubao_config(&core);
    assert!(!config.api_key.is_empty(), "设置里没有豆包密钥，也没有 DOUBAO_ASR_API_KEY");

    let samples = collect_samples(&data, limit, all, &ids);
    assert!(!samples.is_empty(), "没有可对比的录音（{}）", data.display());
    println!("对比 {} 条录音：{:?}（热词 {:?}）\n", samples.len(), labels, variants[1].2);

    let mut report = format!("| 记录 | 当时原文 | {} | {} |\n| --- | --- | --- | --- |\n", labels[0], labels[1]);
    let mut differ = 0;
    for (index, sample) in samples.iter().enumerate() {
        let pcm = att_core::audio::read_audio_16k_mono(&sample.wav).expect("读取录音");
        let mut texts = Vec::new();
        for (_, language, hotwords) in &variants {
            config.hotwords = hotwords.clone();
            let text = match doubao::transcribe_pcm(&config, language, "", &pcm).await {
                Ok(outcome) => outcome.text,
                Err(error) => format!("【失败】{error}"),
            };
            texts.push(text);
        }
        let changed = texts[0] != texts[1];
        differ += usize::from(changed);
        println!(
            "[{}/{}] {} {}\n  原文 : {}\n  优化 : {}\n  {} : {}\n  {} : {}\n",
            index + 1,
            samples.len(),
            &sample.id[..8.min(sample.id.len())],
            if changed { "≠" } else { "=" },
            sample.raw_text,
            sample.optimized_text,
            labels[0],
            texts[0],
            labels[1],
            texts[1],
        );
        if changed {
            let _ = writeln!(
                report,
                "| {} | {} | {} | {} |",
                &sample.id[..8.min(sample.id.len())],
                cell(&sample.raw_text),
                cell(&texts[0]),
                cell(&texts[1])
            );
        }
    }
    println!("共 {} 条，两种结果不同的 {} 条", samples.len(), differ);

    if let Ok(out) = std::env::var("ATT_AB_OUT") {
        let header = format!("# 豆包 {} vs {}\n\n共 {} 条，结果不同的 {} 条（下表只列不同的）。\n\n", labels[0], labels[1], samples.len(), differ);
        std::fs::write(&out, header + &report).expect("写出对比报告");
        println!("报告：{out}");
    }
    let _ = std::fs::remove_dir_all(&sandbox);
}
