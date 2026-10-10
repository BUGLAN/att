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
from app.records.manager import RecordManager
from app.config import settings


def test_audio_processor(temp_dir):
    """Test audio processor"""

    async def run_test():
        print("\n=== Testing Audio Processor ===")

        processor = AudioProcessor()

        from pydub import AudioSegment
        silence = AudioSegment.silent(duration=5000)
        test_path = temp_dir / "test_audio.wav"
        silence.export(str(test_path), format="wav")

        with open(test_path, "rb") as f:
            content = f.read()

        metadata = await processor.validate_audio(content, "test_audio.wav")
        print(f"✓ Audio validated: {metadata.duration}s, {metadata.format}")

        print("✓ Audio processor test passed")

    asyncio.run(run_test())


def test_record_manager(temp_dir):
    """Test record manager"""

    async def run_test():
        print("\n=== Testing Record Manager ===")

        manager = RecordManager(temp_dir)
        await manager.initialize()

        record = await manager.create_record(
            audio_path="test/path.wav",
            duration=5.0
        )
        print(f"✓ Record created: {record.id}")

        updated = await manager.update_record(
            record.id,
            raw_text="这是一个测试文本。"
        )
        print(f"✓ Record updated: {updated.raw_text}")

        records = await manager.list_records()
        print(f"✓ Listed {len(records)} records")

        print("✓ Record manager test passed")

    asyncio.run(run_test())


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
