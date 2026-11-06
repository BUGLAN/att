//! 一次性迁移（首启动执行，幂等，产出报告写入 `data/migration-report.json`）。
//!
//! 步骤（架构文档 §8.4）：
//! 1. settings.json：剥离退役键；旧 `hotkey` + 旧 Electron userData 的
//!    desktop-settings.json 合并为新版 `hotkeys[]`（desktop 端优先，它是旧桌面层真相源）
//! 2. 音频错位修复（阶段① 缺陷 #3）：record.audio_path 指向存在的文件 →
//!    重命名为 `{record_id}.<原扩展名>`（新录音 .flac / 旧录音 .wav）；悬空引用按时长启发式匹配未认领的 wav；
//!    仍未认领的移入 `audio/orphans/`（只隔离不删除）
//! 3. 遗留文件不迁移（wordbook/、data/prompts/presets.json 保持原样，无代码读取）

use crate::records::{is_safe_identifier, normalize_audio_rel, RecordRepo};
use crate::settings::{self, UserSettings};
use crate::storage::json_store;
use crate::Platform;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct MigrationReport {
    pub migrated_at: String,
    pub settings_normalized: bool,
    /// 热键来源："desktop-legacy"（旧 userData/desktop-settings.json）| "settings-legacy" | "existing" | "default"
    pub hotkey_source: String,
    /// 重命名为 {id}.wav 的文件数（原本就指向存在的文件）
    pub renamed_audio: Vec<String>,
    /// 按时长启发式匹配回的文件
    pub matched_audio: Vec<MatchedAudio>,
    /// 移入 audio/orphans/ 的未认领文件
    pub orphan_audio: Vec<String>,
    /// 仍指向不存在文件的记录（保持 audio_path 不动，仅报告）
    pub missing_audio: Vec<String>,
    pub legacy_desktop_settings_found: bool,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct MatchedAudio {
    pub record_id: String,
    pub wav_file: String,
    pub record_duration: f64,
    pub wav_duration: f64,
}

/// 执行迁移（探测本机旧 Electron 桌面设置后委托 [`run_with_legacy`]）。
///
/// 性能约定：首启动完成迁移后写 `data/.migration-3a.done` 标记，
/// 后续启动直接跳过（否则每次都要扫 900+ 记录与全部 wav，实测拖慢启动）。
/// 需要重跑时设置 `ATT_FORCE_MIGRATION=1`。
pub fn run(data_dir: &Path, platform: Platform) -> MigrationReport {
    let marker = data_dir.join(".migration-3a.done");
    let force = std::env::var("ATT_FORCE_MIGRATION").map(|v| v == "1").unwrap_or(false);
    if marker.exists() && !force {
        return MigrationReport {
            migrated_at: crate::now_rfc3339(),
            hotkey_source: "skipped".into(),
            ..Default::default()
        };
    }

    let legacy = find_legacy_desktop_settings();
    let report = run_with_legacy(data_dir, platform, legacy);
    // 迁移已完成（含无需改写的场景）：落标记避免重复全量扫描
    let _ = std::fs::write(&marker, "1");
    report
}

/// 可注入 legacy 来源的迁移入口（测试用，避免依赖本机真实安装痕迹）。
pub fn run_with_legacy(
    data_dir: &Path,
    platform: Platform,
    legacy_desktop: Option<serde_json::Value>,
) -> MigrationReport {
    let mut report = MigrationReport {
        migrated_at: crate::now_rfc3339(),
        ..Default::default()
    };

    // ---- 1. 设置归一化 + 热键合并 ----
    let settings_path = data_dir.join("settings.json");
    report.legacy_desktop_settings_found = legacy_desktop.is_some();

    json_store::with_resource_lock(&settings_path, || {
        let raw: serde_json::Value = json_store::read_json_lenient(&settings_path);
        let mut object = json_store::ensure_object(raw);
        let Some(map) = object.as_object_mut() else {
            return; // ensure_object 已保证是对象，此分支仅为类型收敛
        };
        let mut changed = false;

        // 剥离退役键
        for key in settings::RETIRED_SETTINGS_KEYS {
            if map.remove(*key).is_some() {
                changed = true;
            }
        }

        // 旧 hotkey → hotkeys[]（若新版不存在）
        let has_new = map
            .get("hotkeys")
            .and_then(|h| h.as_array())
            .is_some_and(|a| !a.is_empty());
        if !has_new {
            let legacy_value = legacy_desktop
                .as_ref()
                .and_then(|d| d.get("hotkey").cloned())
                .or_else(|| map.get("hotkey").cloned());
            let binding = match &legacy_value {
                Some(v) => settings::hotkey::binding_from_legacy(platform, v),
                None => settings::hotkey::HotkeyBinding::default_binding(platform),
            };
            report.hotkey_source = if legacy_desktop.as_ref().and_then(|d| d.get("hotkey")).is_some() {
                "desktop-legacy".into()
            } else if legacy_value.is_some() {
                "settings-legacy".into()
            } else {
                "default".into()
            };
            map.insert("hotkeys".into(), serde_json::to_value(vec![binding]).unwrap_or_default());
            changed = true;
        } else {
            report.hotkey_source = "existing".into();
        }
        map.remove("hotkey"); // 旧键退役

        if changed {
            // 合并默认值会补上最新修订号：先按旧修订号升级默认值，否则这次升级就被跳过了
            settings::upgrade_legacy_defaults(map);
            let merged = merge_defaults(std::mem::take(map), platform);
            if json_store::write_json_atomic_locked(&settings_path, &merged).is_ok() {
                report.settings_normalized = true;
            }
        }
    });

    // ---- 2. 音频错位修复 ----
    repair_audio_mismatch(data_dir, &mut report);

    // ---- 3. 报告落盘 ----
    let report_path = data_dir.join("migration-report.json");
    if let Err(error) = json_store::write_json_atomic(&report_path, &report) {
        tracing::error!("迁移报告写入失败: {error}");
    }
    report
}

/// 合并默认值后返回可持久化的完整对象。
fn merge_defaults(
    mut object: serde_json::Map<String, serde_json::Value>,
    platform: Platform,
) -> serde_json::Value {
    let defaults = serde_json::to_value(UserSettings::with_platform(platform))
        .unwrap_or_else(|_| serde_json::json!({}));
    if let Some(default_map) = defaults.as_object() {
        for (key, value) in default_map {
            object.entry(key.clone()).or_insert_with(|| value.clone());
        }
    }
    serde_json::Value::Object(object)
}

/// 探测旧 Electron userData 下的 desktop-settings.json（Electron app.name =
/// package.json name = "audio-to-text"；也顺带探测 productName 变体）。
fn find_legacy_desktop_settings() -> Option<serde_json::Value> {
    let config_dir = dirs::config_dir()?;
    for dir_name in ["audio-to-text", "Audio to Text"] {
        let path = config_dir.join(dir_name).join("desktop-settings.json");
        if path.is_file() {
            match json_store::read_json_file(&path) {
                Ok(value) => return Some(value),
                Err(error) => {
                    tracing::warn!("旧桌面设置损坏，忽略: {} ({error})", path.display());
                    return None;
                }
            }
        }
    }
    None
}

/// 音频错位修复（尽力而为，绝不删除数据）。
fn repair_audio_mismatch(data_dir: &Path, report: &mut MigrationReport) {
    let repo = RecordRepo::new(data_dir);
    if repo.ensure_dirs().is_err() {
        return;
    }
    let audio_dir = repo.audio_dir().to_path_buf();

    // 收集 wav：文件名 → 绝对路径
    let mut wav_files: Vec<(String, PathBuf)> = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&audio_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("wav") {
                if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                    wav_files.push((name.to_string(), path.clone()));
                }
            }
        }
    }

    // 载入全部记录（只读视角；set_audio_path 走仓库写路径）
    let (records, _) = repo.load_all();

    // pass 1：audio_path 指向存在的文件 → 重命名为 {id}.<原扩展名>
    // （新录音是 .flac，旧录音是 .wav；扩展名必须跟着文件内容走，不能一律改成 .wav）
    for record in &records {
        if !is_safe_identifier(&record.id) {
            continue;
        }
        if record.audio_path.is_empty() {
            continue;
        }
        let source = repo.resolve_audio_path(&record.audio_path);
        if source.is_file() {
            let extension = source.extension().and_then(|e| e.to_str()).unwrap_or("wav");
            let target_name = format!("{}.{extension}", record.id);
            if record.audio_path != target_name {
                let target = audio_dir.join(&target_name);
                if !target.exists() {
                    if std::fs::rename(&source, &target).is_ok() {
                        let _ = repo.set_audio_path(&record.id, &target_name);
                        report.renamed_audio.push(target_name);
                    }
                } else {
                    // 目标名被占：保守起见只挂到缺失清单，交给人工
                    report.missing_audio.push(record.id.clone());
                }
            }
        }
    }

    // pass 2：悬空 audio_path 的记录 ← 按时长匹配未认领 wav
    // （referenced 用归一化文件名比较：旧数据的 `data\audio\records\x.wav`
    //   引用的是 x.wav 这个文件，字符串原样比较会把它误判为未认领）
    let referenced: HashSet<String> = repo
        .load_all()
        .0
        .iter()
        .filter(|r| !r.audio_path.is_empty())
        .map(|r| normalize_audio_rel(&r.audio_path))
        .collect();
    let unclaimed: Vec<(String, PathBuf)> = wav_files
        .into_iter()
        .filter(|(name, _)| !referenced.contains(name) && !claimed_by_referenced_pending(&report, name))
        .collect();

    for record in repo.load_all().0 {
        let source_exists = !record.audio_path.is_empty()
            && repo.resolve_audio_path(&record.audio_path).is_file();
        if source_exists {
            continue;
        }
        if record.duration <= 0.0 {
            if !record.audio_path.is_empty() {
                report.missing_audio.push(record.id.clone());
            }
            continue;
        }
        // 找时长唯一匹配（±0.1s 容差，多候选视为不可判定，宁缺勿错）
        let candidates: Vec<String> = unclaimed
            .iter()
            .filter(|entry| {
                wav_duration_seconds(&entry.1)
                    .map(|d| (d - record.duration).abs() <= 0.1)
                    .unwrap_or(false)
            })
            .map(|entry| entry.0.clone())
            .collect();
        if candidates.len() == 1 {
            let name = candidates[0].clone();
            let target_name = format!("{}.wav", record.id);
            let target = audio_dir.join(&target_name);
            if !target.exists() {
                let wav_dur = wav_duration_seconds(&audio_dir.join(&name)).unwrap_or(record.duration);
                if std::fs::rename(audio_dir.join(&name), &target).is_ok() {
                    let _ = repo.set_audio_path(&record.id, &target_name);
                    report.matched_audio.push(MatchedAudio {
                        record_id: record.id.clone(),
                        wav_file: name,
                        record_duration: record.duration,
                        wav_duration: wav_dur,
                    });
                }
            }
        } else if !record.audio_path.is_empty() {
            report.missing_audio.push(record.id.clone());
        }
    }

    // pass 3：仍未认领的移入 orphans/（隔离不删除；referenced 同样用归一化名）
    let orphan_dir = data_dir.join("audio").join("orphans");
    let referenced_now: HashSet<String> = repo
        .load_all()
        .0
        .iter()
        .filter(|r| !r.audio_path.is_empty())
        .map(|r| normalize_audio_rel(&r.audio_path))
        .collect();
    if let Ok(entries) = std::fs::read_dir(&audio_dir) {
        let _ = std::fs::create_dir_all(&orphan_dir);
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("wav") {
                continue;
            }
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            if !referenced_now.contains(name) && !name.ends_with(".tmp") {
                let target = orphan_dir.join(name);
                if !target.exists() && std::fs::rename(&path, &target).is_ok() {
                    report.orphan_audio.push(name.to_string());
                }
            }
        }
    }
}

/// pass 2 内部：本轮已处理的文件不再参与匹配。
fn claimed_by_referenced_pending(report: &MigrationReport, name: &str) -> bool {
    report.renamed_audio.iter().any(|n| n == name) || report.matched_audio.iter().any(|m| &m.wav_file == name)
}

/// WAV 时长（秒）：hound 读头即可，不载入采样。
fn wav_duration_seconds(path: &Path) -> Option<f64> {
    let reader = hound::WavReader::open(path).ok()?;
    let spec = reader.spec();
    let frames: u64 = reader.duration() as u64;
    Some(frames as f64 / spec.sample_rate as f64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn temp_data(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("att-migration-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// 生成一个最小合法 wav（hound 写入 N 秒静音 16k mono）。
    fn write_wav(path: &Path, seconds: f64) {
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 16_000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::create(path, spec).unwrap();
        let frames = (seconds * 16_000.0) as u32;
        for _ in 0..frames {
            writer.write_sample(0i16).unwrap();
        }
        writer.finalize().unwrap();
    }

    #[test]
    fn 首次迁移_默认设置与热键生成且幂等() {
        let dir = temp_data("fresh");
        let report = run_with_legacy(&dir, Platform::Windows, None);
        assert!(report.settings_normalized);
        assert_eq!(report.hotkey_source, "default");

        // 二次运行：不再重复迁移（existing）
        let report2 = run_with_legacy(&dir, Platform::Windows, None);
        assert_eq!(report2.hotkey_source, "existing");
        assert!(dir.join("migration-report.json").is_file());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn 旧热键与退役键迁移() {
        let dir = temp_data("legacy");
        std::fs::write(
            dir.join("settings.json"),
            json!({
                "asr_provider": "doubao_asr",
                "selection_toolbar_enabled": true,
                "hotkey": {"modifiers": ["ctrl", "shift"], "key": "r", "display": "Ctrl+Shift+R"}
            }).to_string(),
        ).unwrap();
        // 注入一个无 hotkey 键的 legacy 桌面设置（模拟旧机但热键真相在 settings.json）
        let report = run_with_legacy(&dir, Platform::Linux, Some(json!({"pause_audio_on_recording": true})));
        assert_eq!(report.hotkey_source, "settings-legacy");

        let raw = std::fs::read_to_string(dir.join("settings.json")).unwrap();
        let value: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert!(value.get("hotkey").is_none(), "旧 hotkey 键必须退役");
        assert!(value["hotkeys"].as_array().unwrap().len() == 1);
        assert!(value.get("selection_toolbar_enabled").is_none(), "退役键必须剥离");
        assert_eq!(value["asr_provider"], json!("doubao_asr"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn 音频错位修复_重命名_时长匹配_孤儿隔离() {
        let dir = temp_data("audio");
        let repo = RecordRepo::new(&dir);
        repo.ensure_dirs().unwrap();
        let audio = repo.audio_dir();

        // 记录 A：audio_path 指向存在但名字错位的文件（uuid.wav）
        write_wav(&audio.join("uuid-a.wav"), 2.0);
        repo.insert(crate::records::Record {
            id: "rec-a".into(),
            audio_path: "uuid-a.wav".into(),
            duration: 2.0,
            ..Default::default()
        }).unwrap();

        // 记录 B：audio_path 悬空，但目录里有一个 3.05s 的未认领 wav
        write_wav(&audio.join("uuid-b.wav"), 3.05);
        repo.insert(crate::records::Record {
            id: "rec-b".into(),
            audio_path: "gone.wav".into(),
            duration: 3.0,
            ..Default::default()
        }).unwrap();

        // 纯孤儿：无任何记录引用
        write_wav(&audio.join("orphan-1.wav"), 9.0);

        let report = run_with_legacy(&dir, Platform::Windows, None);
        assert!(report.renamed_audio.contains(&"rec-a.wav".to_string()), "改名: {:?}", report.renamed_audio);
        assert_eq!(report.matched_audio.len(), 1, "时长匹配: {:?}", report.matched_audio);
        assert_eq!(report.matched_audio[0].record_id, "rec-b");
        assert!(report.orphan_audio.contains(&"orphan-1.wav".to_string()), "孤儿隔离: {:?}", report.orphan_audio);

        // 验证记录指向已修正
        assert_eq!(repo.get("rec-a").unwrap().audio_path, "rec-a.wav");
        assert_eq!(repo.get("rec-b").unwrap().audio_path, "rec-b.wav");
        assert!(dir.join("audio").join("orphans").join("orphan-1.wav").is_file());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 回归锁：Python 版带 `data\audio\records\` 前缀的 audio_path 必须被
    /// pass-1 按精确路径认领（旧版直接 join 拼错路径，导致 886 个 wav 被整体
    /// 误隔离进 orphans/、801 条记录标 missing 的数据事故）。
    #[test]
    fn 音频错位修复_带历史前缀路径_精确认领不误隔离() {
        let dir = temp_data("audio-prefix");
        let repo = RecordRepo::new(&dir);
        repo.ensure_dirs().unwrap();
        let audio = repo.audio_dir();

        // 记录 C：audio_path 是 Python 版形态（带 data\ 前缀 + 反斜杠）
        write_wav(&audio.join("uuid-c.wav"), 1.5);
        repo.insert(crate::records::Record {
            id: "rec-c".into(),
            audio_path: "data\\audio\\records\\uuid-c.wav".into(),
            duration: 1.5,
            ..Default::default()
        }).unwrap();

        let report = run_with_legacy(&dir, Platform::Windows, None);
        assert!(
            report.renamed_audio.contains(&"rec-c.wav".to_string()),
            "带前缀路径必须按精确路径认领: renamed={:?} orphaned={:?}",
            report.renamed_audio,
            report.orphan_audio
        );
        assert!(report.orphan_audio.is_empty(), "被引用的 wav 不得进 orphans: {:?}", report.orphan_audio);
        assert_eq!(repo.get("rec-c").unwrap().audio_path, "rec-c.wav");
        assert!(audio.join("rec-c.wav").is_file(), "文件应已改名为 rec-c.wav");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 新录音存 FLAC：pass-1 改名必须保留 .flac 扩展名，也不得把它当孤儿隔离。
    #[test]
    fn 音频错位修复_flac录音保留扩展名() {
        let dir = temp_data("audio-flac");
        let repo = RecordRepo::new(&dir);
        repo.ensure_dirs().unwrap();
        let audio = repo.audio_dir();

        crate::audio::write_recording_16k(&audio.join("uuid-d.flac"), &[0i16; 1600]).unwrap();
        repo.insert(crate::records::Record {
            id: "rec-d".into(),
            audio_path: "uuid-d.flac".into(),
            duration: 0.1,
            ..Default::default()
        }).unwrap();

        let report = run_with_legacy(&dir, Platform::Windows, None);
        assert!(report.renamed_audio.contains(&"rec-d.flac".to_string()), "改名: {:?}", report.renamed_audio);
        assert!(report.orphan_audio.is_empty(), "被引用的 flac 不得进 orphans: {:?}", report.orphan_audio);
        assert_eq!(repo.get("rec-d").unwrap().audio_path, "rec-d.flac");
        assert_eq!(crate::audio::read_audio_16k_mono(&audio.join("rec-d.flac")).unwrap().len(), 1600);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
