#!/usr/bin/env python3
"""
Basic test script for the Audio-to-Text API
"""
import asyncio
import sys
from pathlib import Path

# Add parent directory to path
sys.path.insert(0, str(Path(__file__).parent.parent))

from app.audio.processor import AudioProcessor
from app.stt.transcriber import WhisperTranscriber
from app.records.manager import RecordManager
from app.config import settings


async def test_audio_processor():
    """Test audio processor"""
    print("\n=== Testing Audio Processor ===")

    processor = AudioProcessor()

    # Create a test audio file (silence)
    from pydub import AudioSegment
    silence = AudioSegment.silent(duration=5000)  # 5 seconds
    test_path = Path("test_audio.wav")
    silence.export(str(test_path), format="wav")

    # Read and validate
    with open(test_path, "rb") as f:
        content = f.read()

    metadata = await processor.validate_audio(content, "test_audio.wav")
    print(f"✓ Audio validated: {metadata.duration}s, {metadata.format}")

    # Cleanup
    test_path.unlink()
    print("✓ Audio processor test passed")


async def test_record_manager():
    """Test record manager"""
    print("\n=== Testing Record Manager ===")

    manager = RecordManager(settings.data_path)
    await manager.initialize()

    # Create record
    record = await manager.create_record(
        audio_path="test/path.wav",
        duration=5.0
    )
    print(f"✓ Record created: {record.id}")

    # Update record
    updated = await manager.update_record(
        record.id,
        raw_text="这是一个测试文本。"
    )
    print(f"✓ Record updated: {updated.raw_text}")

    # List records
    records = await manager.list_records()
    print(f"✓ Listed {len(records)} records")

    # Cleanup
    await manager.delete_record(record.id)
    print("✓ Record manager test passed")


async def main():
    """Run all tests"""
    print("Running basic tests...")

    try:
        await test_audio_processor()
        await test_record_manager()

        print("\n=== All tests passed! ===")
        print("\nNote: Whisper model test skipped (requires model download)")
        print("Run: python scripts/download_models.py --model large-v3")

    except Exception as e:
        print(f"\n✗ Test failed: {e}")
        import traceback
        traceback.print_exc()
        sys.exit(1)


if __name__ == "__main__":
    asyncio.run(main())
