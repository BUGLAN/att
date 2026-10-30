//! 提示词预设（只读，唯一真相源是 `prompts/*.txt`，移植自 Python `app/prompts/file_manager.py`）。
//!
//! 预设 id = `preset-<文件名>`（与 Python 版一致，`settings.json` 里的 `default_preset_id` 两版通用）；
//! 默认回退「通用优化」，不存在时取按名称排序的第一个。

use serde_json::{json, Value};
use std::path::{Path, PathBuf};

pub const DEFAULT_PROMPT_NAME: &str = "通用优化";

#[derive(Debug, Clone)]
pub struct Preset {
    pub id: String,
    pub name: String,
    pub template: String,
}

impl Preset {
    pub fn to_json(&self) -> Value {
        json!({
            "id": self.id,
            "name": self.name,
            "description": format!("来自 prompts/{}.txt", self.name),
            "category": "general",
            "prompt_template": self.template,
            "is_builtin": true,
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

/// 读取全部预设（空文件跳过），按名称排序。每次调用都读盘：改文件即时生效。
pub fn load_all(dir: &Path) -> Vec<Preset> {
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
            (!template.is_empty()).then(|| Preset { id: preset_id(&name), name, template })
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

/// 按 id 取模板；id 缺失或不存在时回退默认预设；全都没有返回 None（调用方用内置兜底）。
pub fn resolve_template(dir: &Path, preset_id: Option<&str>) -> Option<String> {
    let presets = load_all(dir);
    let chosen = preset_id
        .and_then(|id| presets.iter().find(|p| p.id == id))
        .or_else(|| default_id(&presets).and_then(|id| presets.iter().find(|p| p.id == id)))?;
    Some(chosen.template.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 预设读取_默认回退与空文件跳过() {
        let dir = std::env::temp_dir().join(format!("att-prompts-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("通用优化.txt"), "通用模板").unwrap();
        std::fs::write(dir.join("程序员模式.txt"), "程序员模板 {text}").unwrap();
        std::fs::write(dir.join("空.txt"), "  ").unwrap();
        let presets = load_all(&dir);
        assert_eq!(presets.len(), 2);
        assert_eq!(default_id(&presets).as_deref(), Some("preset-通用优化"));
        assert_eq!(resolve_template(&dir, Some("preset-程序员模式")).as_deref(), Some("程序员模板 {text}"));
        assert_eq!(resolve_template(&dir, Some("preset-不存在")).as_deref(), Some("通用模板"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
