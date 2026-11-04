//! 提示词预设：内置预设（只读）+ 自定义预设（可增删改）。
//!
//! - **内置预设**：唯一真相源是 `prompts/*.txt`（打包为只读资源，升级会覆盖），移植自 Python
//!   `app/prompts/file_manager.py`。id = `preset-<文件名>`（与 Python 版一致，`settings.json` 两版通用）。
//!   内置预设不允许改写（只读资源，升级会覆盖），用户自己的提示词走自定义预设。
//! - **自定义预设**：存 `data/custom_prompts.json`（[`CUSTOM_PRESETS_FILE`]），写入一律走
//!   `json_store` 原子写；文件不存在 = 没有自定义预设。id = `custom-<uuid>`，与名称无关，改名不影响已选用处。
//! - 列表顺序：内置（按名称）在前，自定义（按创建顺序）在后。
//! - 默认回退「通用优化」，不存在时取第一个。

use crate::storage::json_store;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

pub const DEFAULT_PROMPT_NAME: &str = "通用优化";
/// 自定义预设文件名（位于数据目录下）
pub const CUSTOM_PRESETS_FILE: &str = "custom_prompts.json";
/// 自定义预设 id 前缀（内置为 `preset-`，两者不会撞）
pub const CUSTOM_ID_PREFIX: &str = "custom-";
/// 名称上限（字符数）
pub const MAX_NAME_CHARS: usize = 40;
/// 提示词正文上限（字符数）：远超任何正常预设，只防误粘贴整本书
pub const MAX_TEMPLATE_CHARS: usize = 20_000;
/// 自定义预设缺名称时的占位名
pub const UNNAMED_PRESET: &str = "未命名预设";

#[derive(Debug, Clone)]
pub struct Preset {
    pub id: String,
    pub name: String,
    pub template: String,
    pub is_builtin: bool,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
}

impl Preset {
    pub fn to_json(&self) -> Value {
        let description = if self.is_builtin {
            format!("来自 prompts/{}.txt", self.name)
        } else {
            "自定义预设".to_string()
        };
        json!({
            "id": self.id,
            "name": self.name,
            "description": description,
            "category": if self.is_builtin { "general" } else { "custom" },
            "prompt_template": self.template,
            "is_builtin": self.is_builtin,
            "created_at": self.created_at,
            "updated_at": self.updated_at,
        })
    }
}

pub fn preset_id(name: &str) -> String {
    format!("preset-{name}")
}

/// 预设目录：`ATT_PROMPTS_DIR` > 数据目录同级 `prompts/`（开发态仓库布局）> 安装资源目录。
pub fn resolve_dir(data_dir: &Path, resource_dir: Option<&Path>) -> PathBuf {
    if let Ok(dir) = std::env::var("ATT_PROMPTS_DIR") {
        return PathBuf::from(dir);
    }
    let candidates = [
        data_dir.parent().map(|p| p.join("prompts")),
        resource_dir.map(|p| p.join("prompts")),
    ];
    candidates
        .into_iter()
        .flatten()
        .find(|dir| dir.is_dir())
        .unwrap_or_else(|| PathBuf::from("prompts"))
}

/// 读取全部内置预设（空文件跳过），按名称排序。每次调用都读盘：改文件即时生效。
pub fn load_builtin(dir: &Path) -> Vec<Preset> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        tracing::warn!("提示词目录不存在: {}", dir.display());
        return Vec::new();
    };
    let mut presets: Vec<Preset> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("txt"))
        .filter_map(|path| {
            let name = path.file_stem()?.to_str()?.to_string();
            let template = std::fs::read_to_string(&path).ok()?.trim().to_string();
            (!template.is_empty()).then(|| Preset {
                id: preset_id(&name),
                name,
                template,
                is_builtin: true,
                created_at: None,
                updated_at: None,
            })
        })
        .collect();
    presets.sort_by(|a, b| a.name.cmp(&b.name));
    presets
}

/// 默认预设 id：「通用优化」优先，否则第一个。
pub fn default_id(presets: &[Preset]) -> Option<String> {
    let preferred = preset_id(DEFAULT_PROMPT_NAME);
    if presets.iter().any(|p| p.id == preferred) {
        return Some(preferred);
    }
    presets.first().map(|p| p.id.clone())
}

// ---------------- 自定义预设 ----------------

/// `custom_prompts.json` 里的一条；字段全部可缺省（缺字段兜底）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct CustomPreset {
    pub id: String,
    pub name: String,
    pub prompt_template: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct CustomPresetFile {
    pub presets: Vec<CustomPreset>,
}

impl CustomPreset {
    fn to_preset(&self) -> Preset {
        Preset {
            id: self.id.clone(),
            // 手改文件丢了名称时给个占位名，避免下拉框里出现空白项
            name: Some(self.name.trim()).filter(|n| !n.is_empty()).unwrap_or(UNNAMED_PRESET).to_string(),
            template: self.prompt_template.clone(),
            is_builtin: false,
            created_at: Some(self.created_at.clone()).filter(|s| !s.is_empty()),
            updated_at: Some(self.updated_at.clone()).filter(|s| !s.is_empty()),
        }
    }

    /// 读到的条目是否可用：id 合法且正文非空（手改坏的条目直接跳过，不让整个列表挂掉）。
    fn is_usable(&self) -> bool {
        self.id.starts_with(CUSTOM_ID_PREFIX) && !self.prompt_template.trim().is_empty()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum PresetError {
    #[error("{0}")]
    Invalid(String),
    #[error("内置预设只读，不能修改或删除")]
    Builtin,
    #[error("预设不存在")]
    NotFound,
    #[error("自定义预设文件损坏，为避免覆盖已拒绝写入：{0}")]
    Corrupt(String),
    #[error("保存自定义预设失败: {0}")]
    Io(#[from] std::io::Error),
}

/// 新建 / 修改的输入；None 表示该字段不改（仅修改时有意义）。
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct PresetDraft {
    pub name: Option<String>,
    pub prompt_template: Option<String>,
}

fn now_iso() -> String {
    chrono::Local::now().format("%Y-%m-%dT%H:%M:%S%.6f").to_string()
}

/// 预设库：内置目录 + 自定义文件。无内部状态，每次读盘（文件很小）。
#[derive(Debug, Clone)]
pub struct PromptLibrary {
    pub builtin_dir: PathBuf,
    pub custom_path: PathBuf,
}

impl PromptLibrary {
    pub fn new(builtin_dir: impl Into<PathBuf>, data_dir: &Path) -> Self {
        Self { builtin_dir: builtin_dir.into(), custom_path: data_dir.join(CUSTOM_PRESETS_FILE) }
    }

    /// 宽容读自定义预设（文件缺失 / 损坏 → 空列表；坏条目跳过）。
    pub fn load_custom(&self) -> Vec<Preset> {
        let file: CustomPresetFile = json_store::read_json_lenient(&self.custom_path);
        file.presets.iter().filter(|p| p.is_usable()).map(CustomPreset::to_preset).collect()
    }

    /// 全部预设：内置在前，自定义在后。
    pub fn load_all(&self) -> Vec<Preset> {
        let mut all = load_builtin(&self.builtin_dir);
        all.extend(self.load_custom());
        all
    }

    pub fn find(&self, id: &str) -> Option<Preset> {
        self.load_all().into_iter().find(|p| p.id == id)
    }

    /// 按候选 id 依次查找模板，都不存在时回退默认预设；全都没有返回 None（调用方用内置兜底）。
    pub fn resolve_template(&self, candidates: &[Option<&str>]) -> Option<String> {
        let presets = self.load_all();
        let by_id = |id: &str| presets.iter().find(|p| p.id == id);
        candidates
            .iter()
            .flatten()
            .find_map(|id| by_id(id))
            .or_else(|| default_id(&presets).and_then(|id| by_id(&id)))
            .map(|p| p.template.clone())
    }

    pub fn create(&self, draft: &PresetDraft) -> Result<Preset, PresetError> {
        let name = draft.name.as_deref().unwrap_or_default();
        let template = draft.prompt_template.as_deref().unwrap_or_default();
        self.mutate(|file, builtin| {
            let name = validate_name(name, builtin, &file.presets, None)?;
            let template = validate_template(template)?;
            let now = now_iso();
            let item = CustomPreset {
                id: format!("{CUSTOM_ID_PREFIX}{}", uuid::Uuid::new_v4().simple()),
                name,
                prompt_template: template,
                created_at: now.clone(),
                updated_at: now,
            };
            file.presets.push(item.clone());
            Ok(item.to_preset())
        })
    }

    pub fn update(&self, id: &str, draft: &PresetDraft) -> Result<Preset, PresetError> {
        if !id.starts_with(CUSTOM_ID_PREFIX) {
            return Err(self.builtin_or_missing(id));
        }
        self.mutate(|file, builtin| {
            let index = file.presets.iter().position(|p| p.id == id).ok_or(PresetError::NotFound)?;
            let name = match draft.name.as_deref() {
                Some(name) => Some(validate_name(name, builtin, &file.presets, Some(id))?),
                None => None,
            };
            let template = draft.prompt_template.as_deref().map(validate_template).transpose()?;
            let item = &mut file.presets[index];
            if let Some(name) = name {
                item.name = name;
            }
            if let Some(template) = template {
                item.prompt_template = template;
            }
            item.updated_at = now_iso();
            Ok(item.to_preset())
        })
    }

    pub fn delete(&self, id: &str) -> Result<(), PresetError> {
        if !id.starts_with(CUSTOM_ID_PREFIX) {
            return Err(self.builtin_or_missing(id));
        }
        self.mutate(|file, _| {
            let before = file.presets.len();
            file.presets.retain(|p| p.id != id);
            if file.presets.len() == before {
                return Err(PresetError::NotFound);
            }
            Ok(())
        })
    }

    fn builtin_or_missing(&self, id: &str) -> PresetError {
        if load_builtin(&self.builtin_dir).iter().any(|p| p.id == id) {
            PresetError::Builtin
        } else {
            PresetError::NotFound
        }
    }

    /// 锁内「严格读 → 改 → 原子写」。文件存在但解析失败时拒绝写入：宽容读回落空列表再写回会抹掉用户数据。
    fn mutate<T>(
        &self,
        f: impl FnOnce(&mut CustomPresetFile, &[Preset]) -> Result<T, PresetError>,
    ) -> Result<T, PresetError> {
        json_store::with_resource_lock(&self.custom_path, || {
            let mut file = if self.custom_path.exists() {
                json_store::read_json_file::<CustomPresetFile>(&self.custom_path)
                    .map_err(|e| PresetError::Corrupt(e.to_string()))?
            } else {
                CustomPresetFile::default()
            };
            let builtin = load_builtin(&self.builtin_dir);
            let outcome = f(&mut file, &builtin)?;
            json_store::write_json_atomic_locked(&self.custom_path, &file)?;
            Ok(outcome)
        })
    }
}

fn validate_name(
    name: &str,
    builtin: &[Preset],
    custom: &[CustomPreset],
    editing_id: Option<&str>,
) -> Result<String, PresetError> {
    let name = name.trim();
    if name.is_empty() {
        return Err(PresetError::Invalid("预设名称不能为空".into()));
    }
    if name.chars().count() > MAX_NAME_CHARS {
        return Err(PresetError::Invalid(format!("预设名称不能超过 {MAX_NAME_CHARS} 个字")));
    }
    let taken = builtin.iter().any(|p| p.name == name)
        || custom.iter().any(|p| p.name == name && Some(p.id.as_str()) != editing_id);
    if taken {
        return Err(PresetError::Invalid(format!("已有名为「{name}」的预设，换个名字吧")));
    }
    Ok(name.to_string())
}

fn validate_template(template: &str) -> Result<String, PresetError> {
    let template = template.trim();
    if template.is_empty() {
        return Err(PresetError::Invalid("提示词内容不能为空".into()));
    }
    if template.chars().count() > MAX_TEMPLATE_CHARS {
        return Err(PresetError::Invalid(format!("提示词内容不能超过 {MAX_TEMPLATE_CHARS} 个字")));
    }
    Ok(template.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root() -> PathBuf {
        let root = std::env::temp_dir().join(format!("att-prompts-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(root.join("prompts")).unwrap();
        std::fs::create_dir_all(root.join("data")).unwrap();
        std::fs::write(root.join("prompts/通用优化.txt"), "通用模板").unwrap();
        std::fs::write(root.join("prompts/程序员模式.txt"), "程序员模板 {text}").unwrap();
        root
    }

    fn draft(name: &str, template: &str) -> PresetDraft {
        PresetDraft { name: Some(name.into()), prompt_template: Some(template.into()) }
    }

    #[test]
    fn 预设读取_默认回退与空文件跳过() {
        let root = temp_root();
        std::fs::write(root.join("prompts/空.txt"), "  ").unwrap();
        let lib = PromptLibrary::new(root.join("prompts"), &root.join("data"));
        let presets = lib.load_all();
        assert_eq!(presets.len(), 2, "没有 custom_prompts.json 也能读，只有内置");
        assert_eq!(default_id(&presets).as_deref(), Some("preset-通用优化"));
        assert_eq!(lib.resolve_template(&[Some("preset-程序员模式")]).as_deref(), Some("程序员模板 {text}"));
        assert_eq!(lib.resolve_template(&[Some("preset-不存在")]).as_deref(), Some("通用模板"));
        // 候选链：第一个不存在时用第二个
        assert_eq!(
            lib.resolve_template(&[Some("custom-已删"), Some("preset-程序员模式")]).as_deref(),
            Some("程序员模板 {text}")
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn 自定义预设_增改删与校验() {
        let root = temp_root();
        let lib = PromptLibrary::new(root.join("prompts"), &root.join("data"));

        let created = lib.create(&draft("  周报  ", "  把口述整理成周报  ")).unwrap();
        assert!(created.id.starts_with(CUSTOM_ID_PREFIX));
        assert_eq!(created.name, "周报", "名称去首尾空白");
        assert_eq!(created.template, "把口述整理成周报");
        assert!(!created.is_builtin);
        let all = lib.load_all();
        assert_eq!(all.len(), 3);
        assert_eq!(all.last().unwrap().id, created.id, "自定义排在内置后面");
        assert_eq!(lib.resolve_template(&[Some(&created.id)]).as_deref(), Some("把口述整理成周报"));

        // 校验：空名 / 空正文 / 与内置重名 / 与其他自定义重名
        assert!(matches!(lib.create(&draft(" ", "x")), Err(PresetError::Invalid(_))));
        assert!(matches!(lib.create(&draft("a", "  ")), Err(PresetError::Invalid(_))));
        assert!(matches!(lib.create(&draft("通用优化", "x")), Err(PresetError::Invalid(_))));
        assert!(matches!(lib.create(&draft("周报", "x")), Err(PresetError::Invalid(_))));
        assert!(matches!(lib.create(&draft(&"长".repeat(MAX_NAME_CHARS + 1), "x")), Err(PresetError::Invalid(_))));

        // 修改：只改正文，名称不变；改成自己原名不算重名
        let updated = lib
            .update(&created.id, &PresetDraft { name: Some("周报".into()), prompt_template: Some("新正文".into()) })
            .unwrap();
        assert_eq!((updated.name.as_str(), updated.template.as_str()), ("周报", "新正文"));
        let renamed = lib.update(&created.id, &PresetDraft { name: Some("周报 v2".into()), prompt_template: None }).unwrap();
        assert_eq!((renamed.name.as_str(), renamed.template.as_str()), ("周报 v2", "新正文"));
        assert_eq!(renamed.id, created.id, "改名不改 id");

        // 内置只读
        assert!(matches!(lib.update("preset-通用优化", &draft("x", "y")), Err(PresetError::Builtin)));
        assert!(matches!(lib.delete("preset-通用优化"), Err(PresetError::Builtin)));
        assert!(matches!(lib.delete("preset-不存在"), Err(PresetError::NotFound)));

        // 删除
        lib.delete(&created.id).unwrap();
        assert!(matches!(lib.delete(&created.id), Err(PresetError::NotFound)));
        assert_eq!(lib.load_all().len(), 2);
        assert!(!root.join("data/custom_prompts.json.tmp").exists(), "原子写不留临时文件");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn 自定义预设_缺字段兜底与损坏文件拒写() {
        let root = temp_root();
        let lib = PromptLibrary::new(root.join("prompts"), &root.join("data"));
        // 缺字段 / 坏条目：能读的照读，坏的跳过
        std::fs::write(
            &lib.custom_path,
            r#"{"presets":[{"id":"custom-a","prompt_template":"只有正文"},{"id":"bad","prompt_template":"x"},{"id":"custom-b"}]}"#,
        )
        .unwrap();
        let custom = lib.load_custom();
        assert_eq!(custom.len(), 1);
        assert_eq!((custom[0].id.as_str(), custom[0].name.as_str()), ("custom-a", UNNAMED_PRESET));

        // 损坏：列表回落为空，但写操作拒绝，原文件保持不动
        std::fs::write(&lib.custom_path, "{ 半截").unwrap();
        assert!(lib.load_custom().is_empty());
        assert!(matches!(lib.create(&draft("新", "x")), Err(PresetError::Corrupt(_))));
        assert_eq!(std::fs::read_to_string(&lib.custom_path).unwrap(), "{ 半截");
        let _ = std::fs::remove_dir_all(&root);
    }
}
