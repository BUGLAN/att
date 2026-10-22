"""安全封堵回归测试（审计报告 §4.1 / §4.2）。

覆盖：
- 任意文件读取被拒：绝对路径 + ``..`` 穿越（含 HTTP 层真实复现路径）；
- 任意 ``*.json`` 越界删除被拒：``..%5C..%5Cprobe`` 变体 + 直接调用 manager 的多种变体；
- ``audio_path`` 越界时不会被 unlink；
- ``settings.json`` 顶层为 ``null`` / ``[]`` / ``"str"`` 时后端仍能正常读取配置；
- CORS 白名单不再对任意来源回显。
"""
import asyncio
import json
from contextlib import contextmanager
from pathlib import Path

import pytest
from fastapi import FastAPI
from fastapi.testclient import TestClient

from app.clipboard.manager import ClipboardManager
from app.clipboard.models import ClipboardItemType
from app.config import UserSettings
from app.records.manager import RecordManager
from app.records.session_manager import SessionManager


@contextmanager
def _api_client(record_manager=None, clipboard_manager=None):
    """挂载真实路由构造测试客户端（不触发 lifespan，避免触碰真实 data/）。"""

    import app.api.routes as api_routes
    import app.clipboard.routes as clipboard_routes

    saved_record_manager = api_routes.record_manager
    saved_clipboard_manager = clipboard_routes.clipboard_manager

    try:
        if record_manager is not None:
            api_routes.init_processors(None, None, None, record_manager)  # type: ignore[arg-type]
        if clipboard_manager is not None:
            clipboard_routes.init_clipboard_manager(clipboard_manager)

        test_app = FastAPI()
        test_app.include_router(api_routes.router)
        test_app.include_router(clipboard_routes.router)
        yield TestClient(test_app)
    finally:
        api_routes.record_manager = saved_record_manager
        clipboard_routes.clipboard_manager = saved_clipboard_manager


def _new_clipboard_manager(root: Path) -> ClipboardManager:
    async def setup():
        manager = ClipboardManager(root)
        await manager.initialize()
        return manager

    return asyncio.run(setup())


def _new_record_manager(root: Path) -> RecordManager:
    async def setup():
        manager = RecordManager(root)
        await manager.initialize()
        return manager

    return asyncio.run(setup())


# ------------------------------------------------------------------ 任意文件读取

def test_clipboard_image_rejects_absolute_path_outside_images(tmp_path: Path):
    """审计报告 §4.1 缺陷 1 的直连复现：绝对路径不得被当成图片返回。"""

    secret = tmp_path / "probe_secret.env"
    secret.write_text("DEEPSEEK_API_KEY=real-key", encoding="utf-8")

    data_root = tmp_path / "data"

    async def run():
        manager = ClipboardManager(data_root)
        await manager.initialize()

        item = await manager.create_item(
            content="image:absolute-probe",
            item_type=ClipboardItemType.IMAGE,
            metadata={"image_path": str(secret)},
        )
        assert await manager.get_item_image_path(item.id) is None

    asyncio.run(run())


@pytest.mark.parametrize(
    "raw_image_path",
    [
        "images/../../probe_secret.png",
        "..\\..\\probe_secret.png",
        "images/..\\..\\probe_secret.png",
        "data/clipboard/images/../../../../probe_secret.png",
        "images/.././../probe_secret.png",
    ],
)
def test_clipboard_image_rejects_traversal_variants(tmp_path: Path, raw_image_path: str):
    data_root = tmp_path / "data"
    images_dir = data_root / "clipboard" / "images"
    images_dir.mkdir(parents=True, exist_ok=True)

    # 越界目标：data/ 上一级的探针文件
    probe = tmp_path / "probe_secret.png"
    probe.write_bytes(b"secret-image-bytes")

    async def run():
        manager = ClipboardManager(data_root)
        await manager.initialize()
        item = await manager.create_item(
            content=f"image:{raw_image_path}",
            item_type=ClipboardItemType.IMAGE,
            metadata={"image_path": raw_image_path, "stored_file_name": ""},
        )
        assert await manager.get_item_image_path(item.id) is None

    asyncio.run(run())


def test_clipboard_image_http_endpoint_does_not_leak_arbitrary_file(tmp_path: Path):
    """原始复现路径：POST 造条目 → GET /image 不能再返回任意文件内容。"""

    secret = tmp_path / "probe_secret.env"
    secret.write_text("DEEPSEEK_API_KEY=real-key", encoding="utf-8")

    data_root = tmp_path / "data"
    manager = _new_clipboard_manager(data_root)

    with _api_client(clipboard_manager=manager) as client:
        created = client.post(
            "/api/clipboard/items",
            json={
                "content": "image:http-probe",
                "item_type": "image",
                "metadata": {"image_path": str(secret)},
            },
        )
        assert created.status_code == 200
        item_id = created.json()["id"]

        response = client.get(f"/api/clipboard/items/{item_id}/image")

    assert response.status_code == 404
    assert "DEEPSEEK_API_KEY" not in response.text


def test_clipboard_image_still_resolves_relative_and_legacy_inside_paths(tmp_path: Path):
    """正常流程不能被安全封堵误伤：相对路径与历史绝对路径（位于 images/ 内）都要能用。"""

    data_root = tmp_path / "data"
    images_dir = data_root / "clipboard" / "images"
    images_dir.mkdir(parents=True, exist_ok=True)
    image_file = images_dir / "clipboard-image.png"
    image_file.write_bytes(b"fake-image")

    async def run():
        manager = ClipboardManager(data_root)
        await manager.initialize()

        relative_item = await manager.create_item(
            content="image:relative",
            item_type=ClipboardItemType.IMAGE,
            metadata={"image_path": "images/clipboard-image.png"},
        )
        assert await manager.get_item_image_path(relative_item.id) == image_file

        legacy_absolute_item = await manager.create_item(
            content="image:legacy-absolute",
            item_type=ClipboardItemType.IMAGE,
            metadata={"image_path": str(image_file)},
        )
        assert await manager.get_item_image_path(legacy_absolute_item.id) == image_file

        stored_name_item = await manager.create_item(
            content="image:stored-name",
            item_type=ClipboardItemType.IMAGE,
            metadata={"image_path": "", "stored_file_name": "clipboard-image.png"},
        )
        assert await manager.get_item_image_path(stored_name_item.id) == image_file

    asyncio.run(run())


# ------------------------------------------------------------------ 越界删除

def test_record_http_delete_rejects_percent5c_traversal(tmp_path: Path):
    """审计报告 §4.1 缺陷 2 的原始复现：DELETE /api/records/..%5C..%5Cprobe → 必须被拒。"""

    data_root = tmp_path / "data"
    manager = _new_record_manager(data_root)

    probe = tmp_path / "probe_del.json"
    probe.write_text(
        json.dumps(
            {
                "id": "probe_del",
                "created_at": "2026-01-01T00:00:00",
                "updated_at": "2026-01-01T00:00:00",
                "audio_path": "",
                "duration": 0.0,
                "raw_text": "probe",
                "optimized_text": "",
                "status": "pending",
                "processing_time": 0.0,
                "is_modified": False,
                "is_favorite": False,
            },
            ensure_ascii=False,
            indent=2,
        ),
        encoding="utf-8",
    )

    with _api_client(record_manager=manager) as client:
        encoded = client.delete("/api/records/..%5C..%5Cprobe_del")
        slash_variant = client.delete("/api/records/..%2F..%2Fprobe_del")

    assert encoded.status_code == 404, encoded.text
    assert slash_variant.status_code != 200
    assert probe.exists(), "越界删除必须失败，探针文件不能被删掉"


@pytest.mark.parametrize(
    "record_id",
    [
        "..\\..\\probe_del",
        "../../probe_del",
        "..%5C..%5Cprobe_del",
        "sub/probe_del",
        "sub\\probe_del",
        "..",
        ".",
        "",
        "C:\\Windows\\probe_del",
    ],
)
def test_record_manager_delete_rejects_unsafe_ids(tmp_path: Path, record_id: str):
    data_root = tmp_path / "data"
    manager = _new_record_manager(data_root)

    probe = tmp_path / "probe_del.json"
    probe.write_text("{}", encoding="utf-8")

    deleted = asyncio.run(manager.delete_record(record_id))

    assert deleted is False
    assert probe.exists()


def test_record_manager_read_rejects_unsafe_ids(tmp_path: Path):
    data_root = tmp_path / "data"
    manager = _new_record_manager(data_root)

    assert asyncio.run(manager.get_record("..\\..\\settings")) is None
    assert asyncio.run(manager.get_record("../../settings")) is None

    with pytest.raises(ValueError):
        manager.get_audio_file_path("..\\..\\x")


def test_record_delete_only_unlinks_audio_inside_records_dir(tmp_path: Path):
    """JSON 里的 audio_path 越界时不得 unlink；在界内时正常删除。"""

    data_root = tmp_path / "data"
    manager = _new_record_manager(data_root)

    outside_audio = tmp_path / "outside.wav"
    outside_audio.write_bytes(b"outside")
    inside_audio = data_root / "audio" / "records" / "inside.wav"
    inside_audio.write_bytes(b"inside")

    async def run():
        outside_record = await manager.create_record(audio_path=str(outside_audio), duration=1.0)
        assert await manager.delete_record(outside_record.id) is True
        assert outside_audio.exists(), "越界 audio_path 不能被删除"

        inside_record = await manager.create_record(audio_path=str(inside_audio), duration=1.0)
        assert await manager.delete_record(inside_record.id) is True
        assert not inside_audio.exists(), "在界内的音频文件应正常删除"

    asyncio.run(run())


# ------------------------------------------------------------------ 会话空音频

def test_complete_session_does_not_persist_empty_merged_audio_path(tmp_path: Path):
    """审计报告 §4.2 缺陷 5：不再写入 audio_path=""/merged_audio_path=""。"""

    data_root = tmp_path / "data"
    session_manager = SessionManager(str(data_root))

    async def run():
        session = await session_manager.create_session()
        completed = await session_manager.complete_session(session.id)
        assert completed is not None
        assert completed.merged_audio_path == ""

        stored = json.loads((data_root / "sessions" / f"{session.id}.json").read_text(encoding="utf-8"))
        assert stored["merged_audio_path"] == ""
        assert stored["segments"] == []

    asyncio.run(run())


def test_session_manager_delete_rejects_unsafe_id_and_outside_audio(tmp_path: Path):
    data_root = tmp_path / "data"
    session_manager = SessionManager(str(data_root))

    probe = tmp_path / "probe_session.json"
    probe.write_text("{}", encoding="utf-8")

    assert asyncio.run(session_manager.delete_session("..\\..\\probe_session")) is False
    assert probe.exists()

    outside_audio = tmp_path / "outside-session.wav"
    outside_audio.write_bytes(b"outside")

    async def run():
        session = await session_manager.create_session()
        segment = await session_manager.add_segment(session.id, str(outside_audio), 1.0)
        assert segment.audio_path == str(outside_audio)
        assert await session_manager.delete_session(session.id) is True

    asyncio.run(run())
    assert outside_audio.exists(), "会话音频越界时不得删除"


# ------------------------------------------------------------------ 配置形状容错

@pytest.mark.parametrize("raw", ["null", "[]", '"str"', "42"])
def test_config_module_import_survives_broken_settings_file(tmp_path: Path, monkeypatch, raw: str):
    """真实失败模式回归：`import app.config` 期间不能因为 settings.json 形状非法而崩。

    旧实现会在 `{**DEFAULT, **loaded}` 处抛 TypeError，后端直接起不来。
    """

    import importlib.util

    data_dir = tmp_path / "data"
    data_dir.mkdir()
    (data_dir / "settings.json").write_text(raw, encoding="utf-8")

    monkeypatch.chdir(tmp_path)

    config_path = Path(__file__).resolve().parent.parent / "app" / "config.py"
    spec = importlib.util.spec_from_file_location("att_config_probe", config_path)
    assert spec and spec.loader
    module = importlib.util.module_from_spec(spec)

    spec.loader.exec_module(module)  # 旧实现会在这里抛 TypeError

    assert module.user_settings.get("asr_provider") == "whisper"
    assert module.user_settings.get("hotkey")
    assert module.settings.data_path == Path("data")


@pytest.mark.parametrize("raw", ["null", "[]", '"str"', "42", "true", "{ not json", ""])
def test_user_settings_tolerates_invalid_shapes(tmp_path: Path, raw: str):
    """settings.json 内容形状非法时，UserSettings 仍必须能加载出完整默认值。"""

    settings_file = tmp_path / "settings.json"
    settings_file.write_text(raw, encoding="utf-8")

    user_settings = UserSettings(settings_file)
    loaded = user_settings.get_all()

    assert isinstance(loaded, dict)
    assert loaded["asr_provider"] == UserSettings.DEFAULT_SETTINGS["asr_provider"]
    assert loaded["hotkey"], "hotkey 必须被归一化成可注册的默认值"
    assert "recording_mode" in loaded


def test_user_settings_public_api_and_atomic_write(tmp_path: Path):
    settings_file = tmp_path / "settings.json"

    user_settings = UserSettings(settings_file)
    assert user_settings.get("asr_provider") == "whisper"
    assert user_settings.get("missing_key", "fallback") == "fallback"

    user_settings.set("asr_language", "en")
    assert user_settings.get("asr_language") == "en"

    user_settings.update({"auto_optimize": True, "recording_mode": "multi"})
    all_settings = user_settings.get_all()
    assert all_settings["auto_optimize"] is True
    assert all_settings["recording_mode"] == "multi"
    assert all_settings["asr_language"] == "en"

    # 落盘格式保持 indent=2 / ensure_ascii=False，且没有半截临时文件
    raw = settings_file.read_text(encoding="utf-8")
    assert raw == json.dumps(all_settings, ensure_ascii=False, indent=2)
    assert not (tmp_path / "settings.json.tmp").exists()

    # 重新加载仍然一致
    assert UserSettings(settings_file).get_all() == all_settings


def test_llm_store_tolerates_invalid_shapes(tmp_path: Path):
    from app.llm.store import LLMStore

    config_file = tmp_path / "llm_v2.json"
    config_file.write_text("null", encoding="utf-8")

    store = LLMStore(data_dir=str(tmp_path))
    assert store.list_profiles() == []
    assert store.list_models() == []
    assert store.get_default_model() is None


def test_cors_whitelist_allows_electron_and_blocks_foreign_origins():
    """Electron（file:// → Origin: null、dev :8090）可用，任意站点不再被回显。"""

    from app.main import app as main_app

    client = TestClient(main_app)

    for origin in ("http://localhost:8090", "http://127.0.0.1:8090", "null"):
        response = client.get("/", headers={"Origin": origin})
        assert response.status_code == 200
        assert response.headers.get("access-control-allow-origin") == origin
        assert "access-control-allow-credentials" not in response.headers

    blocked = client.get("/", headers={"Origin": "https://evil.example"})
    assert "access-control-allow-origin" not in blocked.headers
