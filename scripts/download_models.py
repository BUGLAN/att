#!/usr/bin/env python3
"""
下载 ASR 模型到项目内缓存目录
"""
import argparse
from pathlib import Path
import sys

PROJECT_ROOT = Path(__file__).resolve().parent.parent
if str(PROJECT_ROOT) not in sys.path:
    sys.path.insert(0, str(PROJECT_ROOT))

from app.config import settings


def download_whisper_model(model_name: str, models_dir: Path) -> None:
    """下载 faster-whisper 模型"""
    from faster_whisper import WhisperModel

    models_dir.mkdir(parents=True, exist_ok=True)

    print(f"开始下载 faster-whisper 模型: {model_name}")
    WhisperModel(
        model_name,
        device="cpu",
        compute_type="int8",
        download_root=str(models_dir)
    )
    print(f"下载完成: {model_name}")
    print(f"缓存目录: {models_dir}")


def download_qwen_model(
    model_name: str,
    cache_dir: Path,
    forced_aligner_model: str | None = None
) -> None:
    """下载 Qwen ASR 模型"""
    from huggingface_hub import snapshot_download

    cache_dir.mkdir(parents=True, exist_ok=True)

    print(f"开始下载 Qwen ASR 模型: {model_name}")
    snapshot_download(
        repo_id=model_name,
        cache_dir=str(cache_dir)
    )
    print(f"下载完成: {model_name}")

    if forced_aligner_model:
        print(f"开始下载强制对齐模型: {forced_aligner_model}")
        snapshot_download(
            repo_id=forced_aligner_model,
            cache_dir=str(cache_dir)
        )
        print(f"下载完成: {forced_aligner_model}")

    print(f"缓存目录: {cache_dir}")


def build_parser() -> argparse.ArgumentParser:
    """构建命令行参数"""
    parser = argparse.ArgumentParser(description="下载 Whisper 或 Qwen ASR 模型")
    parser.add_argument(
        "--provider",
        required=True,
        choices=["whisper", "qwen"],
        help="选择要下载的 provider"
    )
    parser.add_argument(
        "--model",
        help="模型名称；不传则使用配置中的默认值"
    )
    parser.add_argument(
        "--models-dir",
        help="缓存目录；不传则使用项目内默认目录"
    )
    parser.add_argument(
        "--with-aligner",
        action="store_true",
        help="下载 Qwen 强制对齐模型"
    )
    parser.add_argument(
        "--aligner-model",
        help="强制对齐模型名称；不传则使用配置中的默认值"
    )
    return parser


def main() -> None:
    """脚本入口"""
    parser = build_parser()
    args = parser.parse_args()

    if args.provider == "whisper":
        model_name = args.model or settings.whisper_model
        models_dir = Path(args.models_dir) if args.models_dir else settings.faster_whisper_models_path
        download_whisper_model(model_name, models_dir)
        return

    model_name = args.model or settings.qwen_asr_model
    cache_dir = Path(args.models_dir) if args.models_dir else settings.qwen_models_path
    forced_aligner_model = None
    if args.with_aligner:
        forced_aligner_model = args.aligner_model or settings.qwen_asr_forced_aligner_model

    download_qwen_model(
        model_name=model_name,
        cache_dir=cache_dir,
        forced_aligner_model=forced_aligner_model
    )


if __name__ == "__main__":
    main()
