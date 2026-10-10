"""文件侧数据卫生（housekeeping）：JSON 唯一真相源下的保留策略与卡死状态收敛。

背景（见报告 §3.2 / §4.2 / §3.5⑥）：
- 项目不使用任何数据库，``data/`` 就是唯一真相源，因而「数据库 retention job」的职责
  必须落到文件侧：孤儿图片、合并后残留的片段音频、长年卡在中间态的会话/记录；
- 这些问题不会让服务崩溃，但会让 ``data/`` 持续膨胀、让 UI 永远显示「进行中」。

安全约束（硬要求）：
- ``data/`` 是用户真实数据，本模块**默认只读**（dry_run=True），只扫描、只报告；
- 仅当显式传入 ``dry_run=False`` 或设置环境变量 ``ATT_HOUSEKEEPING_APPLY=1`` 时才真正删除/改状态；
- 所有删除动作先经 :func:`is_within_directory` 校验目录包含性，越界路径一律跳过并记录；
- 所有状态改写走 :func:`write_json_atomic_async` + 与各 Manager 相同的资源锁，
  保证「读-改-写」与业务写入互斥；
- 启动钩子 :func:`run_startup_housekeeping` **永远只读**，环境变量也不会让它删任何东西。

扫描出的四类问题：
1. ``orphan_image``：``data/clipboard/images/`` 中未被任何剪贴板条目引用的图片；
2. ``stale_segment``：会话已完成且合并音频已落盘后，仍留在磁盘上的片段音频；
3. ``stuck_session``：超过阈值小时数仍停留在 ``recording`` 的会话；
4. ``stuck_record``：超过阈值小时数仍停留在 ``optimizing`` / ``transcribing`` 的记录。
"""

from __future__ import annotations

import asyncio
import json
import logging
import os
from dataclasses import dataclass, field
from datetime import datetime
from pathlib import Path
from typing import Callable, Dict, List, Optional, Tuple

from app.repositories.json_store import (
    get_async_lock,
    read_json_lenient_async,
    write_json_atomic_async,
)
from app.repositories.paths import is_safe_identifier, is_within_directory

logger = logging.getLogger(__name__)

# 显式允许执行清理的环境变量（迁移 / 手工运维用）
APPLY_ENV = "ATT_HOUSEKEEPING_APPLY"

_APPLY_ENV_VALUES = {"1", "true", "yes", "on"}

# 默认卡死阈值：超过 6 小时仍停在中间态即视为卡死
DEFAULT_STUCK_AFTER_HOURS = 6.0

# 默认孤儿图片最小年龄：刚写入还没落 JSON 的图片不参与清理（防止与捕获流程竞态）
DEFAULT_ORPHAN_MIN_AGE_HOURS = 1.0

# 报告里每类问题最多列出的明细条数
DEFAULT_DETAIL_LIMIT = 20

# 四类问题标识
CATEGORY_ORPHAN_IMAGE = "orphan_image"
CATEGORY_STALE_SEGMENT = "stale_segment"
CATEGORY_STUCK_SESSION = "stuck_session"
CATEGORY_STUCK_RECORD = "stuck_record"

ALL_CATEGORIES: Tuple[str, ...] = (
    CATEGORY_ORPHAN_IMAGE,
    CATEGORY_STALE_SEGMENT,
    CATEGORY_STUCK_SESSION,
    CATEGORY_STUCK_RECORD,
)

# 中文标签，用于日志与 CLI 输出
CATEGORY_LABELS: Dict[str, str] = {
    CATEGORY_ORPHAN_IMAGE: "孤儿图片",
    CATEGORY_STALE_SEGMENT: "残留片段音频",
    CATEGORY_STUCK_SESSION: "卡死会话",
    CATEGORY_STUCK_RECORD: "卡在中间态的记录",
}

# 会话卡死态
_STUCK_SESSION_STATUS = "recording"

# 记录卡死态（与 RecordStatus 的 value 保持一致，避免与 models 互相 import）
_STUCK_RECORD_STATUSES = {"optimizing", "transcribing"}

# 记录失败态（RecordStatus.FAILED.value）
_FAILED_STATUS = "failed"

# 剪贴板条目里可能存放图片文件名的元数据键
_IMAGE_METADATA_KEYS = ("image_path", "stored_file_name", "file_name")

# 图片哈希键：部分历史数据只存哈希，文件名就是 ``<hash>.png``
_IMAGE_HASH_KEY = "image_hash"


# --------------------------------------------------------------------------- 通用工具


def housekeeping_apply_requested() -> bool:
    """``ATT_HOUSEKEEPING_APPLY=1`` 时返回 True（显式要求执行清理）。"""

    value = str(os.getenv(APPLY_ENV, "")).strip().lower()
    return value in _APPLY_ENV_VALUES


def resolve_dry_run(dry_run: Optional[bool]) -> bool:
    """把三态参数收敛成最终是否只读。

    - ``dry_run=True`` → 只读（显式要求，环境变量无法覆盖，保证「说不删就不删」）；
    - ``dry_run=False`` → 执行；
    - ``dry_run=None``（默认）→ 由环境变量决定：未设置即只读。
    """

    if dry_run is None:
        return not housekeeping_apply_requested()
    return bool(dry_run)


def _parse_datetime(value: object) -> Optional[datetime]:
    """把 JSON 里的时间字符串解析成 ``datetime``，失败返回 ``None``。"""

    if not isinstance(value, str) or not value.strip():
        return None
    try:
        return datetime.fromisoformat(value.strip())
    except ValueError:
        return None


def _age_hours(moment: Optional[datetime], now: datetime) -> Optional[float]:
    """计算距 ``now`` 的小时数；时间缺失或晚于当前时间时返回 ``None``。"""

    if moment is None:
        return None
    try:
        delta = now - moment
    except TypeError:
        return None
    return delta.total_seconds() / 3600.0


def _normcase_path(value: object) -> str:
    """路径规范化（Windows 大小写不敏感），用于比较两个路径是否指向同一文件。"""

    return os.path.normcase(str(value or "").strip())


# --------------------------------------------------------------------------- 数据结构


@dataclass
class Finding:
    """一条待处理问题。

    :param category: 问题类别（见 ``CATEGORY_*``）
    :param path: 涉及的文件绝对路径（卡死状态类问题指向对应的 JSON 文件）
    :param identifier: 业务 ID（记录 ID / 会话 ID），无则空串
    :param detail: 人类可读的中文说明
    :param size_bytes: 涉及文件大小（字节），状态类问题为 0
    """

    category: str
    path: str
    identifier: str = ""
    detail: str = ""
    size_bytes: int = 0

    def to_dict(self, data_path: Optional[str] = None) -> dict:
        """转成可 JSON 序列化的字典；路径尽量转成相对 ``data/`` 的写法便于阅读。"""

        display_path = self.path
        if data_path:
            try:
                display_path = str(Path(self.path).relative_to(Path(data_path)))
            except (ValueError, OSError):
                display_path = self.path

        return {
            "category": self.category,
            "id": self.identifier,
            "path": display_path,
            "absolute_path": self.path,
            "size_bytes": self.size_bytes,
            "detail": self.detail,
        }


@dataclass
class HousekeepingReport:
    """一次体检（可选执行）的结构化报告。"""

    dry_run: bool = True
    data_path: str = ""
    generated_at: str = field(default_factory=lambda: datetime.now().isoformat())
    stuck_after_hours: float = DEFAULT_STUCK_AFTER_HOURS
    orphan_min_age_hours: float = DEFAULT_ORPHAN_MIN_AGE_HOURS
    scanned: Dict[str, int] = field(default_factory=dict)
    findings: List[Finding] = field(default_factory=list)
    applied: Dict[str, int] = field(default_factory=dict)
    errors: List[str] = field(default_factory=list)

    # ---------------------------------------------------------------- 查询辅助

    def items_of(self, category: str) -> List[Finding]:
        """取出某一类的全部问题明细。"""

        return [item for item in self.findings if item.category == category]

    def count_of(self, category: str) -> int:
        """某一类问题的数量。"""

        return len(self.items_of(category))

    def counts(self) -> Dict[str, int]:
        """四类问题的数量汇总（无问题的类别也返回 0，便于对比前后）。"""

        return {category: self.count_of(category) for category in ALL_CATEGORIES}

    def total_bytes(self, category: str) -> int:
        """某一类问题涉及的磁盘占用（字节）。"""

        return sum(item.size_bytes for item in self.items_of(category))

    def has_findings(self) -> bool:
        """是否存在任何待处理问题。"""

        return bool(self.findings)

    def summary(self) -> str:
        """一行中文摘要，用于日志。"""

        counts = self.counts()
        parts = [f"{CATEGORY_LABELS[category]} {counts[category]}" for category in ALL_CATEGORIES]
        state = "只读体检" if self.dry_run else "已执行清理"
        applied_total = sum(self.applied.values())
        tail = f"，实际处理 {applied_total} 项" if not self.dry_run else ""
        return f"{state}：" + "、".join(parts) + tail

    def to_dict(self, *, detail_limit: int = DEFAULT_DETAIL_LIMIT) -> dict:
        """完整报告字典；每类明细只保留前 ``detail_limit`` 条并标记是否截断。"""

        categories: Dict[str, dict] = {}
        for category in ALL_CATEGORIES:
            items = self.items_of(category)
            categories[category] = {
                "label": CATEGORY_LABELS[category],
                "count": len(items),
                "total_bytes": sum(item.size_bytes for item in items),
                "truncated": len(items) > detail_limit,
                "items": [item.to_dict(self.data_path) for item in items[:detail_limit]],
            }

        return {
            "dry_run": self.dry_run,
            "data_path": self.data_path,
            "generated_at": self.generated_at,
            "stuck_after_hours": self.stuck_after_hours,
            "orphan_min_age_hours": self.orphan_min_age_hours,
            "scanned": dict(self.scanned),
            "counts": self.counts(),
            "total_bytes": {category: self.total_bytes(category) for category in ALL_CATEGORIES},
            "applied": dict(self.applied),
            "errors": list(self.errors),
            "categories": categories,
        }


# --------------------------------------------------------------------------- 主体


class Housekeeping:
    """文件侧数据卫生扫描/清理器。

    :param data_path: ``data/`` 目录；测试必须显式传入临时目录，避免误碰真实数据
    :param stuck_after_hours: 中间态超过该小时数即判为卡死
    :param orphan_min_age_hours: 孤儿图片的最小年龄，避免与图片捕获流程竞态
    """

    def __init__(
        self,
        data_path: Optional[Path] = None,
        *,
        stuck_after_hours: float = DEFAULT_STUCK_AFTER_HOURS,
        orphan_min_age_hours: float = DEFAULT_ORPHAN_MIN_AGE_HOURS,
    ):
        if data_path is None:
            # 延迟 import：避免测试与 CLI 为了拿一个默认路径而初始化整套 settings
            from app.config import settings

            data_path = settings.data_path

        self.data_path = Path(data_path).resolve()
        self.stuck_after_hours = float(stuck_after_hours)
        self.orphan_min_age_hours = float(orphan_min_age_hours)

    # ---------------------------------------------------------------- 目录

    @property
    def records_dir(self) -> Path:
        return self.data_path / "records"

    @property
    def sessions_dir(self) -> Path:
        return self.data_path / "sessions"

    @property
    def clipboard_dir(self) -> Path:
        return self.data_path / "clipboard"

    @property
    def clipboard_images_dir(self) -> Path:
        return self.clipboard_dir / "images"

    @property
    def records_audio_dir(self) -> Path:
        return self.data_path / "audio" / "records"

    # ---------------------------------------------------------------- 扫描

    def scan(self) -> HousekeepingReport:
        """只读扫描，返回结构化报告；本方法绝不修改磁盘。"""

        report = HousekeepingReport(
            dry_run=True,
            data_path=str(self.data_path),
            stuck_after_hours=self.stuck_after_hours,
            orphan_min_age_hours=self.orphan_min_age_hours,
        )

        self._scan_orphan_images(report)
        self._scan_stale_segments(report)
        self._scan_stuck_sessions(report)
        self._scan_stuck_records(report)

        return report

    def _scan_orphan_images(self, report: HousekeepingReport) -> None:
        """扫描 ``data/clipboard/images/`` 中未被任何剪贴板条目引用的图片。"""

        images_dir = self.clipboard_images_dir
        now = datetime.now()

        references = self._collect_image_references(report)

        files = self._iter_files(images_dir)
        report.scanned["clipboard_images"] = len(files)

        for image_file in files:
            # 命中任意一种引用写法（绝对路径 / images/ 前缀 / 纯文件名）都按「被引用」处理
            if image_file.name.lower() in references:
                continue

            age = _age_hours(datetime.fromtimestamp(image_file.stat().st_mtime), now)
            if age is not None and age < self.orphan_min_age_hours:
                # 刚写入的图片可能还没落条目 JSON，按「暂不处理」跳过
                continue

            detail = "未被任何剪贴板条目引用"
            if age is not None:
                detail = f"{detail}（文件年龄 {age:.1f} 小时）"

            report.findings.append(
                Finding(
                    category=CATEGORY_ORPHAN_IMAGE,
                    path=str(image_file),
                    detail=detail,
                    size_bytes=self._safe_size(image_file),
                )
            )

    def _collect_image_references(self, report: HousekeepingReport) -> set:
        """收集剪贴板条目引用到的图片文件名（小写化的 basename 集合）。"""

        references: set = set()
        item_files = self._iter_json_files(self.clipboard_dir)
        report.scanned["clipboard_items"] = len(item_files)

        for item_file in item_files:
            data = self._load_json_dict(item_file)
            if data is None:
                report.scanned["unreadable_json"] = report.scanned.get("unreadable_json", 0) + 1
                continue

            metadata = data.get("metadata")
            if not isinstance(metadata, dict):
                continue

            for key in _IMAGE_METADATA_KEYS:
                raw_value = str(metadata.get(key) or "").strip()
                if raw_value:
                    # 统一按 basename 比对，兼容绝对路径 / data 前缀 / images/ 前缀等历史写法
                    references.add(Path(raw_value.replace("\\", "/")).name.lower())

            raw_hash = str(metadata.get(_IMAGE_HASH_KEY) or "").strip()
            if raw_hash:
                references.add(Path(f"{raw_hash}.png").name.lower())

        return references

    def _scan_stale_segments(self, report: HousekeepingReport) -> None:
        """扫描「会话已完成且合并音频已落盘」后残留的片段音频。"""

        audio_dir = self.records_audio_dir
        session_files = self._iter_json_files(self.sessions_dir)
        report.scanned["sessions"] = len(session_files)

        for session_file in session_files:
            data = self._load_json_dict(session_file)
            if data is None:
                report.scanned["unreadable_json"] = report.scanned.get("unreadable_json", 0) + 1
                continue

            if str(data.get("status") or "") != "completed":
                continue

            merged_raw = str(data.get("merged_audio_path") or "").strip()
            if not merged_raw:
                continue

            merged_path = self._resolve_data_path(merged_raw)
            if not merged_path.is_file():
                # 合并音频不存在时不能判定片段是残留，宁可不处理
                continue

            session_id = str(data.get("id") or session_file.stem)

            for segment in data.get("segments") or []:
                if not isinstance(segment, dict):
                    continue

                segment_raw = str(segment.get("audio_path") or "").strip()
                if not segment_raw:
                    continue

                segment_path = self._resolve_data_path(segment_raw)
                if _normcase_path(segment_path) == _normcase_path(merged_path):
                    continue
                if not segment_path.is_file():
                    continue
                if not is_within_directory(segment_path, audio_dir):
                    # 越界路径只记录不处理，绝不删除 data/audio/records/ 之外的文件
                    report.errors.append(f"片段音频越界，已跳过: {segment_path}")
                    continue

                report.findings.append(
                    Finding(
                        category=CATEGORY_STALE_SEGMENT,
                        path=str(segment_path),
                        identifier=session_id,
                        detail="会话已完成并生成合并音频，该片段音频为残留",
                        size_bytes=self._safe_size(segment_path),
                    )
                )

    def _scan_stuck_sessions(self, report: HousekeepingReport) -> None:
        """扫描超过阈值仍停留在 ``recording`` 的会话。"""

        now = datetime.now()
        session_files = self._iter_json_files(self.sessions_dir)
        report.scanned.setdefault("sessions", len(session_files))

        for session_file in session_files:
            data = self._load_json_dict(session_file)
            if data is None:
                continue

            status = str(data.get("status") or "")
            if status != _STUCK_SESSION_STATUS:
                continue

            created_at = _parse_datetime(data.get("created_at"))
            age = _age_hours(created_at, now)
            if age is None:
                age = _age_hours(datetime.fromtimestamp(session_file.stat().st_mtime), now)

            if age is None or age <= self.stuck_after_hours:
                continue

            report.findings.append(
                Finding(
                    category=CATEGORY_STUCK_SESSION,
                    path=str(session_file),
                    identifier=str(data.get("id") or session_file.stem),
                    detail=(
                        f"会话停留在 {status} 已 {age:.1f} 小时"
                        f"（阈值 {self.stuck_after_hours:.0f} 小时），片段数 "
                        f"{len(data.get('segments') or [])}"
                    ),
                )
            )

    def _scan_stuck_records(self, report: HousekeepingReport) -> None:
        """扫描超过阈值仍停留在 ``optimizing`` / ``transcribing`` 的记录。"""

        now = datetime.now()
        record_files = self._iter_json_files(self.records_dir)
        report.scanned["records"] = len(record_files)

        for record_file in record_files:
            data = self._load_json_dict(record_file)
            if data is None:
                report.scanned["unreadable_json"] = report.scanned.get("unreadable_json", 0) + 1
                continue

            status = str(data.get("status") or "")
            if status not in _STUCK_RECORD_STATUSES:
                continue

            # 优先用 updated_at（进入中间态的时间），缺失时退回 created_at
            moment = _parse_datetime(data.get("updated_at")) or _parse_datetime(data.get("created_at"))
            age = _age_hours(moment, now)
            if age is None or age <= self.stuck_after_hours:
                continue

            report.findings.append(
                Finding(
                    category=CATEGORY_STUCK_RECORD,
                    path=str(record_file),
                    identifier=str(data.get("id") or record_file.stem),
                    detail=(
                        f"记录停留在 {status} 已 {age:.1f} 小时"
                        f"（阈值 {self.stuck_after_hours:.0f} 小时）"
                    ),
                )
            )

    # ---------------------------------------------------------------- 执行

    async def apply(self, report: HousekeepingReport) -> HousekeepingReport:
        """执行报告中的破坏性动作；调用方必须先确认 ``dry_run=False``。

        每个动作都会重新做一次目录包含性校验，避免报告在扫描后被篡改导致越界删除。
        """

        if report.dry_run:
            raise ValueError("apply() 只接受 dry_run=False 的报告；默认路径必须保持只读")

        report.applied = {category: 0 for category in ALL_CATEGORIES}

        await self._apply_orphan_images(report)
        await self._apply_stale_segments(report)
        await self._apply_stuck_sessions(report)
        await self._apply_stuck_records(report)

        return report

    async def run(
        self,
        *,
        dry_run: Optional[bool] = None,
        report: Optional[HousekeepingReport] = None,
    ) -> HousekeepingReport:
        """扫描（可选执行）+ 写日志的统一入口。

        :param dry_run: ``None`` 表示按环境变量裁决（默认只读）；``True`` 强制只读；``False`` 执行
        :param report: 可复用的扫描结果；不传则现扫一次
        """

        effective_dry_run = resolve_dry_run(dry_run)

        if report is None:
            # 扫描是纯文件 I/O，放到线程池避免阻塞事件循环（lifespan / API 复用同一入口）
            report = await asyncio.to_thread(self.scan)

        report.dry_run = effective_dry_run

        if not effective_dry_run:
            await self.apply(report)

        log_report(report)
        return report

    async def _apply_orphan_images(self, report: HousekeepingReport) -> None:
        """删除未被引用的图片（限定在 data/clipboard/images/ 内）。"""

        for finding in report.items_of(CATEGORY_ORPHAN_IMAGE):
            path = Path(finding.path)
            if not is_within_directory(path, self.clipboard_images_dir):
                report.errors.append(f"孤儿图片越界，已跳过: {path}")
                continue

            try:
                path.unlink()
            except FileNotFoundError:
                # 已被其它流程清理：目标已达成，按成功计数
                report.applied[CATEGORY_ORPHAN_IMAGE] += 1
                continue
            except OSError as error:
                report.errors.append(f"删除孤儿图片失败: {path}（{error}）")
                continue

            logger.info("已删除孤儿图片: %s", path)
            report.applied[CATEGORY_ORPHAN_IMAGE] += 1

    async def _apply_stale_segments(self, report: HousekeepingReport) -> None:
        """删除残留片段音频，并同步清空会话 JSON 里的片段路径（保持 JSON 与磁盘一致）。"""

        deleted_by_session: Dict[str, List[str]] = {}

        for finding in report.items_of(CATEGORY_STALE_SEGMENT):
            path = Path(finding.path)
            if not is_within_directory(path, self.records_audio_dir):
                report.errors.append(f"残留片段音频越界，已跳过: {path}")
                continue

            try:
                path.unlink()
            except FileNotFoundError:
                pass
            except OSError as error:
                report.errors.append(f"删除残留片段音频失败: {path}（{error}）")
                continue

            logger.info("已删除残留片段音频: %s", path)
            report.applied[CATEGORY_STALE_SEGMENT] += 1
            deleted_by_session.setdefault(finding.identifier, []).append(finding.path)

        for session_id, deleted_paths in deleted_by_session.items():
            deleted_keys = {_normcase_path(item) for item in deleted_paths}

            def clear_audio_path(data: dict, keys: set = deleted_keys) -> bool:
                """把已被删除的片段路径置空，避免 JSON 指向不存在的文件。"""

                changed = False
                for segment in data.get("segments") or []:
                    if not isinstance(segment, dict):
                        continue
                    if _normcase_path(segment.get("audio_path")) in keys:
                        segment["audio_path"] = ""
                        changed = True
                return changed

            error = await self._update_json_file(self.sessions_dir, session_id, clear_audio_path)
            if error:
                report.errors.append(error)

    async def _apply_stuck_sessions(self, report: HousekeepingReport) -> None:
        """把卡死的会话标记为 failed 并补上完成时间，让 UI 不再永远显示「录音中」。"""

        for finding in report.items_of(CATEGORY_STUCK_SESSION):
            now_iso = datetime.now().isoformat()

            def mark_failed(data: dict, stamp: str = now_iso) -> bool:
                if str(data.get("status") or "") != _STUCK_SESSION_STATUS:
                    # 扫描后状态已被业务改写（例如会话真的完成了），不再覆盖
                    return False
                data["status"] = _FAILED_STATUS
                data["completed_at"] = stamp
                return True

            error = await self._update_json_file(self.sessions_dir, finding.identifier, mark_failed)
            if error:
                report.errors.append(error)
                continue

            report.applied[CATEGORY_STUCK_SESSION] += 1
            logger.info("已把卡死会话标记为失败: %s", finding.identifier)

    async def _apply_stuck_records(self, report: HousekeepingReport) -> None:
        """把卡在 optimizing / transcribing 的记录标记为 failed 并持久化失败原因。"""

        for finding in report.items_of(CATEGORY_STUCK_RECORD):
            now_iso = datetime.now().isoformat()

            def mark_failed(data: dict, stamp: str = now_iso) -> bool:
                previous = str(data.get("status") or "")
                if previous not in _STUCK_RECORD_STATUSES:
                    return False

                # 用真实停留时长写失败原因（而不是只写阈值），便于用户判断是否需要重试
                moment = _parse_datetime(data.get("updated_at")) or _parse_datetime(data.get("created_at"))
                age = _age_hours(moment, datetime.now())
                age_text = f"{age:.1f} 小时" if age is not None else f"超过 {self.stuck_after_hours:.0f} 小时"

                data["status"] = _FAILED_STATUS
                data["error"] = (
                    f"任务在 {age_text}前停留在 {previous}，已由数据卫生清理标记为失败；"
                    "如需结果请重新提交优化。"
                )
                data["updated_at"] = stamp
                return True

            error = await self._update_json_file(self.records_dir, finding.identifier, mark_failed)
            if error:
                report.errors.append(error)
                continue

            report.applied[CATEGORY_STUCK_RECORD] += 1
            logger.info("已把卡死记录标记为失败: %s", finding.identifier)

    # ---------------------------------------------------------------- 内部工具

    async def _update_json_file(
        self,
        directory: Path,
        identifier: str,
        updater: Callable[[dict], bool],
    ) -> Optional[str]:
        """原子地「读-改-写」单个 JSON 文件。

        - 锁与业务 Manager 使用同一把资源锁（``get_async_lock`` 按规范化路径缓存），
          因此不会与业务写入互相覆盖；
        - ``updater`` 返回 False 表示无需落盘；
        - 返回中文错误说明，成功返回 ``None``。
        """

        if not is_safe_identifier(identifier):
            return f"非法 ID，已跳过: {identifier!r}"

        file_path = directory / f"{identifier}.json"

        async with get_async_lock(directory):
            data = await read_json_lenient_async(file_path, default=None)
            if not isinstance(data, dict):
                return f"JSON 不存在或无法解析，已跳过: {file_path}"

            if not updater(data):
                return None

            await write_json_atomic_async(file_path, data)

        return None

    def _iter_json_files(self, directory: Path) -> List[Path]:
        """列出目录下的 ``*.json``（目录不存在时返回空列表）。"""

        try:
            if not directory.is_dir():
                return []
            return sorted(path for path in directory.glob("*.json") if path.is_file())
        except OSError:
            return []

    def _iter_files(self, directory: Path) -> List[Path]:
        """递归列出目录下的所有普通文件（目录不存在时返回空列表）。"""

        try:
            if not directory.is_dir():
                return []
            return sorted(path for path in directory.rglob("*") if path.is_file())
        except OSError:
            return []

    def _load_json_dict(self, path: Path) -> Optional[dict]:
        """读取 JSON 顶层对象；损坏或非对象一律返回 ``None``（扫描需容错）。"""

        try:
            with open(path, "r", encoding="utf-8") as handle:
                data = json.load(handle)
        except (OSError, ValueError):
            return None
        return data if isinstance(data, dict) else None

    def _safe_size(self, path: Path) -> int:
        """取文件大小，取不到按 0 处理。"""

        try:
            return int(path.stat().st_size)
        except OSError:
            return 0

    def _resolve_data_path(self, raw_path: str) -> Path:
        """把 JSON 里的相对路径解析成绝对路径。

        历史数据里的音频路径写法不统一（既有 ``data\\audio\\records\\x.wav`` 这种
        「相对项目根」的旧写法，也可能有 ``audio/records/x.wav`` 这种「相对 data/」的写法），
        这里按「谁真实存在就用谁」的顺序依次尝试，全都命中不了时退回「相对 data/」，
        保证判定结果可预期，也不会因为进程工作目录不同而误判。
        """

        candidate = Path(raw_path)
        if candidate.is_absolute():
            return candidate

        options = (
            candidate,                       # 相对进程工作目录（如 data\audio\records\x.wav）
            self.data_path / candidate,      # 相对 data/
        )
        for option in options:
            try:
                if option.is_file():
                    return option
            except OSError:
                continue

        return self.data_path / candidate


# --------------------------------------------------------------------------- 启动钩子


def log_report(report: HousekeepingReport, *, target: Optional[logging.Logger] = None) -> None:
    """把报告摘要写进日志（有明细时补一行计数）。"""

    output = target or logger
    output.info("数据卫生体检完成 —— %s", report.summary())
    if report.errors:
        output.warning("数据卫生体检发现 %d 条跳过/异常说明", len(report.errors))
    if report.has_findings() and report.dry_run:
        output.info(
            "数据卫生体检为只读模式，未删除任何文件；如需清理请执行 "
            "`uv run python scripts/housekeeping.py --apply`"
        )


def run_startup_scan(data_path: Optional[Path] = None) -> HousekeepingReport:
    """同步版启动体检：**永远只读**，只扫描 + 写日志。"""

    report = Housekeeping(data_path).scan()
    report.dry_run = True
    log_report(report)
    return report


async def run_startup_housekeeping(data_path: Optional[Path] = None) -> HousekeepingReport:
    """lifespan 启动钩子：在线程池里跑一次只读体检并写日志。

    设计上刻意不接受 ``dry_run`` 参数、也不读 ``ATT_HOUSEKEEPING_APPLY``：
    启动路径必须绝对安全，任何破坏性动作只能由 CLI ``--apply`` 显式触发。
    """

    return await asyncio.to_thread(run_startup_scan, data_path)
