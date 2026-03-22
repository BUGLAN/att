#!/usr/bin/env python3
"""
Download Whisper models for offline use
"""
import argparse
from pathlib import Path

from faster_whisper import WhisperModel


def download_model(model_size: str, models_dir: Path) -> None:
    """Download a specific model"""
    print(f"Downloading {model_size} model...")

    # This will download the model to the default cache location
    # then we can optionally copy it
    model = WhisperModel(
        model_size,
        device="cpu",
        compute_type="int8",
        download_root=str(models_dir)
    )

    print(f"Model {model_size} downloaded successfully!")
    print(f"Model location: {models_dir}")


def main():
    parser = argparse.ArgumentParser(description="Download Whisper models")
    parser.add_argument(
        "--model",
        default="large-v3",
        choices=["tiny", "base", "small", "medium", "large-v2", "large-v3"],
        help="Model size to download (default: large-v3 for Chinese)"
    )
    parser.add_argument(
        "--models-dir",
        default="models/whisper",
        help="Directory to store models"
    )

    args = parser.parse_args()

    models_dir = Path(args.models_dir)
    models_dir.mkdir(parents=True, exist_ok=True)

    download_model(args.model, models_dir)


if __name__ == "__main__":
    main()
