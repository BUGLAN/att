"""app/repositories/json_store.py 的回归测试。

覆盖：
- 原子写保持 indent=2 / ensure_ascii=False，且不残留 .tmp；
- 写入中途失败（序列化失败 / os.replace 失败）不会产生半截文件，旧内容完好；
- read_json 严格读与 read_json_lenient 宽容读的差异；
- ensure_dict 对 null / [] / "str" 等非法顶层形状的收敛；
- 按路径的锁注册表（asyncio / threading 两套）。
"""
import asyncio
import json
import os
from pathlib import Path

import pytest

from app.repositories.json_store import (
    ensure_dict,
    get_async_lock,
    get_thread_lock,
    read_json,
    read_json_async,
    read_json_lenient,
    read_json_lenient_async,
    write_json_atomic,
    write_json_atomic_async,
)


def test_write_json_atomic_keeps_readable_format(tmp_path: Path):
    """写盘格式必须是 indent=2 + ensure_ascii=False，且不留下临时文件。"""
    target = tmp_path / "nested" / "sample.json"
    payload = {"title": "中文标题", "items": [1, 2, 3]}

    write_json_atomic(target, payload)

    raw = target.read_text(encoding="utf-8")
    assert "中文标题" in raw, "ensure_ascii=False 必须保留原始中文，不能转义成 \\uXXXX"
    assert raw == json.dumps(payload, ensure_ascii=False, indent=2)
    assert json.loads(raw) == payload

    leftovers = [p.name for p in target.parent.glob("*.tmp")]
    assert leftovers == [], f"原子写不应残留临时文件: {leftovers}"


def test_write_json_atomic_replaces_existing_content(tmp_path: Path):
    target = tmp_path / "sample.json"
    write_json_atomic(target, {"version": 1})
    write_json_atomic(target, {"version": 2})

    assert json.loads(target.read_text(encoding="utf-8")) == {"version": 2}
    assert not (tmp_path / "sample.json.tmp").exists()


def test_write_json_atomic_serialization_failure_keeps_old_content(tmp_path: Path):
    """序列化失败：目标文件保持旧内容，且不产生半截文件。"""
    target = tmp_path / "sample.json"
    write_json_atomic(target, {"ok": True})

    class NotSerializable:
        pass

    with pytest.raises(TypeError):
        write_json_atomic(target, {"bad": NotSerializable()})

    assert json.loads(target.read_text(encoding="utf-8")) == {"ok": True}
    assert not (tmp_path / "sample.json.tmp").exists()


def test_write_json_atomic_replace_failure_keeps_old_content(tmp_path: Path, monkeypatch):
    """替换阶段失败（模拟写入中途异常）：旧内容完好，临时文件被清理。"""
    target = tmp_path / "sample.json"
    write_json_atomic(target, {"ok": "old"})

    real_replace = os.replace

    def broken_replace(src, dst):
        raise OSError("模拟 os.replace 失败")

    monkeypatch.setattr(os, "replace", broken_replace)

    with pytest.raises(OSError):
        write_json_atomic(target, {"ok": "new"})

    monkeypatch.setattr(os, "replace", real_replace)

    assert json.loads(target.read_text(encoding="utf-8")) == {"ok": "old"}
    assert not (tmp_path / "sample.json.tmp").exists()


def test_read_json_strict_vs_lenient(tmp_path: Path):
    missing = tmp_path / "missing.json"
    broken = tmp_path / "broken.json"
    broken.write_text("{ not json", encoding="utf-8")
    null_file = tmp_path / "null.json"
    null_file.write_text("null", encoding="utf-8")

    # 严格读：文件缺失返回 default，内容损坏直接抛异常
    assert read_json(missing, default={"fallback": True}) == {"fallback": True}
    with pytest.raises(json.JSONDecodeError):
        read_json(broken)

    # 宽容读：损坏 / 空值 / 缺失一律回落 default，绝不抛异常
    assert read_json_lenient(broken, default={}) == {}
    assert read_json_lenient(null_file, default={}) == {}
    assert read_json_lenient(missing, default={}) == {}


def test_async_read_write_roundtrip(tmp_path: Path):
    async def run():
        target = tmp_path / "async.json"
        await write_json_atomic_async(target, {"中文": "内容"})
        assert await read_json_async(target) == {"中文": "内容"}
        assert await read_json_lenient_async(tmp_path / "none.json", default={"d": 1}) == {"d": 1}

    asyncio.run(run())


@pytest.mark.parametrize("value", [None, [], "str", 42, True])
def test_ensure_dict_converts_invalid_shapes(value):
    assert ensure_dict(value) == {}


def test_ensure_dict_keeps_valid_dict():
    payload = {"a": 1}
    assert ensure_dict(payload) is payload


def test_ensure_dict_logs_when_shape_invalid():
    class FakeLogger:
        def __init__(self):
            self.messages = []

        def warning(self, message, *args):
            self.messages.append(message % args if args else message)

    logger = FakeLogger()
    assert ensure_dict([], logger=logger, source="settings.json") == {}
    assert logger.messages, "非法形状必须留下 warning，便于排查数据被静默重置"
    assert "settings.json" in logger.messages[0]


def test_async_lock_registry_is_stable_per_path(tmp_path: Path):
    async def run():
        first = get_async_lock(tmp_path / "records")
        second = get_async_lock(tmp_path / "records")
        other = get_async_lock(tmp_path / "clipboard")

        assert first is second, "同一资源路径必须复用同一把锁"
        assert first is not other, "不同资源路径必须使用不同的锁"

        async with first:
            assert first.locked()

        # 锁可重复获取（前一次已释放）
        async with first:
            pass

    asyncio.run(run())


def test_thread_lock_registry_is_reentrant(tmp_path: Path):
    first = get_thread_lock(tmp_path / "settings.json")
    second = get_thread_lock(tmp_path / "settings.json")

    assert first is second
    with first:
        with second:  # 可重入，避免同一调用链自锁死
            assert True
