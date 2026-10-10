#!/usr/bin/env python
# -*- coding: utf-8 -*-
"""数据目录体检与迁移工具（ATT）。

设计原则（务必遵守）：

1. **默认只读**：不带 ``--apply`` 时，``--check`` / ``--to-flat`` / ``--to-shard``
   都不会改动磁盘上的任何文件。真正会改动 ``data/`` 的只有
   ``--to-flat --apply`` / ``--to-shard --apply`` / ``--init-schema``。
2. **绝不删除**：本工具从不删除数据文件，最多只做「同盘移动」。
   清理孤儿文件请用 ``scripts/housekeeping.py --apply``（另一套工具，有独立安全阀）。
3. **平铺布局下 wav 文件名 ≠ record id**：真实数据里 758 个 wav 的文件名是
   ``audio_path`` 里记录的另一个 uuid，与 709 个 record id **交集为 0**。
   因此任何迁移都必须以 JSON 里的 ``audio_path`` 为准，绝不能按 record id 猜文件名。
4. **先写 JSON 再落盘移动**：布局互转时顺序固定为
   「读 JSON → 校验 → 移动文件 → 回写 audio_path」，避免出现「文件已移动但
   JSON 仍指向旧路径」的中间态。

典型用法：

    uv run python scripts/migrate_data.py --check                    # 全量体检（只读）
    uv run python scripts/migrate_data.py --check --json             # 机器可读结果
    uv run python scripts/migrate_data.py --report --sample 20       # 统计 + 打印前 20 条条目
    uv run python scripts/migrate_data.py --export backup.zip        # 导出「解压即用」的 data/
    uv run python scripts/migrate_data.py --to-shard                 # 预演：平铺 -> 分片
    uv run python scripts/migrate_data.py --to-shard --apply         # 真正执行分片
    uv run python scripts/migrate_data.py --to-flat --apply          # 分片 -> 平铺（默认布局）
"""

from __future__ import annotations

import argparse
import json
import os
import re
import shutil
import sys
import zipfile
from dataclasses import dataclass, field
from datetime import datetime
from pathlib import Path

# --------------------------------------------------------------------------- 常量

# 数据目录结构版本；每次布局变更（如平铺↔分片）必须递增
SCHEMA_VERSION = 1
SCHEMA_FILE = ".schema_version"

# 仓库根目录：本脚本位于 <repo>/scripts/ 下
REPO_ROOT = Path(__file__).resolve().parent.parent

# uuid 形如 8-4-4-4-12；真实数据里被引用但并不存在的音频名也长这样，需要区分「命名规范但缺文件」与「名字本身可疑」
UUID_RE = re.compile(
    r"^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$"
)

# 记录文件中承载音频引用的字段（record 本体 / session segment / session 合并音频）
AUDIO_FIELDS = ("audio_path", "merged_audio_path")

# 已知历史异常（2026-10-01 报告 §4.6 实测）：体检必须能复现这些数字
KNOWN_EMPTY_AUDIO_PATH = 36      # 36 条记录 audio_path 为空
KNOWN_ID_MISMATCH = True         # wav 文件名与 record id 完全错位（交集 0）

EXIT_OK = 0
EXIT_FINDINGS = 1     # 发现问题（仅 --strict 时非 0）
EXIT_ERROR = 2        # 用法错误 / 迁移失败


# --------------------------------------------------------------------------- 工具函数


def eprint(message: str) -> None:
    """把错误信息写到 stderr。"""
    print(message, file=sys.stderr)


def setup_console_encoding() -> None:
    """把标准输出切到 UTF-8。

    Windows 控制台默认是 GBK 代码页，中文之外的符号（✔/✘ 等）会直接抛
    ``UnicodeEncodeError`` 把体检脚本打断。这里统一改成 UTF-8 并对无法编码的
    字符做替换，保证输出永远不会因为终端编码而崩掉。
    """
    for stream_name in ("stdout", "stderr"):
        stream = getattr(sys, stream_name, None)
        reconfigure = getattr(stream, "reconfigure", None)
        if reconfigure is None:
            continue
        try:
            reconfigure(encoding="utf-8", errors="replace")
        except (ValueError, OSError):
            continue


def human_size(num_bytes: int) -> str:
    """人类可读的字节数。"""
    value = float(num_bytes)
    for unit in ("B", "KB", "MB", "GB", "TB"):
        if value < 1024 or unit == "TB":
            return f"{value:.1f} {unit}" if unit != "B" else f"{int(value)} B"
        value /= 1024
    return f"{value:.1f} TB"


def rel_posix(path: Path, base: Path) -> str:
    """返回相对 base 的 POSIX 风格路径，便于跨平台比对。"""
    try:
        return path.resolve().relative_to(base.resolve()).as_posix()
    except ValueError:
        return path.as_posix()


def read_json_file(path: Path) -> tuple[object | None, str | None]:
    """读取 JSON，返回 (数据, 错误信息)。错误信息为 None 表示解析成功。"""
    try:
        raw = path.read_text(encoding="utf-8")
    except OSError as error:
        return None, f"读取失败: {error}"
    try:
        return json.loads(raw), None
    except json.JSONDecodeError as error:
        return None, f"JSON 解析失败: {error}"
    except UnicodeDecodeError as error:
        return None, f"编码错误(非 UTF-8): {error}"


def atomic_write_json(path: Path, payload: object) -> None:
    """原子写 JSON：先写临时文件再替换，避免进程被杀时留下半截文件。"""
    path.parent.mkdir(parents=True, exist_ok=True)
    temp_path = path.with_suffix(path.suffix + ".tmp-migrate")
    text = json.dumps(payload, ensure_ascii=False, indent=2)
    temp_path.write_text(text, encoding="utf-8")
    os.replace(temp_path, path)


def shard_parts(stem: str) -> tuple[str, ...]:
    """按文件名主干推导分片目录（两级 2/2 十六进制，长度不足时自然降级）。"""
    head = re.sub(r"[^0-9a-zA-Z]", "", stem)
    if len(head) >= 4:
        return (head[:2].lower(), head[2:4].lower())
    if len(head) >= 2:
        return (head[:2].lower(),)
    return ()


def is_probably_uuid(name: str) -> bool:
    """判断文件名主干是否形如 uuid。"""
    return bool(UUID_RE.match(name))


# --------------------------------------------------------------------------- 数据目录模型


@dataclass
class LayoutInfo:
    """单个音频目录的布局信息。"""

    root: Path                # data/audio/records 或 data/audio/permanent
    flat_files: list[Path] = field(default_factory=list)   # 直接位于 root 下的文件
    sharded_files: list[Path] = field(default_factory=list)  # 位于 root/<sub>/... 下的文件
    total_bytes: int = 0

    @property
    def total(self) -> int:
        return len(self.flat_files) + len(self.sharded_files)

    @property
    def layout(self) -> str:
        """flat / sharded / mixed / empty"""
        if self.total == 0:
            return "empty"
        if self.flat_files and not self.sharded_files:
            return "flat"
        if self.sharded_files and not self.flat_files:
            return "sharded"
        return "mixed"


class DataStore:
    """对 data/ 目录的只读视图 + 迁移能力。

    所有路径解析都以命令行传入的 ``data_dir`` 为基准，绝不假设进程工作目录。
    """

    def __init__(self, data_dir: Path) -> None:
        self.data_dir = data_dir.resolve()
        self.records_dir = self.data_dir / "records"
        self.sessions_dir = self.data_dir / "sessions"
        self.clipboard_dir = self.data_dir / "clipboard"
        self.clipboard_images_dir = self.clipboard_dir / "images"
        self.wordbook_dir = self.data_dir / "wordbook"
        self.audio_dir = self.data_dir / "audio"
        self.records_audio_dir = self.audio_dir / "records"
        self.permanent_audio_dir = self.audio_dir / "permanent"
        self.temp_audio_dir = self.audio_dir / "temp"
        self.schema_file = self.data_dir / SCHEMA_FILE

    # ---------------------------------------------------------------- 路径解析

    def resolve_audio(self, raw_path: str) -> Path | None:
        """把 JSON 中的相对/绝对音频路径解析成真实文件路径。

        历史写法不统一，按「谁真实存在就用谁」的顺序尝试：
        1) 相对 data/（如 ``audio/records/x.wav``）
        2) 相对仓库根（如 ``data\\audio\\records\\x.wav``，旧格式含反斜杠）
        3) 相对 data/audio（如 ``records/x.wav``）
        4) 裸文件名（如 ``x.wav``，默认落在 data/audio/records/）
        5) 容器内绝对路径 ``/app/data/audio/records/x.wav``（Docker 版写入）：
           原样路径在宿主机不存在时，按文件名回落到 data/audio/records/
        """
        raw = (raw_path or "").strip()
        if not raw:
            return None

        normalized = raw.replace("\\", "/")
        candidate = Path(normalized)
        if candidate.is_absolute():
            try:
                if candidate.is_file():
                    return candidate
            except OSError:
                pass
            # 绝对路径不存在（典型是容器内 /app/... 写法）：继续下面的候选路径
            if not normalized.startswith("/app/"):
                return candidate

        options = [
            self.data_dir / normalized,
            REPO_ROOT / normalized,
            self.audio_dir / normalized,
            self.records_audio_dir / candidate.name,
        ]
        for option in options:
            try:
                if option.is_file():
                    return option
            except OSError:
                continue

        # 全都不存在时返回首选解释，便于把「缺失」判成 record 缺音频而不是路径写错
        return self.data_dir / normalized

    def iter_json_files(self, directory: Path) -> list[Path]:
        """列出目录下的 .json 文件（非递归，目录不存在返回空）。"""
        if not directory.is_dir():
            return []
        return sorted(directory.glob("*.json"))

    # ---------------------------------------------------------------- 布局信息

    def scan_audio_layout(self, root: Path) -> LayoutInfo:
        """扫描音频目录，区分平铺文件与分片文件。"""
        info = LayoutInfo(root=root)
        if not root.is_dir():
            return info

        for path in root.rglob("*"):
            if not path.is_file():
                continue
            try:
                info.total_bytes += path.stat().st_size
            except OSError:
                pass
            if path.parent == root:
                info.flat_files.append(path)
            else:
                info.sharded_files.append(path)

        return info

    def audio_layout_summary(self) -> dict[str, LayoutInfo]:
        """返回主要音频目录的布局摘要。"""
        return {
            "records": self.scan_audio_layout(self.records_audio_dir),
            "permanent": self.scan_audio_layout(self.permanent_audio_dir),
            "temp": self.scan_audio_layout(self.temp_audio_dir),
        }

    # ---------------------------------------------------------------- schema 版本

    def read_schema_version(self) -> int | None:
        """读取 .schema_version；文件缺失或不可解析时返回 None。"""
        if not self.schema_file.is_file():
            return None
        try:
            text = self.schema_file.read_text(encoding="utf-8").strip()
        except OSError:
            return None
        if not text:
            return None
        try:
            return int(text.splitlines()[0])
        except ValueError:
            return None

    def write_schema_version(self, version: int = SCHEMA_VERSION) -> Path:
        """写入 .schema_version（只在 --init-schema / --apply 时调用）。"""
        self.data_dir.mkdir(parents=True, exist_ok=True)
        self.schema_file.write_text(f"{version}\n", encoding="utf-8")
        return self.schema_file


# --------------------------------------------------------------------------- 体检结果


@dataclass
class Issue:
    """一条体察发现。"""

    level: str      # error / warning / info
    code: str       # 机器可读的问题码
    message: str    # 中文描述
    paths: list[str] = field(default_factory=list)   # 相关文件（相对路径，最多保留若干条）

    def to_dict(self) -> dict:
        return {
            "level": self.level,
            "code": self.code,
            "message": self.message,
            "paths": self.paths,
        }


@dataclass
class CheckReport:
    """体检报告聚合对象。"""

    data_dir: Path
    generated_at: str = ""
    schema_version: int | None = None
    schema_file_exists: bool = False
    stats: dict = field(default_factory=dict)
    issues: list[Issue] = field(default_factory=list)

    def add(self, level: str, code: str, message: str, paths: list[str] | None = None,
            max_paths: int = 8) -> None:
        """追加一条发现（路径列表超长时截断，只保留前若干条）。"""
        trimmed = list(paths or [])[:max_paths]
        self.issues.append(Issue(level=level, code=code, message=message, paths=trimmed))

    @property
    def errors(self) -> list[Issue]:
        return [item for item in self.issues if item.level == "error"]

    @property
    def warnings(self) -> list[Issue]:
        return [item for item in self.issues if item.level == "warning"]

    def to_dict(self) -> dict:
        return {
            "data_dir": str(self.data_dir),
            "generated_at": self.generated_at,
            "schema_version": self.schema_version,
            "schema_file_exists": self.schema_file_exists,
            "stats": self.stats,
            "issues": [item.to_dict() for item in self.issues],
            "counts": {
                "errors": len(self.errors),
                "warnings": len(self.warnings),
                "infos": len([item for item in self.issues if item.level == "info"]),
            },
        }


# --------------------------------------------------------------------------- 体检实现


def _sample(paths: list[str], limit: int = 8) -> list[str]:
    """取前若干条路径作为样例。"""
    return paths[:limit]


def run_check(store: DataStore, *, sample_limit: int = 8) -> CheckReport:
    """全量体检：JSON 可解析 + 记录数与音频引用可达 + 孤儿/残留统计。

    该函数**只读**，不做任何写盘动作。
    """
    report = CheckReport(data_dir=store.data_dir)
    report.generated_at = datetime.now().isoformat(timespec="seconds")
    report.schema_version = store.read_schema_version()
    report.schema_file_exists = store.schema_file.is_file()

    stats: dict = {}
    stats["data_dir_exists"] = store.data_dir.is_dir()
    stats["schema_version"] = report.schema_version
    stats["repo_root"] = str(REPO_ROOT)

    # ------------------------------------------------------------------ 顶层结构
    expected_dirs = ["records", "sessions", "clipboard", "wordbook", "audio"]
    missing_dirs = [name for name in expected_dirs if not (store.data_dir / name).is_dir()]
    stats["missing_dirs"] = missing_dirs
    if missing_dirs:
        report.add(
            "warning",
            "missing_dirs",
            f"缺少预期子目录：{', '.join(missing_dirs)}（首次运行会由后端自动创建）",
        )

    if not report.schema_file_exists:
        report.add(
            "warning",
            "schema_version_missing",
            f"缺少 {SCHEMA_FILE} 版本标记，无法判断数据目录结构版本；"
            f"可执行 `--init-schema` 写入当前版本 {SCHEMA_VERSION}",
            [SCHEMA_FILE],
        )
    elif report.schema_version is None:
        report.add("error", "schema_version_unreadable",
                   f"{SCHEMA_FILE} 内容无法解析为整数", [SCHEMA_FILE])
    elif report.schema_version > SCHEMA_VERSION:
        report.add(
            "error",
            "schema_version_newer",
            f"数据目录版本 {report.schema_version} 高于本工具支持的 {SCHEMA_VERSION}，"
            "请升级 ATT 后再操作该目录",
            [SCHEMA_FILE],
        )
    elif report.schema_version < SCHEMA_VERSION:
        report.add(
            "info",
            "schema_version_older",
            f"数据目录版本 {report.schema_version} 低于当前 {SCHEMA_VERSION}",
            [SCHEMA_FILE],
        )

    # ------------------------------------------------------------------ JSON 可解析性
    json_errors: list[str] = []
    json_counts: dict[str, int] = {}
    for name, directory in (
        ("records", store.records_dir),
        ("sessions", store.sessions_dir),
        ("clipboard", store.clipboard_dir),
        ("wordbook", store.wordbook_dir),
    ):
        files = store.iter_json_files(directory)
        json_counts[name] = len(files)
        for path in files:
            _, error = read_json_file(path)
            if error:
                json_errors.append(f"{rel_posix(path, store.data_dir)} :: {error}")

    # 顶层散装 JSON（settings.json / llm_configs.json 等）
    top_level_json = sorted(
        path for path in store.data_dir.glob("*.json") if path.is_file()
    )
    for path in top_level_json:
        _, error = read_json_file(path)
        if error:
            json_errors.append(f"{path.name} :: {error}")

    stats["json_counts"] = json_counts
    stats["top_level_json"] = [path.name for path in top_level_json]
    stats["json_total"] = sum(json_counts.values()) + len(top_level_json)
    stats["json_errors"] = len(json_errors)
    if json_errors:
        report.add("error", "json_unparsable",
                   f"{len(json_errors)} 个 JSON 文件无法解析", _sample(json_errors, sample_limit))

    # ------------------------------------------------------------------ records
    record_files = store.iter_json_files(store.records_dir)
    record_ids: set[str] = set()
    empty_audio_path: list[str] = []          # 已知异常：36 条
    audio_missing: list[str] = []             # audio_path 非空但文件不可达
    container_path_audio: list[str] = []      # 指向容器内路径（/app/data/...）的记录
    audio_outside: list[str] = []             # audio_path 落在 data/audio/records 之外
    referenced_audio: set[str] = set()        # 被记录引用的音频绝对路径（normcase）
    external_ref_count = 0

    for path in record_files:
        record_id = path.stem
        record_ids.add(record_id)
        payload, error = read_json_file(path)
        if error or not isinstance(payload, dict):
            continue

        payload_id = str(payload.get("id") or "").strip()
        if payload_id and payload_id != record_id:
            report.add("warning", "record_id_mismatch",
                       f"记录 {record_id}.json 内部 id 为 {payload_id}，与文件名不一致",
                       [f"records/{path.name}"])

        raw_audio = str(payload.get("audio_path") or "").strip()
        if not raw_audio:
            empty_audio_path.append(f"records/{path.name}")
            continue

        resolved = store.resolve_audio(raw_audio)
        if resolved is None:
            empty_audio_path.append(f"records/{path.name}")
            continue

        # 容器内绝对路径（Docker 版写入的 /app/data/... ）需要改写；
        # 上面的 resolve_audio 已按文件名兜底命中，因此这里只做标注、不算断链
        if raw_audio.replace("\\", "/").startswith("/app/"):
            container_path_audio.append(f"records/{path.name} -> {raw_audio}")

        try:
            exists = resolved.is_file()
        except OSError:
            exists = False

        if not exists:
            audio_missing.append(f"records/{path.name} -> {raw_audio}")
            continue

        referenced_audio.add(os.path.normcase(str(resolved)))
        if store.records_audio_dir.resolve() not in resolved.resolve().parents:
            external_ref_count += 1
            if len(audio_outside) < 200:
                audio_outside.append(f"records/{path.name} -> {raw_audio}")

    stats["records"] = len(record_files)
    stats["records_named_by_uuid"] = sum(1 for name in {p.stem for p in record_files}
                                        if is_probably_uuid(name))
    stats["records_empty_audio_path"] = len(empty_audio_path)
    stats["records_audio_missing"] = len(audio_missing)
    stats["records_audio_container_path"] = len(container_path_audio)
    stats["records_audio_outside_records_dir"] = external_ref_count

    if empty_audio_path:
        report.add(
            "warning" if len(empty_audio_path) >= KNOWN_EMPTY_AUDIO_PATH else "info",
            "record_empty_audio_path",
            f"{len(empty_audio_path)} 条记录的 audio_path 为空（无法播放/导出音频，"
            f"历史已知异常基线为 {KNOWN_EMPTY_AUDIO_PATH} 条）",
            _sample(empty_audio_path, sample_limit),
        )
    if audio_missing:
        report.add("error", "record_audio_unreachable",
                   f"{len(audio_missing)} 条记录的 audio_path 指向不存在的文件（音频已丢失或路径写错）",
                   _sample(audio_missing, sample_limit))
    if container_path_audio:
        report.add("warning", "record_audio_container_path",
                   f"{len(container_path_audio)} 条记录的 audio_path 是容器内绝对路径（/app/data/...），"
                   "宿主机上按文件名兜底可达，但建议统一改写为相对 data/ 的相对路径",
                   _sample(container_path_audio, sample_limit))
    if external_ref_count:
        report.add("warning", "record_audio_outside",
                   f"{external_ref_count} 条记录的音频不在 data/audio/records/ 内（后端删除时会跳过这些文件）",
                   _sample(audio_outside, sample_limit))

    # ------------------------------------------------------------------ 音频文件与孤儿
    layout_summary = store.audio_layout_summary()
    records_audio_all: set[str] = set()
    records_audio_by_name: dict[str, list[Path]] = {}
    for path in layout_summary["records"].flat_files + layout_summary["records"].sharded_files:
        key = os.path.normcase(str(path))
        records_audio_all.add(key)
        records_audio_by_name.setdefault(path.name.lower(), []).append(path)

    orphan_audio = sorted(
        rel_posix(Path(key), store.data_dir)
        for key in records_audio_all - referenced_audio
    )
    # 文件名与 record id 完全错位：统计 wav 主干与 record id 的交集（真实数据为 0）
    wav_stems = {path.stem for path in layout_summary["records"].flat_files}
    wav_stems |= {path.stem for path in layout_summary["records"].sharded_files}
    id_overlap = record_ids & wav_stems

    stats["wav_total"] = len(records_audio_all)
    stats["wav_total_bytes"] = layout_summary["records"].total_bytes
    stats["wav_orphans"] = len(orphan_audio)
    stats["wav_orphans_bytes"] = sum(
        path.stat().st_size
        for path in layout_summary["records"].flat_files + layout_summary["records"].sharded_files
        if os.path.normcase(str(path)) not in referenced_audio
    )
    stats["wav_stem_vs_record_id_overlap"] = len(id_overlap)
    stats["audio_layout"] = {
        name: {
            "layout": info.layout,
            "flat": len(info.flat_files),
            "sharded": len(info.sharded_files),
            "bytes": info.total_bytes,
        }
        for name, info in layout_summary.items()
    }

    if stats["wav_total"] and not id_overlap:
        report.add(
            "warning",
            "wav_name_vs_record_id_mismatch",
            f"{stats['wav_total']} 个音频文件名（主干）与 {len(record_ids)} 个 record id "
            "**交集为 0**：音频名是另一个 uuid，二者完全错位。"
            "任何迁移都必须以 JSON 里的 audio_path 为准，禁止按 record id 推导文件名",
            _sample(sorted(wav_stems), sample_limit),
        )
    if orphan_audio:
        report.add(
            "info",
            "wav_orphans",
            f"{len(orphan_audio)} 个音频文件未被任何记录引用（孤儿，"
            f"{human_size(stats['wav_orphans_bytes'])}）；本工具不会删除，"
            "如需清理请用 scripts/housekeeping.py --apply",
            _sample(orphan_audio, sample_limit),
        )

    if layout_summary["records"].layout == "mixed":
        report.add(
            "warning",
            "audio_layout_mixed",
            "data/audio/records/ 同时存在平铺与分片文件（迁移中断的中间态）；"
            "可执行 --to-flat --apply 或 --to-shard --apply 收敛到单一布局",
        )

    record_temp_bytes = layout_summary["temp"].total_bytes
    stats["audio_temp_files"] = layout_summary["temp"].total
    stats["audio_temp_bytes"] = record_temp_bytes
    if layout_summary["temp"].total:
        report.add(
            "info",
            "audio_temp_residue",
            f"data/audio/temp/ 残留 {layout_summary['temp'].total} 个文件（{human_size(record_temp_bytes)}），"
            "属转码中间产物残留",
            _sample([rel_posix(p, store.data_dir) for p in layout_summary["temp"].flat_files], sample_limit),
        )

    # ------------------------------------------------------------------ sessions
    session_files = store.iter_json_files(store.sessions_dir)
    session_audio_missing: list[str] = []
    session_audio_referenced: set[str] = set()
    stalled_sessions: list[str] = []
    for path in session_files:
        payload, error = read_json_file(path)
        if error or not isinstance(payload, dict):
            continue

        status = str(payload.get("status") or "")
        if status in {"recording", "transcribing"}:
            stalled_sessions.append(f"sessions/{path.name} (status={status})")

        raw_paths: list[str] = []
        merged = str(payload.get("merged_audio_path") or "").strip()
        if merged:
            raw_paths.append(merged)
        for segment in payload.get("segments") or []:
            if isinstance(segment, dict):
                segment_audio = str(segment.get("audio_path") or "").strip()
                if segment_audio:
                    raw_paths.append(segment_audio)

        for raw in raw_paths:
            resolved = store.resolve_audio(raw)
            if resolved is None:
                continue
            try:
                exists = resolved.is_file()
            except OSError:
                exists = False
            if not exists:
                session_audio_missing.append(f"sessions/{path.name} -> {raw}")
            else:
                session_audio_referenced.add(os.path.normcase(str(resolved)))

    stats["sessions"] = len(session_files)
    stats["sessions_in_progress"] = len(stalled_sessions)
    stats["sessions_audio_missing"] = len(session_audio_missing)
    if stalled_sessions:
        report.add(
            "warning",
            "session_stalled",
            f"{len(stalled_sessions)} 个会话仍停在 recording/transcribing 中间态（历史数据里见过卡死 6 个月）",
            _sample(stalled_sessions, sample_limit),
        )
    if session_audio_missing:
        report.add("warning", "session_audio_unreachable",
                   f"{len(session_audio_missing)} 处会话音频引用不可达",
                   _sample(session_audio_missing, sample_limit))

    # ------------------------------------------------------------------ clipboard
    clipboard_files = store.iter_json_files(store.clipboard_dir)
    image_refs: set[str] = set()
    image_items = 0
    empty_hash: list[str] = []
    for path in clipboard_files:
        payload, error = read_json_file(path)
        if error or not isinstance(payload, dict):
            continue
        item_type = str(payload.get("item_type") or "")
        if item_type == "image":
            image_items += 1
            metadata = payload.get("metadata") or {}
            raw_image = ""
            if isinstance(metadata, dict):
                raw_image = str(metadata.get("image_path") or metadata.get("file_path") or "").strip()
            if raw_image:
                resolved = store.resolve_audio(raw_image)
                if resolved is not None:
                    image_refs.add(os.path.normcase(str(resolved)))
            elif not str(payload.get("content") or "").strip():
                empty_hash.append(f"clipboard/{path.name}")
        if not str(payload.get("content_hash") or "").strip():
            empty_hash.append(f"clipboard/{path.name}")

    images_on_disk = [
        path for path in store.clipboard_images_dir.rglob("*") if path.is_file()
    ] if store.clipboard_images_dir.is_dir() else []
    orphan_images = sorted(
        rel_posix(path, store.data_dir)
        for path in images_on_disk
        if os.path.normcase(str(path)) not in image_refs
    )

    stats["clipboard_items"] = len(clipboard_files)
    stats["clipboard_image_items"] = image_items
    stats["clipboard_images_on_disk"] = len(images_on_disk)
    stats["clipboard_orphan_images"] = len(orphan_images)
    stats["clipboard_missing_content_hash"] = len(empty_hash)
    if orphan_images:
        report.add(
            "info",
            "clipboard_orphan_images",
            f"{len(orphan_images)} 张剪贴板图片未被任何条目引用（孤儿）",
            _sample(orphan_images, sample_limit),
        )
    if empty_hash:
        report.add("warning", "clipboard_missing_hash",
                   f"{len(empty_hash)} 个剪贴板条目缺少 content_hash（去重失效）",
                   _sample(empty_hash, sample_limit))

    # ------------------------------------------------------------------ 残留与孤儿目录
    residue: list[str] = []
    for dirpath, dirnames, filenames in os.walk(store.data_dir):
        current = Path(dirpath)
        if current == store.data_dir:
            continue
        if current.name == "__pycache__":
            residue.append(rel_posix(current, store.data_dir))
            continue
        for name in filenames:
            if name.endswith((".tmp", ".tmp-migrate", ".bak")) or ".bak-" in name:
                residue.append(rel_posix(current / name, store.data_dir))

    known_residue_dirs = ["test-clipboard"]
    for name in known_residue_dirs:
        if (store.data_dir / name).is_dir():
            residue.append(name)

    stats["residue"] = len(residue)
    if residue:
        report.add("info", "data_residue",
                   f"{len(residue)} 项残留（临时文件/测试目录/备份文件）",
                   _sample(residue, sample_limit))

    # ------------------------------------------------------------------ 结论摘要
    stats["known_baseline"] = {
        "empty_audio_path": KNOWN_EMPTY_AUDIO_PATH,
        "wav_name_id_mismatch": KNOWN_ID_MISMATCH,
    }
    report.stats = stats
    return report


# --------------------------------------------------------------------------- 报告输出


def print_check_report(report: CheckReport, *, verbose: bool = False, sample: int = 8) -> None:
    """把体检报告打印成人类可读文本。"""
    stats = report.stats
    print("=" * 78)
    print("ATT 数据目录体检")
    print("=" * 78)
    print(f"数据目录      : {report.data_dir}")
    print(f"仓库根目录    : {stats.get('repo_root')}")
    print(f"体检时间      : {report.generated_at}")
    schema = report.schema_version
    print(f"结构版本      : {schema if schema is not None else '缺失（未初始化）'}"
          f"  文件存在={report.schema_file_exists}")

    print("-" * 78)
    print("【规模】")
    print(f"  记录 JSON        : {stats.get('records', 0)}")
    print(f"  会话 JSON        : {stats.get('sessions', 0)}")
    print(f"  剪贴板条目 JSON  : {stats.get('clipboard_items', 0)}（图片类型 {stats.get('clipboard_image_items', 0)}）")
    print(f"  音频文件         : {stats.get('wav_total', 0)} 个 / {human_size(stats.get('wav_total_bytes', 0))}")
    print(f"  剪贴板图片       : {stats.get('clipboard_images_on_disk', 0)} 张")
    print(f"  JSON 总数        : {stats.get('json_total', 0)}")

    print("-" * 78)
    print("【JSON 可解析性】")
    json_errors = stats.get("json_errors", 0)
    print(f"  解析失败         : {json_errors}" + ("  [OK]" if json_errors == 0 else "  [FAIL]"))
    print("  备注             : 0 个损坏 JSON 才是健康态；非 0 时见下方发现明细")
    for name, count in (stats.get("json_counts") or {}).items():
        print(f"    - {name:9s}: {count}")

    print("-" * 78)
    print("【音频引用可达性】")
    print(f"  audio_path 为空  : {stats.get('records_empty_audio_path', 0)}"
          f"（历史基线 {KNOWN_EMPTY_AUDIO_PATH}）")
    print(f"  引用不可达       : {stats.get('records_audio_missing', 0)}")
    print(f"  容器内绝对路径   : {stats.get('records_audio_container_path', 0)}"
          "（/app/data/... 需改写）")
    print(f"  引用在 records/ 外: {stats.get('records_audio_outside_records_dir', 0)}")
    print(f"  音频孤儿         : {stats.get('wav_orphans', 0)} 个 / {human_size(stats.get('wav_orphans_bytes', 0))}")
    print(f"  wav 名 vs id 交集: {stats.get('wav_stem_vs_record_id_overlap', 0)}"
          "   （0 = 完全错位，符合已知异常）")

    print("-" * 78)
    print("【音频目录布局】")
    for name, info in (stats.get("audio_layout") or {}).items():
        print(f"  {name:10s}: {info['layout']:8s} 平铺={info['flat']:4d} 分片={info['sharded']:4d} "
              f"{human_size(info['bytes'])}")

    print("-" * 78)
    print("【孤儿 / 残留】")
    print(f"  剪贴板孤儿图片   : {stats.get('clipboard_orphan_images', 0)}")
    print(f"  剪贴板缺 hash    : {stats.get('clipboard_missing_content_hash', 0)}")
    print(f"  audio/temp 残留  : {stats.get('audio_temp_files', 0)} 个 / {human_size(stats.get('audio_temp_bytes', 0))}")
    print(f"  其它残留         : {stats.get('residue', 0)}")
    print(f"  会话中间态卡死   : {stats.get('sessions_in_progress', 0)}")

    print("-" * 78)
    print("【发现】")
    if not report.issues:
        print("  无异常 [OK]")
    for issue in report.issues:
        mark = {"error": "[FAIL]", "warning": "[WARN]", "info": "[INFO]"}.get(issue.level, "[----]")
        print(f"  {mark} [{issue.code}] {issue.message}")
        if verbose:
            for path in issue.paths:
                print(f"        - {path}")

    print("-" * 78)
    print(f"汇总：error={len(report.errors)} warning={len(report.warnings)} "
          f"info={len([i for i in report.issues if i.level == 'info'])}")
    print("=" * 78)


def print_summary_report(store: DataStore, *, sample: int = 10) -> None:
    """只打印规模统计与前若干条明细（比 --check 更像概览）。"""
    report = run_check(store)
    stats = report.stats
    print(f"数据目录: {store.data_dir}")
    print(f"记录={stats.get('records', 0)} 会话={stats.get('sessions', 0)} "
          f"剪贴板={stats.get('clipboard_items', 0)} 音频={stats.get('wav_total', 0)} "
          f"({human_size(stats.get('wav_total_bytes', 0))})")
    print(f"error={len(report.errors)} warning={len(report.warnings)}")
    for issue in report.issues:
        print(f"  [{issue.level}] {issue.code}: {issue.message}")
        for path in issue.paths[:sample]:
            print(f"      - {path}")


# --------------------------------------------------------------------------- 导出 zip


def _iter_export_files(store: DataStore) -> tuple[list[Path], list[Path]]:
    """返回 (全部待导出文件, 必须排除的敏感文件)。"""
    excludes: list[Path] = []
    files: list[Path] = []

    key_file = store.data_dir / ".encryption_key"
    if key_file.is_file():
        excludes.append(key_file)

    for path in sorted(store.data_dir.rglob("*")):
        if not path.is_file():
            continue
        name = path.name
        # 临时/中间产物不进备份
        if name.endswith((".tmp", ".tmp-migrate")) or "__pycache__" in path.parts:
            continue
        if path in excludes:
            continue
        files.append(path)

    return files, excludes


def run_export(store: DataStore, destination: Path, *, include_key: bool = False) -> int:
    """导出「解压即用」的 data/ 压缩包。

    默认**排除** ``.encryption_key``：把解密密钥与密文放进同一个压缩包，
    等于把 API Key 一起送出去（这正是打包必须排除 data/ 的原因）。
    """
    if not store.data_dir.is_dir():
        eprint(f"[export] 数据目录不存在：{store.data_dir}")
        return EXIT_ERROR

    files, excludes = _iter_export_files(store)
    if include_key:
        files.extend(excludes)
        excludes = []

    key_file = store.data_dir / ".encryption_key"
    # 无论是否包含密钥，都在包内写一份清单，便于接收方核对
    manifest = {
        "exported_at": datetime.now().isoformat(timespec="seconds"),
        "source_data_dir": str(store.data_dir),
        "schema_version": store.read_schema_version(),
        "tool_schema_version": SCHEMA_VERSION,
        "include_encryption_key": bool(include_key),
        "file_count": len(files) + 1,
        "excluded": [rel_posix(path, store.data_dir) for path in excludes],
        "hint": (
            "解压后把 data/ 放到目标机器的数据目录："
            "开发态=仓库根目录，安装态=%APPDATA%/Audio to Text/data"
        ),
    }

    destination.parent.mkdir(parents=True, exist_ok=True)
    written = 0
    total_bytes = 0
    with zipfile.ZipFile(destination, "w", compression=zipfile.ZIP_DEFLATED, compresslevel=6) as archive:
        for path in files:
            arcname = f"data/{rel_posix(path, store.data_dir)}"
            archive.write(path, arcname)
            written += 1
            try:
                total_bytes += path.stat().st_size
            except OSError:
                pass
        archive.writestr("data/export-manifest.json",
                         json.dumps(manifest, ensure_ascii=False, indent=2))

    print("=" * 78)
    print("数据导出完成")
    print("=" * 78)
    print(f"源数据目录 : {store.data_dir}")
    print(f"输出压缩包 : {destination}")
    print(f"压缩包大小 : {human_size(destination.stat().st_size)}")
    print(f"原始大小   : {human_size(total_bytes)}")
    print(f"文件数     : {written + 1}（含 export-manifest.json）")
    if excludes and not include_key:
        print(f"已排除     : {', '.join(rel_posix(p, store.data_dir) for p in excludes)}"
              "  ← 解密密钥不随包分发")
    elif key_file.is_file() and include_key:
        print("[WARN] 已按要求包含 .encryption_key：该压缩包等于明文凭据，请勿外发")
    print("=" * 78)
    return EXIT_OK


# --------------------------------------------------------------------------- 布局互转


@dataclass
class MovePlan:
    """单个文件的移动计划。"""

    source: Path
    target: Path
    references: list[tuple[Path, str, str]] = field(default_factory=list)  # (JSON 文件, 字段, 旧值)


@dataclass
class MigrationPlan:
    """一次布局迁移的完整计划。"""

    mode: str                                     # "shard" 或 "flat"
    moves: list[MovePlan] = field(default_factory=list)
    skipped: list[str] = field(default_factory=list)     # 已就位，无需移动
    conflicts: list[str] = field(default_factory=list)   # 目标已存在 / 多处引用同名文件
    already: int = 0

    @property
    def total_bytes(self) -> int:
        size = 0
        for move in self.moves:
            try:
                size += move.source.stat().st_size
            except OSError:
                pass
        return size


def _collect_audio_references(store: DataStore) -> dict[str, list[tuple[Path, str, str]]]:
    """扫描 records/ 与 sessions/，建立「音频绝对路径 -> 引用它的 (JSON, 字段, 旧值)」索引。

    兼容两种 write-back：record 的 ``audio_path`` 字段、session 的 ``segments[].audio_path``
    与 ``merged_audio_path``。返回的键统一 normcase，便于 Windows 大小写无关比较。
    """
    index: dict[str, list[tuple[Path, str, str]]] = {}

    for path in store.iter_json_files(store.records_dir):
        payload, error = read_json_file(path)
        if error or not isinstance(payload, dict):
            continue
        raw = str(payload.get("audio_path") or "").strip()
        if not raw:
            continue
        resolved = store.resolve_audio(raw)
        if resolved is None:
            continue
        index.setdefault(os.path.normcase(str(resolved)), []).append((path, "audio_path", raw))

    for path in store.iter_json_files(store.sessions_dir):
        payload, error = read_json_file(path)
        if error or not isinstance(payload, dict):
            continue
        merged = str(payload.get("merged_audio_path") or "").strip()
        if merged:
            resolved = store.resolve_audio(merged)
            if resolved is not None:
                index.setdefault(os.path.normcase(str(resolved)), []).append(
                    (path, "merged_audio_path", merged)
                )
        for position, segment in enumerate(payload.get("segments") or []):
            if not isinstance(segment, dict):
                continue
            raw = str(segment.get("audio_path") or "").strip()
            if not raw:
                continue
            resolved = store.resolve_audio(raw)
            if resolved is not None:
                index.setdefault(os.path.normcase(str(resolved)), []).append(
                    (path, f"segments[{position}].audio_path", raw)
                )

    return index


def _target_path(store: DataStore, source: Path, mode: str) -> Path:
    """按目标布局计算文件应落在哪里。"""
    if mode == "flat":
        return store.records_audio_dir / source.name

    parts = shard_parts(source.stem)
    target_dir = store.records_audio_dir.joinpath(*parts) if parts else store.records_audio_dir
    return target_dir / source.name


def new_audio_path(store: DataStore, target: Path, old_raw: str) -> str:
    """按旧值的书写风格生成新的 audio_path。

    回写时必须沿用它自己那一套写法，否则会让后端路径解析（相对进程工作目录
    vs 相对 data/）前后不一致：

    1) 数据目录就在仓库 ``data/`` 下 → 维持 ``data\\audio\\records\\x.wav``（相对仓库根，各平台通用）
    2) 数据目录在别处（安装态 userData 等）→ 只能写相对 data/ 的 ``audio/records/x.wav``
    3) 旧值本来就写成 ``audio/records/x.wav`` → 继续写相对 data/ 的写法
    """
    old = (old_raw or "").strip()
    normalized_old = old.replace("\\", "/")
    data_relative = rel_posix(target, store.data_dir)
    repo_relative = rel_posix(target, REPO_ROOT)

    repo_data_dir = (REPO_ROOT / "data").resolve()
    data_under_repo = False
    try:
        data_under_repo = store.data_dir == repo_data_dir
    except OSError:
        data_under_repo = False

    if normalized_old.startswith("audio/"):
        return data_relative

    if data_under_repo:
        return repo_relative.replace("/", "\\") if os.name == "nt" else repo_relative

    return data_relative


def build_migration_plan(store: DataStore, mode: str) -> MigrationPlan:
    """构建布局迁移计划（只读，不改动任何文件）。"""
    plan = MigrationPlan(mode=mode)
    references = _collect_audio_references(store)
    layout = store.scan_audio_layout(store.records_audio_dir)

    candidates = layout.flat_files if mode == "shard" else layout.sharded_files

    for source in candidates:
        target = _target_path(store, source, mode)
        if target == source:
            plan.already += 1
            plan.skipped.append(rel_posix(source, store.data_dir))
            continue

        if target.exists() and os.path.normcase(str(target)) != os.path.normcase(str(source)):
            plan.conflicts.append(
                f"目标已存在，跳过：{rel_posix(source, store.data_dir)} -> {rel_posix(target, store.data_dir)}"
            )
            continue

        refs = references.get(os.path.normcase(str(source)), [])
        if not refs:
            # 未被任何记录/会话引用。仍可移动（保持数据完整），但标记出来提醒用户这是孤儿
            plan.moves.append(MovePlan(source=source, target=target, references=[]))
            continue

        plan.moves.append(MovePlan(source=source, target=target, references=refs))

    return plan


def apply_migration(store: DataStore, plan: MigrationPlan) -> tuple[int, list[str]]:
    """执行迁移计划。

    顺序：**先移动文件，成功后再回写 JSON**。单个文件失败只跳过该文件，
    绝不中断整体迁移（避免「一半文件移了、JSON 全没改」的不可恢复状态）。
    """
    moved = 0
    errors: list[str] = []

    for move in plan.moves:
        try:
            move.target.parent.mkdir(parents=True, exist_ok=True)
            shutil.move(str(move.source), str(move.target))
        except OSError as error:
            errors.append(f"移动失败 {move.source} -> {move.target}: {error}")
            continue

        moved += 1

        # 回写引用该音频的 JSON；同一文件可能被多条记录/会话引用，逐个更新。
        # 路径风格沿用该条 JSON 自己的旧写法（相对 data/ 或相对仓库根）
        touched: dict[Path, dict] = {}
        for json_path, field_name, old_value in move.references:
            payload = touched.get(json_path)
            if payload is None:
                payload, error = read_json_file(json_path)
                if error or not isinstance(payload, dict):
                    errors.append(f"回写失败（JSON 不可读）{json_path}: {error}")
                    continue
                touched[json_path] = payload

            new_raw = new_audio_path(store, move.target, old_value)

            if field_name == "audio_path":
                payload["audio_path"] = new_raw
            elif field_name == "merged_audio_path":
                payload["merged_audio_path"] = new_raw
            elif field_name.startswith("segments[") and field_name.endswith("].audio_path"):
                index = int(field_name[len("segments["):-len("].audio_path")])
                segments = payload.get("segments") or []
                if 0 <= index < len(segments) and isinstance(segments[index], dict):
                    segments[index]["audio_path"] = new_raw

        for json_path, payload in touched.items():
            try:
                atomic_write_json(json_path, payload)
            except OSError as error:
                errors.append(f"回写失败 {json_path}: {error}")

    return moved, errors


def print_migration_plan(store: DataStore, plan: MigrationPlan, *, apply: bool,
                         sample: int = 8) -> None:
    """打印迁移计划预览。"""
    mode_text = "平铺 -> 分片（两级 2/2 十六进制）" if plan.mode == "shard" else "分片 -> 平铺（默认布局）"
    print("=" * 78)
    print(f"数据布局迁移{'（已执行）' if apply else '（预演，未改动任何文件）'}")
    print("=" * 78)
    print(f"数据目录 : {store.data_dir}")
    print(f"迁移方向 : {mode_text}")
    print(f"待移动   : {len(plan.moves)} 个文件 / {human_size(plan.total_bytes)}")
    print(f"已就位   : {plan.already} 个")
    print(f"冲突跳过 : {len(plan.conflicts)} 个")
    if plan.moves:
        print("样例：")
        for move in plan.moves[:sample]:
            print(f"  {rel_posix(move.source, store.data_dir)}")
            print(f"    -> {rel_posix(move.target, store.data_dir)}"
                  f"  引用数={len(move.references)}")
    for conflict in plan.conflicts[:sample]:
        print(f"  [WARN] {conflict}")
    if not apply:
        print("-" * 78)
        print("以上为预演结果；确认无误后加 --apply 才会真正移动文件并回写 JSON。")
    print("=" * 78)


def run_layout_migration(store: DataStore, mode: str, *, apply: bool, sample: int = 8) -> int:
    """布局互转入口。"""
    if not store.records_audio_dir.is_dir():
        eprint(f"[migrate] 音频目录不存在：{store.records_audio_dir}")
        return EXIT_ERROR

    plan = build_migration_plan(store, mode)
    print_migration_plan(store, plan, apply=apply, sample=sample)

    if not apply:
        return EXIT_OK

    moved, errors = apply_migration(store, plan)

    # 迁移后写结构版本标记（分片与平铺共用同一版本号，布局由文件位置体现）
    try:
        store.write_schema_version(SCHEMA_VERSION)
    except OSError as error:
        errors.append(f"写入 {SCHEMA_FILE} 失败: {error}")

    print(f"已移动 {moved} 个文件，回写引用完成。")
    if errors:
        print("失败明细：")
        for item in errors[:sample]:
            print(f"  [FAIL] {item}")
        return EXIT_ERROR

    # 迁移后立刻复检，确认引用仍然可达
    report = run_check(store)
    unreachable = report.stats.get("records_audio_missing", 0)
    print(f"复检：引用不可达记录 = {unreachable}"
          + ("  [OK] 全部可达" if unreachable == 0 else "  [FAIL] 存在断链，请用备份回滚"))
    return EXIT_OK if unreachable == 0 else EXIT_ERROR


# --------------------------------------------------------------------------- CLI


def build_parser() -> argparse.ArgumentParser:
    """构造命令行参数。"""
    parser = argparse.ArgumentParser(
        prog="migrate_data.py",
        description="ATT 数据目录体检与迁移工具（默认只读，绝不删除数据）",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog=(
            "示例：\n"
            "  uv run python scripts/migrate_data.py --check\n"
            "  uv run python scripts/migrate_data.py --check --json\n"
            "  uv run python scripts/migrate_data.py --export D:/backup/att-data.zip\n"
            "  uv run python scripts/migrate_data.py --to-shard --apply\n"
            "  uv run python scripts/migrate_data.py --to-flat --apply\n"
        ),
    )
    parser.add_argument("--data-dir", default=None,
                        help="数据目录（默认取环境变量 DATA_DIR，再退回仓库内 data/）")

    actions = parser.add_mutually_exclusive_group()
    actions.add_argument("--check", action="store_true",
                         help="全量体检：JSON 可解析性、记录数与音频引用可达性、孤儿/残留统计")
    actions.add_argument("--report", action="store_true", help="打印规模统计与明细概览")
    actions.add_argument("--export", metavar="ZIP", default=None,
                         help="导出「解压即用」的 data/ 压缩包（默认排除 .encryption_key）")
    actions.add_argument("--to-shard", action="store_true",
                         help="把 data/audio/records/ 从平铺转成分片布局（默认预演）")
    actions.add_argument("--to-flat", action="store_true",
                         help="把分片布局收敛回平铺布局（默认布局，默认预演）")
    actions.add_argument("--init-schema", action="store_true",
                         help=f"仅写入 {SCHEMA_FILE}（当前版本 {SCHEMA_VERSION}）")

    parser.add_argument("--apply", action="store_true",
                        help="真正执行改动（--to-flat / --to-shard / --init-schema 默认是预演）")
    parser.add_argument("--json", action="store_true", help="以 JSON 输出体检结果（便于脚本消费）")
    parser.add_argument("--strict", action="store_true",
                        help="有 error 时退出码为 1（便于 CI 使用）")
    parser.add_argument("--verbose", action="store_true", help="打印每条发现的相关文件明细")
    parser.add_argument("--sample", type=int, default=8, help="每条发现最多打印的样例路径数（默认 8）")
    parser.add_argument("--include-encryption-key", action="store_true",
                        help="导出时包含 .encryption_key（慎用：等于明文凭据）")
    return parser


def resolve_data_dir(cli_value: str | None) -> Path:
    """解析数据目录：命令行 > 环境变量 DATA_DIR > 仓库内 data/。"""
    if cli_value:
        return Path(cli_value).expanduser().resolve()
    env_value = os.environ.get("DATA_DIR", "").strip()
    if env_value:
        return Path(env_value).expanduser().resolve()
    return (REPO_ROOT / "data").resolve()


def main(argv: list[str] | None = None) -> int:
    """命令行入口。"""
    setup_console_encoding()
    parser = build_parser()
    args = parser.parse_args(argv)

    data_dir = resolve_data_dir(args.data_dir)
    if not data_dir.is_dir():
        eprint(f"[migrate] 数据目录不存在：{data_dir}")
        return EXIT_ERROR

    store = DataStore(data_dir)

    if args.export:
        return run_export(store, Path(args.export).expanduser().resolve(),
                          include_key=args.include_encryption_key)

    if args.to_shard or args.to_flat:
        mode = "shard" if args.to_shard else "flat"
        return run_layout_migration(store, mode, apply=args.apply, sample=args.sample)

    if args.init_schema:
        if not args.apply:
            print(f"将写入 {store.schema_file}（版本 {SCHEMA_VERSION}）；确认后加 --apply 执行。")
            return EXIT_OK
        path = store.write_schema_version()
        print(f"已写入 {path}（版本 {SCHEMA_VERSION}）")
        return EXIT_OK

    if args.report:
        print_summary_report(store, sample=args.sample)
        return EXIT_OK

    # 默认行为就是 --check：最常用、最安全
    report = run_check(store, sample_limit=args.sample)
    if args.json:
        print(json.dumps(report.to_dict(), ensure_ascii=False, indent=2))
    else:
        print_check_report(report, verbose=args.verbose, sample=args.sample)

    if args.strict and report.errors:
        return EXIT_FINDINGS
    return EXIT_OK


if __name__ == "__main__":
    sys.exit(main())
