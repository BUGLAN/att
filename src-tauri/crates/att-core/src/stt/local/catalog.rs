//! 本地模型清单与安装状态：设置页的「下载 / 进度 / 删除」与 `/api/models` 都从这里取事实。
//!
//! 地址与 SHA-256 逐个写死并核对过（Hugging Face 的 LFS 元数据给出 `sha256`，非 LFS 小文件
//! 下载后实测；GitHub Releases 的 asset 摘要来自发布页的 `sha256:` 标记），不在运行期拼接
//! `Systran/faster-whisper-{短名}`：`large-v3-turbo` 不在 Systran 名下，拼接会 404。
//!
//! 安装布局与引擎的探测顺序对齐：
//! - Whisper：`models/faster-whisper-<短名>/`（旧的单模型目录 `models/faster_whisper/` 仍识别为 large-v3）；
//! - Qwen3-ASR：GitHub 的 `.tar.bz2` 解压后取模型目录，落到 `models/qwen3-asr-0.6B-int8/`；
//! - Silero VAD：单文件 `models/silero_vad.onnx`（sherpa-onnx 子目录里的旧位置也识别）。

use serde::Serialize;
use std::path::{Path, PathBuf};

/// 本地引擎种类；`Vad` 不是识别引擎，但同样由设置页下载和管理。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Engine {
    Whisper,
    Qwen,
    Vad,
}

impl Engine {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Whisper => "whisper",
            Self::Qwen => "qwen_asr",
            Self::Vad => "vad",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Whisper => "faster-whisper",
            Self::Qwen => "Qwen3-ASR",
            Self::Vad => "Silero VAD",
        }
    }
}

/// 模型下载源：`official` 走 Hugging Face / GitHub Releases；`mirror` 把 Hugging Face
/// 地址换成同路径的 `hf-mirror.com`（没有 HF 地址的项仍走 GitHub，界面标「仅官方源」）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DownloadSource {
    Official,
    Mirror,
}

impl DownloadSource {
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "official" => Some(Self::Official),
            "mirror" => Some(Self::Mirror),
            _ => None,
        }
    }

    pub fn from_setting(value: &str) -> Self {
        Self::parse(value).unwrap_or(Self::Official)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Official => "official",
            Self::Mirror => "mirror",
        }
    }
}

/// 单个下载文件。`url` 为空时按模型仓库的 Hugging Face 地址拼；
/// 有值时是 GitHub Releases 的完整地址（`mirror` 模式下同样走官方）。
#[derive(Debug, Clone, Copy)]
pub struct FileSpec {
    /// 安装目录内的相对路径。
    pub path: &'static str,
    pub size: u64,
    pub sha256: &'static str,
    pub url: Option<&'static str>,
}

/// 一项可安装的模型。
#[derive(Debug, Clone, Copy)]
pub struct ModelSpec {
    pub id: &'static str,
    pub engine: Engine,
    /// 短名（Whisper 的 `large-v3-turbo`、Qwen 的目录名）。
    pub name: &'static str,
    pub title: &'static str,
    pub description: &'static str,
    /// 安装后写回哪个设置键（Whisper 是 `whisper_model`，Qwen 是 `qwen_asr_model`）。
    pub config_key: &'static str,
    /// 设置里的取值（Whisper 为短名；Qwen 为目录名）。
    pub select_value: &'static str,
    /// 安装目标：目录名或文件名（相对模型根目录）。
    pub target: &'static str,
    /// 目标是单文件（VAD），不是目录。
    pub single_file: bool,
    /// 判定「已安装」的最小文件集（目录内相对路径）。
    pub required: &'static [&'static str],
    /// 判定「已安装」还需要的子目录（Qwen 的 `tokenizer/`）。
    pub required_dir: Option<&'static str>,
    /// 是否是 `.tar.bz2` 压缩包（Qwen）：下载后解压，取压缩包内的模型目录。
    pub package: bool,
    /// Hugging Face 仓库（`owner/name`）；没有时为纯 GitHub 项。
    pub repo: Option<&'static str>,
    /// Hugging Face 固定修订；哈希清单与这个提交绑定，仓库后续更新不会让校验无故失效。
    pub revision: Option<&'static str>,
    pub files: &'static [FileSpec],
}

impl ModelSpec {
    /// 下载地址：HF 项按仓库 + 相对路径拼，镜像换域名；GitHub 项原样返回。
    pub fn file_url(&self, file: &FileSpec, source: DownloadSource) -> String {
        if let Some(url) = file.url {
            return url.to_string();
        }
        let repo = self.repo.unwrap_or_default();
        let host = match source {
            DownloadSource::Official => "huggingface.co",
            DownloadSource::Mirror => "hf-mirror.com",
        };
        let revision = self.revision.unwrap_or("main");
        format!("https://{host}/{repo}/resolve/{revision}/{}", file.path)
    }

    /// 镜像模式下是否仍全部走官方地址（GitHub 项）。
    pub fn mirror_supported(&self) -> bool {
        self.repo.is_some()
    }

    pub fn download_size(&self) -> u64 {
        self.files.iter().map(|file| file.size).sum()
    }

    /// 安装目标路径。
    pub fn target_path(&self, models_dir: &Path) -> PathBuf {
        models_dir.join(self.target)
    }
}

const TINY_FILES: &[FileSpec] = &[
    FileSpec {
        path: "config.json",
        size: 2_249,
        sha256: "a73a28cdfe1c43ccc7202fa333d1f89c202477271407ae9a7f19afa52039cac8",
        url: None,
    },
    FileSpec {
        path: "model.bin",
        size: 75_538_270,
        sha256: "dcb76c6586fc06cbdac6dd21f14cfd129cc4cdd9dce19bf4ffa62e59cbe6e6d1",
        url: None,
    },
    FileSpec {
        path: "tokenizer.json",
        size: 2_203_239,
        sha256: "fb7b63191e9bb045082c79fd742a3106a12c99513ab30df4a0d47fa6cb6fd0ab",
        url: None,
    },
    FileSpec {
        path: "vocabulary.txt",
        size: 459_861,
        sha256: "34ce3fe1c5041027b3f8d42912270993f986dbc4bb34cf27f951e34a1e453913",
        url: None,
    },
];

const BASE_FILES: &[FileSpec] = &[
    FileSpec {
        path: "config.json",
        size: 2_309,
        sha256: "56a6d8110d311f19c8f0471e562832c7527f146b567275bfca59fcf7c184da9a",
        url: None,
    },
    FileSpec {
        path: "model.bin",
        size: 145_217_532,
        sha256: "d01c3014881c9c6f3133c182f3d2887eb6ca1c789a7538c5c007196857a0a6a9",
        url: None,
    },
    FileSpec {
        path: "tokenizer.json",
        size: 2_203_239,
        sha256: "fb7b63191e9bb045082c79fd742a3106a12c99513ab30df4a0d47fa6cb6fd0ab",
        url: None,
    },
    FileSpec {
        path: "vocabulary.txt",
        size: 459_861,
        sha256: "34ce3fe1c5041027b3f8d42912270993f986dbc4bb34cf27f951e34a1e453913",
        url: None,
    },
];

const SMALL_FILES: &[FileSpec] = &[
    FileSpec {
        path: "config.json",
        size: 2_370,
        sha256: "b55496ac7940a7ae47d2c01eab40edfd8701feec1229d9cce3b40014383fb828",
        url: None,
    },
    FileSpec {
        path: "model.bin",
        size: 483_546_902,
        sha256: "3e305921506d8872816023e4c273e75d2419fb89b24da97b4fe7bce14170d671",
        url: None,
    },
    FileSpec {
        path: "tokenizer.json",
        size: 2_203_239,
        sha256: "fb7b63191e9bb045082c79fd742a3106a12c99513ab30df4a0d47fa6cb6fd0ab",
        url: None,
    },
    FileSpec {
        path: "vocabulary.txt",
        size: 459_861,
        sha256: "34ce3fe1c5041027b3f8d42912270993f986dbc4bb34cf27f951e34a1e453913",
        url: None,
    },
];

const MEDIUM_FILES: &[FileSpec] = &[
    FileSpec {
        path: "config.json",
        size: 2_257,
        sha256: "3622a2ddc41ec0e0fd4e68c13c6830f03b90c38d89aaad184de02c8c642cf807",
        url: None,
    },
    FileSpec {
        path: "model.bin",
        size: 1_527_906_378,
        sha256: "9b45e1009dcc4ab601eff815b61d80e60ce3fd8c74c1a14f4a282258286b51ae",
        url: None,
    },
    FileSpec {
        path: "tokenizer.json",
        size: 2_203_239,
        sha256: "fb7b63191e9bb045082c79fd742a3106a12c99513ab30df4a0d47fa6cb6fd0ab",
        url: None,
    },
    FileSpec {
        path: "vocabulary.txt",
        size: 459_861,
        sha256: "34ce3fe1c5041027b3f8d42912270993f986dbc4bb34cf27f951e34a1e453913",
        url: None,
    },
];

const LARGE_V3_FILES: &[FileSpec] = &[
    FileSpec {
        path: "config.json",
        size: 2_394,
        sha256: "a9306624f5ec14270a014b647e5c316b6e03a662c369758d1b90697a7b0655b9",
        url: None,
    },
    FileSpec {
        path: "model.bin",
        size: 3_087_284_237,
        sha256: "69f74147e3334731bc3a76048724833325d2ec74642fb52620eda87352e3d4f1",
        url: None,
    },
    FileSpec {
        path: "preprocessor_config.json",
        size: 340,
        sha256: "7ccc62c6f2765af1f3b46c00c9b5894426835a05021c8b9c01eecb6dfb542711",
        url: None,
    },
    FileSpec {
        path: "tokenizer.json",
        size: 2_480_617,
        sha256: "6d8cbd7cd0d8d5815e478dac67b85a26bbe77c1f5e0c6d76d1ce2abc0e5f21ca",
        url: None,
    },
    FileSpec {
        path: "vocabulary.json",
        size: 1_068_114,
        sha256: "c69260f2ab26d659b7c398f9a2b2b48ed0df16c3b47d7326782fd9cba71690c1",
        url: None,
    },
];

const LARGE_V3_TURBO_FILES: &[FileSpec] = &[
    FileSpec {
        path: "config.json",
        size: 2_263,
        sha256: "b0253ea6c0d3bea6b1e19e91a02acfd3b53f4467362efcb5a3e6b16c9b3a9b7e",
        url: None,
    },
    FileSpec {
        path: "model.bin",
        size: 1_617_884_929,
        sha256: "e76620f83d5f5b69efd3d87e3dc180c1bd21df9fbebacfd4335e5e1efcc018da",
        url: None,
    },
    FileSpec {
        path: "preprocessor_config.json",
        size: 340,
        sha256: "7ccc62c6f2765af1f3b46c00c9b5894426835a05021c8b9c01eecb6dfb542711",
        url: None,
    },
    FileSpec {
        path: "tokenizer.json",
        size: 2_710_337,
        sha256: "297b13372ac43916285644fb9687add3cc62ee2a1adb60da3dc25cc94c1871fd",
        url: None,
    },
    FileSpec {
        path: "vocabulary.json",
        size: 1_068_114,
        sha256: "c69260f2ab26d659b7c398f9a2b2b48ed0df16c3b47d7326782fd9cba71690c1",
        url: None,
    },
];

/// Qwen3-ASR 的 GitHub Release 资产（878 MB；k2-fsa 没有在 Hugging Face 上传同一份文件）。
const QWEN_FILES: &[FileSpec] = &[FileSpec {
    path: "sherpa-onnx-qwen3-asr-0.6B-int8-2026-03-25.tar.bz2",
    size: 878_702_423,
    sha256: "393f8a14e2f5fb96746aaab342997a40641001fbd5bf9592a080a8329178ee96",
    url: Some("https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/sherpa-onnx-qwen3-asr-0.6B-int8-2026-03-25.tar.bz2"),
}];

const VAD_FILES: &[FileSpec] = &[FileSpec {
    path: "silero_vad.onnx",
    size: 643_854,
    sha256: "9e2449e1087496d8d4caba907f23e0bd3f78d91fa552479bb9c23ac09cbb1fd6",
    url: Some("https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/silero_vad.onnx"),
}];

/// 首批可安装项：Whisper 六档 + Qwen3-ASR + Silero VAD。
pub static MODELS: &[ModelSpec] = &[
    ModelSpec {
        id: "whisper-tiny",
        engine: Engine::Whisper,
        name: "tiny",
        title: "Whisper tiny",
        description: "最快、最省内存，准确率最低；适合试听和低配机器。",
        config_key: "whisper_model",
        select_value: "tiny",
        target: "faster-whisper-tiny",
        single_file: false,
        required: &["model.bin", "tokenizer.json"],
        required_dir: None,
        package: false,
        repo: Some("Systran/faster-whisper-tiny"),
        revision: Some("d90ca5fe260221311c53c58e660288d3deb8d356"),
        files: TINY_FILES,
    },
    ModelSpec {
        id: "whisper-base",
        engine: Engine::Whisper,
        name: "base",
        title: "Whisper base",
        description: "比 tiny 准，CPU 上仍然很快。",
        config_key: "whisper_model",
        select_value: "base",
        target: "faster-whisper-base",
        single_file: false,
        required: &["model.bin", "tokenizer.json"],
        required_dir: None,
        package: false,
        repo: Some("Systran/faster-whisper-base"),
        revision: Some("ebe41f70d5b6dfa9166e2c581c45c9c0cfc57b66"),
        files: BASE_FILES,
    },
    ModelSpec {
        id: "whisper-small",
        engine: Engine::Whisper,
        name: "small",
        title: "Whisper small",
        description: "速度和准确率的折中，日常录音够用。",
        config_key: "whisper_model",
        select_value: "small",
        target: "faster-whisper-small",
        single_file: false,
        required: &["model.bin", "tokenizer.json"],
        required_dir: None,
        package: false,
        repo: Some("Systran/faster-whisper-small"),
        revision: Some("536b0662742c02347bc0e980a01041f333bce120"),
        files: SMALL_FILES,
    },
    ModelSpec {
        id: "whisper-medium",
        engine: Engine::Whisper,
        name: "medium",
        title: "Whisper medium",
        description: "更准，CPU 上明显变慢。",
        config_key: "whisper_model",
        select_value: "medium",
        target: "faster-whisper-medium",
        single_file: false,
        required: &["model.bin", "tokenizer.json"],
        required_dir: None,
        package: false,
        repo: Some("Systran/faster-whisper-medium"),
        revision: Some("08e178d48790749d25932bbc082711ddcfdfbc4f"),
        files: MEDIUM_FILES,
    },
    ModelSpec {
        id: "whisper-large-v3",
        engine: Engine::Whisper,
        name: "large-v3",
        title: "Whisper large-v3",
        description: "准确率最高，约 3 GB；CPU 上最慢。",
        config_key: "whisper_model",
        select_value: "large-v3",
        target: "faster-whisper-large-v3",
        single_file: false,
        required: &["model.bin", "tokenizer.json"],
        required_dir: None,
        package: false,
        repo: Some("Systran/faster-whisper-large-v3"),
        revision: Some("edaa852ec7e145841d8ffdb056a99866b5f0a478"),
        files: LARGE_V3_FILES,
    },
    ModelSpec {
        id: "whisper-large-v3-turbo",
        engine: Engine::Whisper,
        name: "large-v3-turbo",
        title: "Whisper large-v3-turbo",
        description: "接近 large-v3 的准确率，CPU 上快得多，推荐。",
        config_key: "whisper_model",
        select_value: "large-v3-turbo",
        target: "faster-whisper-large-v3-turbo",
        single_file: false,
        required: &["model.bin", "tokenizer.json"],
        required_dir: None,
        package: false,
        repo: Some("deepdml/faster-whisper-large-v3-turbo-ct2"),
        revision: Some("4df90f75321148c3a29a9e2351b7ddf8f5b115a8"),
        files: LARGE_V3_TURBO_FILES,
    },
    ModelSpec {
        id: "qwen3-asr-0.6b-int8",
        engine: Engine::Qwen,
        name: "qwen3-asr-0.6B-int8",
        title: "Qwen3-ASR 0.6B int8",
        description: "sherpa-onnx 的 ONNX 版本，约 940 MB；需要先有 sherpa-onnx 推理库。",
        config_key: "qwen_asr_model",
        select_value: "qwen3-asr-0.6B-int8",
        target: "qwen3-asr-0.6B-int8",
        single_file: false,
        required: &["conv_frontend.onnx", "encoder.int8.onnx", "decoder.int8.onnx"],
        required_dir: Some("tokenizer"),
        package: true,
        repo: None,
        revision: None,
        files: QWEN_FILES,
    },
    ModelSpec {
        id: "silero-vad",
        engine: Engine::Vad,
        name: "silero_vad",
        title: "Silero VAD",
        description: "Whisper 的噪声过滤前置；缺了它，噪声/音乐素材上可能出现整段幻觉文本。",
        config_key: "",
        select_value: "",
        target: "silero_vad.onnx",
        single_file: true,
        required: &["silero_vad.onnx"],
        required_dir: None,
        package: false,
        repo: None,
        revision: None,
        files: VAD_FILES,
    },
];

pub fn find(id: &str) -> Option<&'static ModelSpec> {
    MODELS.iter().find(|spec| spec.id == id)
}

pub fn whisper_specs() -> impl Iterator<Item = &'static ModelSpec> {
    MODELS.iter().filter(|spec| spec.engine == Engine::Whisper)
}

pub fn vad_spec() -> &'static ModelSpec {
    find("silero-vad").expect("模型清单必须包含 Silero VAD")
}

/// 目录/文件是否存在「已安装」所需的最小文件集。
pub fn is_installed_at(spec: &ModelSpec, path: &Path) -> bool {
    if spec.single_file {
        return path.is_file();
    }
    if !path.is_dir() {
        return false;
    }
    if !spec.required.iter().all(|name| path.join(name).is_file()) {
        return false;
    }
    spec.required_dir.map(|name| path.join(name).is_dir()).unwrap_or(true)
}

/// 旧版单模型目录（Python 版 `download_root=faster_whisper`）：只有 large-v3 会落到这里。
pub fn legacy_whisper_dir(models_dir: &Path) -> PathBuf {
    models_dir.join("faster_whisper")
}

/// 一项模型所有可能的落盘位置（含旧布局）；只返回实际存在的路径。
pub fn existing_paths(spec: &ModelSpec, models_dir: &Path) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if spec.single_file {
        for candidate in [models_dir.join(spec.target), models_dir.join("sherpa-onnx").join(spec.target)] {
            push_existing(&mut paths, candidate, Path::is_file);
        }
        if spec.engine == Engine::Vad {
            push_existing(&mut paths, models_dir.join(".downloads").join("_vad"), Path::exists);
        }
        return paths;
    }

    push_existing(&mut paths, spec.target_path(models_dir), Path::exists);
    match spec.engine {
        Engine::Whisper => {
            let short = spec.select_value;
            for root in [
                models_dir.to_path_buf(),
                models_dir.join("faster_whisper"),
                models_dir.join("faster-whisper"),
                models_dir.join("whisper"),
            ] {
                for name in
                    [short.to_string(), format!("faster-whisper-{short}"), format!("Systran--faster-whisper-{short}")]
                {
                    push_existing(&mut paths, root.join(name), Path::exists);
                }
                let snapshots = root.join(format!("models--Systran--faster-whisper-{short}")).join("snapshots");
                if let Ok(entries) = std::fs::read_dir(snapshots) {
                    for path in entries.flatten().map(|entry| entry.path()).filter(|path| path.is_dir()) {
                        push_existing(&mut paths, path, Path::exists);
                    }
                }
            }
            if spec.id == "whisper-large-v3" {
                push_existing(&mut paths, legacy_whisper_dir(models_dir), Path::exists);
            }
        }
        Engine::Qwen => {
            let short = spec.select_value;
            for name in [
                short.to_string(),
                short.replace('.', "___"),
                "qwen3-asr-0.6B-int8".into(),
                "qwen3_asr_onnx".into(),
                "qwen3-asr".into(),
            ] {
                push_existing(&mut paths, models_dir.join(name), Path::exists);
            }
        }
        Engine::Vad => {}
    }
    push_existing(&mut paths, models_dir.join(".downloads").join(spec.id), Path::exists);
    paths
}

fn push_existing(paths: &mut Vec<PathBuf>, path: PathBuf, predicate: impl FnOnce(&Path) -> bool) {
    if predicate(&path) && !paths.contains(&path) {
        paths.push(path);
    }
}

#[derive(Debug, Clone)]
pub struct ModelStatus {
    /// `installed` / `partial` / `missing`
    pub state: &'static str,
    pub installed_bytes: u64,
    /// 实际存在的路径（删除时逐个删）。
    pub paths: Vec<PathBuf>,
    /// 优先展示/使用的路径。
    pub primary: Option<PathBuf>,
}

pub fn status(spec: &ModelSpec, models_dir: &Path) -> ModelStatus {
    let paths = existing_paths(spec, models_dir);
    let installed_bytes = paths.iter().map(|path| size_of(path)).sum();
    let installed = paths.iter().find(|path| is_installed_at(spec, path)).cloned();
    let state = if installed.is_some() {
        "installed"
    } else if paths.is_empty() {
        "missing"
    } else {
        "partial"
    };
    ModelStatus { state, installed_bytes, primary: installed.or_else(|| paths.first().cloned()), paths }
}

/// 路径占用字节：目录递归累加普通文件。
pub fn size_of(path: &Path) -> u64 {
    let Ok(metadata) = std::fs::symlink_metadata(path) else {
        return 0;
    };
    if metadata.is_file() {
        return metadata.len();
    }
    if !metadata.is_dir() {
        return 0;
    }
    let Ok(entries) = std::fs::read_dir(path) else {
        return 0;
    };
    entries.flatten().map(|entry| size_of(&entry.path())).fold(0u64, |total, size| total.saturating_add(size))
}

/// Silero VAD 是否就位（`/api/health` 的 `local_engine.vad_ready`）。
pub fn vad_installed(models_dir: &Path) -> bool {
    [models_dir.join("silero_vad.onnx"), models_dir.join("sherpa-onnx").join("silero_vad.onnx")]
        .iter()
        .any(|path| path.is_file())
}

/// 在解压目录里找真正的模型目录（压缩包通常带一层 `<包名>/`）。
pub fn find_packaged_dir(root: &Path, required: &[&str], required_dir: Option<&str>) -> Option<PathBuf> {
    let matches = |path: &Path| {
        path.is_dir()
            && required.iter().all(|name| path.join(name).is_file())
            && required_dir.map(|name| path.join(name).is_dir()).unwrap_or(true)
    };
    if matches(root) {
        return Some(root.to_path_buf());
    }
    let mut children: Vec<PathBuf> =
        std::fs::read_dir(root).ok()?.flatten().map(|entry| entry.path()).filter(|path| path.is_dir()).collect();
    children.sort();
    children.into_iter().find(|path| matches(path))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("att-model-catalog-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn 清单_地址与哈希格式正确_镜像只影响_hf项() {
        assert!(MODELS.len() >= 8);
        for spec in MODELS {
            assert!(!spec.files.is_empty(), "{}", spec.id);
            assert_eq!(spec.files.iter().map(|file| file.size).sum::<u64>(), spec.download_size());
            for file in spec.files {
                assert_eq!(file.sha256.len(), 64, "{} / {}", spec.id, file.path);
                assert!(file.sha256.chars().all(|ch| ch.is_ascii_hexdigit()), "{} / {}", spec.id, file.path);
                let official = spec.file_url(file, DownloadSource::Official);
                assert!(official.starts_with("https://"), "{official}");
                let mirror = spec.file_url(file, DownloadSource::Mirror);
                if spec.mirror_supported() {
                    assert!(mirror.starts_with("https://hf-mirror.com/"), "{mirror}");
                } else {
                    assert_eq!(mirror, official, "GitHub 项没有镜像");
                }
            }
        }
        assert!(find("whisper-large-v3-turbo").is_some());
        assert!(find("qwen3-asr-0.6b-int8").is_some());
        let turbo = find("whisper-large-v3-turbo").unwrap();
        assert!(turbo
            .file_url(&turbo.files[0], DownloadSource::Official)
            .contains("/resolve/4df90f75321148c3a29a9e2351b7ddf8f5b115a8/"));
        assert_eq!(vad_spec().engine, Engine::Vad);
        assert!(!vad_spec().mirror_supported());
        assert_eq!(DownloadSource::parse(" MIRROR "), Some(DownloadSource::Mirror));
        assert_eq!(DownloadSource::parse("official"), Some(DownloadSource::Official));
        assert_eq!(DownloadSource::parse("ftp"), None);
    }

    #[test]
    fn 状态_已安装_部分_缺失与旧目录识别() {
        let dir = temp_dir("status");
        let small = find("whisper-small").unwrap();
        assert_eq!(status(small, &dir).state, "missing");

        // 只有部分文件：partial
        let target = small.target_path(&dir);
        std::fs::create_dir_all(&target).unwrap();
        std::fs::write(target.join("model.bin"), b"x").unwrap();
        assert_eq!(status(small, &dir).state, "partial");

        // 补上必需文件：installed（其余可选项不影响判定）
        std::fs::write(target.join("tokenizer.json"), b"{}").unwrap();
        let installed = status(small, &dir);
        assert_eq!(installed.state, "installed");
        assert_eq!(installed.primary.as_deref(), Some(target.as_path()));

        // 旧版 large-v3 单模型目录也算已安装
        let legacy = legacy_whisper_dir(&dir);
        std::fs::create_dir_all(&legacy).unwrap();
        std::fs::write(legacy.join("model.bin"), b"x").unwrap();
        std::fs::write(legacy.join("tokenizer.json"), b"{}").unwrap();
        let large = find("whisper-large-v3").unwrap();
        assert_eq!(status(large, &dir).state, "installed");
        assert!(status(large, &dir).paths.contains(&legacy));

        // 旧 download_root 下的短名目录与 HF cache snapshot 也识别
        let nested = dir.join("faster_whisper").join("faster-whisper-base");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(nested.join("model.bin"), b"x").unwrap();
        std::fs::write(nested.join("tokenizer.json"), b"{}").unwrap();
        let base = find("whisper-base").unwrap();
        assert_eq!(status(base, &dir).state, "installed");
        let snapshot = dir.join("whisper").join("models--Systran--faster-whisper-tiny").join("snapshots").join("abc");
        std::fs::create_dir_all(&snapshot).unwrap();
        std::fs::write(snapshot.join("model.bin"), b"x").unwrap();
        std::fs::write(snapshot.join("tokenizer.json"), b"{}").unwrap();
        let tiny = find("whisper-tiny").unwrap();
        assert_eq!(status(tiny, &dir).primary.as_deref(), Some(snapshot.as_path()));

        // Qwen 旧目录名也能识别
        let qwen_dir = dir.join("qwen3_asr_onnx");
        std::fs::create_dir_all(qwen_dir.join("tokenizer")).unwrap();
        for name in ["conv_frontend.onnx", "encoder.int8.onnx", "decoder.int8.onnx"] {
            std::fs::write(qwen_dir.join(name), b"x").unwrap();
        }
        let qwen = find("qwen3-asr-0.6b-int8").unwrap();
        assert_eq!(status(qwen, &dir).primary.as_deref(), Some(qwen_dir.as_path()));

        // VAD 两个已知位置都认
        let vad = temp_dir("vad");
        std::fs::write(vad.join("silero_vad.onnx"), b"x").unwrap();
        assert!(vad_installed(&vad));
        std::fs::remove_file(vad.join("silero_vad.onnx")).unwrap();
        std::fs::create_dir_all(vad.join("sherpa-onnx")).unwrap();
        std::fs::write(vad.join("sherpa-onnx").join("silero_vad.onnx"), b"x").unwrap();
        assert!(vad_installed(&vad));
        std::fs::remove_file(vad.join("sherpa-onnx").join("silero_vad.onnx")).unwrap();
        let staging = vad.join(".downloads").join("_vad");
        std::fs::create_dir_all(&staging).unwrap();
        std::fs::write(staging.join("silero_vad.onnx.part"), b"partial").unwrap();
        assert!(!vad_installed(&vad), "暂存文件不能让 VAD 误报就绪");
        assert_eq!(status(vad_spec(), &vad).state, "partial");
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&vad);
    }

    #[test]
    fn 解压目录_能找到带一层父目录的模型() {
        let root = temp_dir("packaged");
        let inner = root.join("sherpa-onnx-qwen3-asr-0.6B-int8-2026-03-25");
        std::fs::create_dir_all(inner.join("tokenizer")).unwrap();
        for name in ["conv_frontend.onnx", "encoder.int8.onnx", "decoder.int8.onnx"] {
            std::fs::write(inner.join(name), b"x").unwrap();
        }
        let qwen = find("qwen3-asr-0.6b-int8").unwrap();
        let found = find_packaged_dir(&root, qwen.required, qwen.required_dir).unwrap();
        assert_eq!(found, inner);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn qwen只认清单别名_不误删用户目录() {
        let root = temp_dir("qwen-owned");
        let unrelated = root.join("qwen-experiments");
        std::fs::create_dir_all(unrelated.join("tokenizer")).unwrap();
        std::fs::write(unrelated.join("notes.txt"), b"keep").unwrap();
        let qwen = find("qwen3-asr-0.6b-int8").unwrap();
        assert_eq!(status(qwen, &root).state, "missing");
        assert!(existing_paths(qwen, &root).is_empty());

        for name in ["conv_frontend.onnx", "encoder.int8.onnx", "decoder.int8.onnx"] {
            std::fs::write(unrelated.join(name), b"x").unwrap();
        }
        assert_eq!(status(qwen, &root).state, "missing", "任意 qwen-* 结构不能冒充 0.6B 官方清单项");
        assert!(existing_paths(qwen, &root).is_empty());

        let known = root.join("qwen3_asr_onnx");
        std::fs::create_dir_all(known.join("tokenizer")).unwrap();
        for name in ["conv_frontend.onnx", "encoder.int8.onnx", "decoder.int8.onnx"] {
            std::fs::write(known.join(name), b"x").unwrap();
        }
        assert_eq!(status(qwen, &root).primary.as_deref(), Some(known.as_path()));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn 取消后的暂存文件算部分安装并可用于删除() {
        let root = temp_dir("staging");
        let small = find("whisper-small").unwrap();
        let staging = root.join(".downloads").join(small.id).join("files");
        std::fs::create_dir_all(&staging).unwrap();
        std::fs::write(staging.join("model.bin"), vec![0u8; 32]).unwrap();
        let status = status(small, &root);
        assert_eq!(status.state, "partial");
        assert!(status.installed_bytes >= 32);
        assert!(status.paths.contains(&root.join(".downloads").join(small.id)));
        let _ = std::fs::remove_dir_all(&root);
    }
}
