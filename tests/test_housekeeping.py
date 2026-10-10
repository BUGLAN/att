"""数据卫生（housekeeping）与记录错误字段的回归测试。

覆盖点：
1. ``scan()`` 能识别四类问题（孤儿图片 / 残留片段音频 / 卡死会话 / 卡在中间态的记录）；
2. 默认只读：不删文件、不改状态、不改 JSON；
3. ``dry_run=False`` / ``ATT_HOUSEKEEPING_APPLY=1`` 才真正清理，且删除范围受目录包含性校验约束；
4. 启动钩子 ``run_startup_scan`` 永远只读（环境变量也无法让它动数据）；
5. ``Record.error`` 双向兼容 + ``RecordManager.set_error_message`` 持久化。

所有用例都在 ``tmp_path`` 造数据，绝不触碰真实 ``data/``。
"""
import asyncio
import json
import os
from datetime import datetime, timedelta
from pathlib import Path

import pytest

from app.records.manager import RecordManager
from app.records.models import Record
from app.repositories.housekeeping import (
    APPLY_ENV,
    CATEGORY_ORPHAN_IMAGE,
    CATEGORY_STALE_SEGMENT,
    CATEGORY_STUCK_RECORD,
    CATEGORY_STUCK_SESSION,
    Finding,
    Housekeeping,
    resolve_dry_run,
    run_startup_scan,
)

# 内存文件快照：路径 -> 内容（用于断言「什么都没变」）
Snapshot = dict


def _iso(hours_ago: float) -> str:
    """生成 N 小时前的时间字符串。"""

    return (datetime.now() - timedelta(hours=hours_ago)).isoformat()


def _write_json(path: Path, data: dict) -> Path:
    """写 JSON（测试数据结构与真实 data/ 保持一致：utf-8 + 缩进 2）。"""

    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(data, ensure_ascii=False, indent=2), encoding="utf-8")
    return path


def _write_bytes(path: Path, content: bytes = b"x") -> Path:
    """写二进制文件（模拟图片/音频）。"""

    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(content)
    return path


def _make_old(path: Path, hours: float = 48.0) -> Path:
    """把文件的 mtime 调到 N 小时前（孤儿图片有最小年龄门槛）。"""

    stamp = (datetime.now() - timedelta(hours=hours)).timestamp()
    os.utime(path, (stamp, stamp))
    return path


def _snapshot(data_dir: Path) -> Snapshot:
    """记录目录下所有文件的相对路径与内容，用于断言只读。"""

    result: Snapshot = {}
    for path in sorted(data_dir.rglob("*")):
        if path.is_file():
            result[str(path.relative_to(data_dir))] = path.read_bytes()
    return result


def _session_payload(session_id: str, *, status: str, created_hours_ago: float, segments=None, merged="") -> dict:
    """构造与 RecordingSession.to_dict() 同形状的会话 JSON。"""

    return {
        "id": session_id,
        "mode": "multi",
        "status": status,
        "segments": segments or [],
        "total_duration": 0.0,
        "merged_text": "",
        "merged_audio_path": merged,
        "created_at": _iso(created_hours_ago),
        "completed_at": None,
    }


def _record_payload(record_id: str, *, status: str, updated_hours_ago: float) -> dict:
    """构造与 Record.to_dict() 同形状（含新增 error 字段）的记录 JSON。"""

    return {
        "id": record_id,
        "created_at": _iso(updated_hours_ago + 1),
        "updated_at": _iso(updated_hours_ago),
        "audio_path": "",
        "duration": 1.0,
        "raw_text": "",
        "optimized_text": "",
        "status": status,
        "processing_time": 0.0,
        "is_modified": False,
        "is_favorite": False,
        "error": "",
    }


@pytest.fixture
def data_dir(tmp_path: Path) -> Path:
    """造一份最小可用的 data/ 目录（含四类问题各若干）。"""

    root = tmp_path / "data"

    # ---- 剪贴板：1 张被引用图片 + 1 张孤儿图片（都足够老，越过最小年龄门槛）
    kept_image = _make_old(_write_bytes(root / "clipboard" / "images" / "kept.png", b"kept"))
    _make_old(_write_bytes(root / "clipboard" / "images" / "orphan.png", b"orphan"))
    _write_json(
        root / "clipboard" / "item-1.json",
        {
            "id": "item-1",
            "created_at": _iso(50),
            "updated_at": _iso(50),
            "item_type": "image",
            "content": "image:kept",
            "title": "粘贴图片",
            "summary": "",
            "source": "manual",
            "is_favorite": False,
            "is_pinned": False,
            "content_size": 4,
            "duplicate_count": 1,
            "content_hash": "",
            "metadata": {
                "image_path": str(kept_image),
                "stored_file_name": kept_image.name,
                "file_name": "随便起的名字.png",
            },
        },
    )

    # ---- 会话 1：已完成且合并音频已落盘 → 两个片段音频是残留
    merged_audio = _write_bytes(root / "audio" / "records" / "session-1.wav", b"merged")
    segment_audio_one = _write_bytes(root / "audio" / "records" / "seg-1.wav", b"seg1")
    segment_audio_two = _write_bytes(root / "audio" / "records" / "seg-2.wav", b"seg2")
    _write_json(
        root / "sessions" / "session-1.json",
        _session_payload(
            "session-1",
            status="completed",
            created_hours_ago=30,
            merged=str(merged_audio),
            segments=[
                {"id": "seg-1", "session_id": "session-1", "sequence": 0, "audio_path": str(segment_audio_one),
                 "duration": 1.0, "start_time": _iso(30), "end_time": _iso(30),
                 "status": "transcribed", "raw_text": "a", "processing_time": 0.1},
                {"id": "seg-2", "session_id": "session-1", "sequence": 1, "audio_path": str(segment_audio_two),
                 "duration": 1.0, "start_time": _iso(30), "end_time": _iso(30),
                 "status": "transcribed", "raw_text": "b", "processing_time": 0.1},
            ],
        ),
    )

    # ---- 会话 2：卡在 recording 48 小时
    _write_json(
        root / "sessions" / "session-2.json",
        _session_payload("session-2", status="recording", created_hours_ago=48),
    )

    # ---- 会话 3：刚刚开始录音，不应被判为卡死
    _write_json(
        root / "sessions" / "session-3.json",
        _session_payload("session-3", status="recording", created_hours_ago=0.1),
    )

    # ---- 记录：卡在 optimizing 48 小时 / 卡在 transcribing 48 小时 / 新鲜中间态 / 正常完成
    _write_json(root / "records" / "rec-stuck-optimizing.json",
                _record_payload("rec-stuck-optimizing", status="optimizing", updated_hours_ago=48))
    _write_json(root / "records" / "rec-stuck-transcribing.json",
                _record_payload("rec-stuck-transcribing", status="transcribing", updated_hours_ago=48))
    _write_json(root / "records" / "rec-fresh-optimizing.json",
                _record_payload("rec-fresh-optimizing", status="optimizing", updated_hours_ago=0.1))
    _write_json(root / "records" / "rec-done.json",
                _record_payload("rec-done", status="transcribed", updated_hours_ago=48))

    return root


# --------------------------------------------------------------------- 只读扫描


def test_scan_detects_all_four_categories(data_dir: Path):
    """scan() 能识别四类问题，且给出结构化的计数与明细。"""

    report = Housekeeping(data_dir).scan()

    assert report.dry_run is True
    assert report.count_of(CATEGORY_ORPHAN_IMAGE) == 1
    assert report.count_of(CATEGORY_STALE_SEGMENT) == 2
    assert report.count_of(CATEGORY_STUCK_SESSION) == 1
    assert report.count_of(CATEGORY_STUCK_RECORD) == 2
    assert report.errors == []

    # 明细里带上业务 ID，方便人工核对
    assert report.items_of(CATEGORY_STUCK_SESSION)[0].identifier == "session-2"
    assert {item.identifier for item in report.items_of(CATEGORY_STUCK_RECORD)} == {
        "rec-stuck-optimizing",
        "rec-stuck-transcribing",
    }
    # 陈旧段明细节点为对应会话
    assert report.items_of(CATEGORY_STALE_SEGMENT)[0].identifier == "session-1"
    # 结构化报告可直接 JSON 序列化
    payload = report.to_dict()
    assert payload["counts"][CATEGORY_ORPHAN_IMAGE] == 1
    assert payload["categories"][CATEGORY_STALE_SEGMENT]["truncated"] is False


def test_scan_does_not_change_anything(data_dir: Path):
    """只读扫描前后，data/ 下所有文件的内容与集合完全一致。"""

    before = _snapshot(data_dir)

    Housekeeping(data_dir).scan()

    assert _snapshot(data_dir) == before


def test_run_defaults_to_dry_run(data_dir: Path, monkeypatch: pytest.MonkeyPatch):
    """未显式指定且未设置环境变量时，run() 必须是只读。"""

    monkeypatch.delenv(APPLY_ENV, raising=False)
    before = _snapshot(data_dir)

    report = asyncio.run(Housekeeping(data_dir).run())

    assert report.dry_run is True
    assert sum(report.applied.values()) == 0
    assert _snapshot(data_dir) == before


def test_explicit_dry_run_true_wins_over_env(data_dir: Path, monkeypatch: pytest.MonkeyPatch):
    """显式 dry_run=True 时，即使环境变量要求执行也保持只读。"""

    monkeypatch.setenv(APPLY_ENV, "1")
    before = _snapshot(data_dir)

    report = asyncio.run(Housekeeping(data_dir).run(dry_run=True))

    assert report.dry_run is True
    assert _snapshot(data_dir) == before


def test_resolve_dry_run_semantics(monkeypatch: pytest.MonkeyPatch):
    """三态参数收敛规则：None 看环境变量，True/False 直接生效。"""

    monkeypatch.delenv(APPLY_ENV, raising=False)
    assert resolve_dry_run(None) is True
    assert resolve_dry_run(True) is True
    assert resolve_dry_run(False) is False

    monkeypatch.setenv(APPLY_ENV, "yes")
    assert resolve_dry_run(None) is False
    assert resolve_dry_run(True) is True


# --------------------------------------------------------------------- 执行清理


def test_apply_cleans_orphans_segments_and_stuck_states(data_dir: Path):
    """dry_run=False 时：删孤儿图片/残留片段，卡死状态标记为失败，正常数据不动。"""

    report = asyncio.run(Housekeeping(data_dir).run(dry_run=False))

    assert report.dry_run is False
    assert report.errors == []
    assert report.applied[CATEGORY_ORPHAN_IMAGE] == 1
    assert report.applied[CATEGORY_STALE_SEGMENT] == 2
    assert report.applied[CATEGORY_STUCK_SESSION] == 1
    assert report.applied[CATEGORY_STUCK_RECORD] == 2

    # 孤儿图片被删，被引用图片保留
    assert not (data_dir / "clipboard" / "images" / "orphan.png").exists()
    assert (data_dir / "clipboard" / "images" / "kept.png").exists()

    # 合并音频保留；残留片段音频被删，会话 JSON 里对应片段路径被清空
    assert (data_dir / "audio" / "records" / "session-1.wav").exists()
    assert not (data_dir / "audio" / "records" / "seg-1.wav").exists()
    assert not (data_dir / "audio" / "records" / "seg-2.wav").exists()
    session_one = json.loads((data_dir / "sessions" / "session-1.json").read_text(encoding="utf-8"))
    assert session_one["status"] == "completed"
    assert [segment["audio_path"] for segment in session_one["segments"]] == ["", ""]
    # 片段文本等业务字段不能被清理动作丢掉
    assert [segment["raw_text"] for segment in session_one["segments"]] == ["a", "b"]

    # 卡死会话被标记为失败并补完成时间；新会话不受影响
    session_two = json.loads((data_dir / "sessions" / "session-2.json").read_text(encoding="utf-8"))
    assert session_two["status"] == "failed"
    assert session_two["completed_at"]
    session_three = json.loads((data_dir / "sessions" / "session-3.json").read_text(encoding="utf-8"))
    assert session_three["status"] == "recording"

    # 卡住记录标记为失败并持久化失败原因；新鲜中间态与已完成记录不动
    stuck_optimizing = json.loads((data_dir / "records" / "rec-stuck-optimizing.json").read_text(encoding="utf-8"))
    assert stuck_optimizing["status"] == "failed"
    assert stuck_optimizing["error"]
    # 失败原因里写明真实停留时长，便于用户判断是否需要重试
    assert "48" in stuck_optimizing["error"]
    stuck_transcribing = json.loads(
        (data_dir / "records" / "rec-stuck-transcribing.json").read_text(encoding="utf-8")
    )
    assert stuck_transcribing["status"] == "failed"
    fresh = json.loads((data_dir / "records" / "rec-fresh-optimizing.json").read_text(encoding="utf-8"))
    assert fresh["status"] == "optimizing"
    assert fresh["error"] == ""
    done = json.loads((data_dir / "records" / "rec-done.json").read_text(encoding="utf-8"))
    assert done["status"] == "transcribed"


def test_env_var_enables_apply(data_dir: Path, monkeypatch: pytest.MonkeyPatch):
    """ATT_HOUSEKEEPING_APPLY=1 时，run() 默认就执行清理。"""

    monkeypatch.setenv(APPLY_ENV, "1")

    report = asyncio.run(Housekeeping(data_dir).run())

    assert report.dry_run is False
    assert not (data_dir / "clipboard" / "images" / "orphan.png").exists()


def test_apply_skips_paths_outside_data_dir(data_dir: Path, tmp_path: Path):
    """即使报告里被塞进越界路径，apply() 也必须拒绝删除并记录原因。"""

    outside = _make_old(_write_bytes(tmp_path / "outside.png"))

    housekeeping = Housekeeping(data_dir)
    report = housekeeping.scan()
    report.dry_run = False
    report.findings.append(
        Finding(category=CATEGORY_ORPHAN_IMAGE, path=str(outside), detail="人为构造的越界路径")
    )

    asyncio.run(housekeeping.apply(report))

    assert outside.exists()
    assert report.applied[CATEGORY_ORPHAN_IMAGE] == 1  # 只有真正的孤儿图片被删
    assert any("越界" in message for message in report.errors)


def test_apply_refuses_dry_run_report(data_dir: Path):
    """apply() 不接受只读报告，防止调用方误用默认参数破坏数据。"""

    housekeeping = Housekeeping(data_dir)
    report = housekeeping.scan()

    with pytest.raises(ValueError):
        asyncio.run(housekeeping.apply(report))


# --------------------------------------------------------------------- 启动钩子


def test_startup_scan_is_always_read_only(data_dir: Path, monkeypatch: pytest.MonkeyPatch):
    """启动钩子永远只读：环境变量为 1 也不得删除/改状态。"""

    monkeypatch.setenv(APPLY_ENV, "1")
    before = _snapshot(data_dir)

    report = run_startup_scan(data_dir)

    assert report.dry_run is True
    assert report.has_findings() is True
    assert _snapshot(data_dir) == before


# --------------------------------------------------------------------- Record.error


def test_record_error_field_is_backward_compatible():
    """老 JSON 没有 error 字段时默认空串；新 JSON 双向可读。"""

    legacy = {
        "id": "legacy",
        "created_at": _iso(1),
        "updated_at": _iso(1),
        "audio_path": "",
        "duration": 1.0,
        "raw_text": "",
        "optimized_text": "",
        "status": "optimized",
        "processing_time": 0.0,
        "is_modified": False,
        "is_favorite": False,
    }

    record = Record.from_dict(legacy)
    assert record.error == ""
    assert record.to_dict()["error"] == ""

    record.error = "接口超时"
    restored = Record.from_dict(record.to_dict())
    assert restored.error == "接口超时"


def test_set_error_message_persists_and_clears(data_dir: Path):
    """set_error_message 走原子写持久化；空串清除；记录不存在时不抛异常。"""

    async def run() -> None:
        manager = RecordManager(data_dir)
        await manager.initialize()

        record = await manager.create_record(audio_path="", duration=1.0)

        await manager.set_error_message(record.id, "优化失败：上游 502")
        stored = await manager.get_record(record.id)
        assert stored is not None
        assert stored.error == "优化失败：上游 502"
        # 失败原因确实落盘（而不是只在内存里）
        raw = json.loads((data_dir / "records" / f"{record.id}.json").read_text(encoding="utf-8"))
        assert raw["error"] == "优化失败：上游 502"

        await manager.set_error_message(record.id, "")
        cleared = await manager.get_record(record.id)
        assert cleared is not None
        assert cleared.error == ""

        # 记录不存在：静默返回，不抛异常
        await manager.set_error_message("not-exists", "x")

    asyncio.run(run())


def test_stuck_threshold_is_configurable(data_dir: Path):
    """阈值可配置：把阈值放大到 100 小时后，48 小时的卡死项不再被判为卡死。"""

    report = Housekeeping(data_dir, stuck_after_hours=100.0).scan()

    assert report.count_of(CATEGORY_STUCK_SESSION) == 0
    assert report.count_of(CATEGORY_STUCK_RECORD) == 0
    # 孤儿图片/残留片段与状态阈值无关，仍应被识别
    assert report.count_of(CATEGORY_ORPHAN_IMAGE) == 1
    assert report.count_of(CATEGORY_STALE_SEGMENT) == 2


def test_orphan_min_age_guard_protects_fresh_files(data_dir: Path):
    """刚写入的孤儿图片在最小年龄门槛内不参与清理（避免与捕获流程竞态）。"""

    fresh = _write_bytes(data_dir / "clipboard" / "images" / "fresh.png", b"fresh")

    report = Housekeeping(data_dir, orphan_min_age_hours=24.0).scan()

    assert report.count_of(CATEGORY_ORPHAN_IMAGE) == 1
    assert all(not item.path.endswith("fresh.png") for item in report.items_of(CATEGORY_ORPHAN_IMAGE))
    assert fresh.exists()
