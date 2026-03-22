"""
Pytest configuration and fixtures
"""
import pytest
import asyncio
from pathlib import Path
import tempfile
import shutil


@pytest.fixture(scope="session")
def event_loop():
    """Create event loop for async tests"""
    loop = asyncio.get_event_loop_policy().new_event_loop()
    yield loop
    loop.close()


@pytest.fixture
def temp_dir():
    """Create a temporary directory for tests"""
    dir_path = Path(tempfile.mkdtemp())
    yield dir_path
    shutil.rmtree(dir_path, ignore_errors=True)


@pytest.fixture
def sample_audio(temp_dir):
    """Create a sample audio file for testing"""
    from pydub import AudioSegment

    # Create 1 second of silence
    silence = AudioSegment.silent(duration=1000)
    audio_path = temp_dir / "test.wav"
    silence.export(str(audio_path), format="wav")

    return audio_path
