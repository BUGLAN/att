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
from app.fragments.manager import FragmentManager


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


async def test_fragment_manager():
    """Test fragment manager"""
    print("\n=== Testing Fragment Manager ===")

    manager = FragmentManager()
    await manager.initialize()

    # Create session
    session = await manager.create_session(name="Test Session")
    print(f"✓ Session created: {session.id}")

    # Create fragment
    fragment = await manager.create_fragment(
        session_id=session.id,
        raw_text="这是一个测试文本。",
        start_time=0,
        end_time=5
    )
    print(f"✓ Fragment created: {fragment.id}")

    # List fragments
    fragments = await manager.list_fragments(session_id=session.id)
    print(f"✓ Listed {len(fragments)} fragments")

    # Cleanup
    await manager.delete_session(session.id)
    print("✓ Fragment manager test passed")


async def main():
    """Run all tests"""
    print("Running basic tests...")

    try:
        await test_audio_processor()
        await test_fragment_manager()

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
