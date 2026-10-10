"""
Pytest configuration and fixtures
"""
import pytest
import asyncio
from pathlib import Path
import tempfile
import shutil
import uuid


@pytest.fixture(scope="session")
def event_loop():
    """Create event loop for async tests"""
    loop = asyncio.get_event_loop_policy().new_event_loop()
    yield loop
    loop.close()


@pytest.fixture
def temp_dir():
    """Create a temporary directory for tests"""
    base_dir = Path(__file__).resolve().parent / ".tmp"
    base_dir.mkdir(parents=True, exist_ok=True)
    dir_path = base_dir / str(uuid.uuid4())
    dir_path.mkdir(parents=True, exist_ok=True)
    yield dir_path
    shutil.rmtree(dir_path, ignore_errors=True)


@pytest.fixture(autouse=True)
def isolate_factory_user_settings(monkeypatch):
    """测试不得读取开发者本机的 data/settings.json。

    `app.stt.factory` 在 import 期就绑定了 `app.config.user_settings`，而
    `get_asr_provider_config()` 会把用户设置合并进运行时配置；豆包密钥的优先级又是
    「用户设置（密文）> runtime 参数 > 环境变量」。本机一旦在设置页保存过 Token，
    相关断言就会随环境漂移（曾实际踩到），因此这里统一隔离成空实现。
    """
    from app.stt import factory as factory_module

    class _EmptyUserSettings:
        def get_all(self) -> dict:
            return {}

    monkeypatch.setattr(factory_module, "user_settings", _EmptyUserSettings())


@pytest.fixture
def sample_audio(temp_dir):
    """Create a sample audio file for testing"""
    from pydub import AudioSegment

    # Create 1 second of silence
    silence = AudioSegment.silent(duration=1000)
    audio_path = temp_dir / "test.wav"
    silence.export(str(audio_path), format="wav")

    return audio_path
