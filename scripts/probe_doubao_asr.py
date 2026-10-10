"""火山豆包双向流式 ASR 探针脚本。

用途：**在不打开 App 的情况下**用真实凭据跑通「握手 → 推流 → 结果 → 计费时长」，
出问题时先看这里的输出，再回 App 里排查。

用法（PowerShell，密钥只从环境变量读，不落盘、不进命令行历史）::

    $env:DOUBAO_ASR_API_KEY = "<火山语音技术 Access Token>"
    uv run python scripts/probe_doubao_asr.py --wav tmp-audio/sample.wav
    uv run python scripts/probe_doubao_asr.py --wav x.wav --fast        # 无间隔快速推送
    $env:DOUBAO_ASR_RESOURCE_ID = "volc.bigasr.sauc.duration"           # 换 ASR 1.0 资源 ID

脚本复用 App 内的 ``DoubaoStreamingProvider``（同一套帧编解码与 definite 前缀推进），
因此跑通即代表 App 链路可用；脚本本身不写文件、不打印密钥。

判据（与调用文档 §12 一致）：
1. 握手成功；2. 收到中间结果；3. 收到已确认结果；4. 中间/最终可区分；
5. 首句回显 ≤ 2s（按**首个中间结果**算，不是最终确认）；
6. 计费时长与音频时长一致（±5%）。
"""
from __future__ import annotations

import argparse
import asyncio
import os
import sys
import time
from pathlib import Path

# 允许直接 `python scripts/probe_doubao_asr.py` 运行（把仓库根加进 sys.path）
PROJECT_ROOT = Path(__file__).resolve().parents[1]
if str(PROJECT_ROOT) not in sys.path:
    sys.path.insert(0, str(PROJECT_ROOT))

# Windows 控制台默认 GBK，直接 print 中文/符号会抛 UnicodeEncodeError，这里统一成 UTF-8
for stream in (sys.stdout, sys.stderr):
    try:
        stream.reconfigure(encoding="utf-8", errors="replace")
    except (AttributeError, ValueError):  # pragma: no cover - 非标准流
        pass

from app.stt.providers.doubao_asr_provider import (  # noqa: E402
    DoubaoStreamingProvider,
    _read_pcm16_mono,
)
from app.stt.providers.volcengine_sauc import (  # noqa: E402
    DEFAULT_ENDPOINT,
    DEFAULT_RESOURCE_ID,
    SaucSessionError,
)

DEFAULT_CHUNK_MS = 200


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="豆包双向流式 ASR 探针")
    parser.add_argument("--wav", required=True, help="待识别音频（任意 soundfile 支持的格式）")
    parser.add_argument("--fast", action="store_true", help="不做实时节奏等待，尽可能快推流")
    parser.add_argument("--language", default="zh", help="语言码，默认 zh")
    parser.add_argument("--context", default="", help="对话上下文（续录偏置用）")
    parser.add_argument("--end-window-size", type=int, default=0, help="强制断句静音时长（毫秒），0=关闭")
    return parser.parse_args()


def build_provider(args: argparse.Namespace) -> DoubaoStreamingProvider:
    api_key = os.environ.get("DOUBAO_ASR_API_KEY", "").strip()
    if not api_key:
        raise SystemExit(
            "未设置 DOUBAO_ASR_API_KEY 环境变量。\n"
            'PowerShell: $env:DOUBAO_ASR_API_KEY = "<你的 Access Token>"'
        )

    return DoubaoStreamingProvider(
        api_key=api_key,
        resource_id=os.environ.get("DOUBAO_ASR_RESOURCE_ID", DEFAULT_RESOURCE_ID),
        endpoint=os.environ.get("DOUBAO_ASR_ENDPOINT", DEFAULT_ENDPOINT),
        end_window_size=args.end_window_size,
        # 探针按真实节奏等结果，延迟指标才有意义
        drain_timeout_seconds=0.3,
    )


def main() -> int:
    args = parse_args()
    audio_path = Path(args.wav)
    if not audio_path.exists():
        raise SystemExit(f"音频文件不存在: {audio_path}")

    provider = build_provider(args)
    samples, duration = _read_pcm16_mono(str(audio_path))
    if samples.size == 0:
        raise SystemExit("音频为空")

    chunk_samples = provider.chunk_samples
    chunk_ms = provider.chunk_ms
    total_chunks = (samples.size + chunk_samples - 1) // chunk_samples

    print(f"音频: {audio_path}")
    print(f"时长: {duration:.2f}s  采样: {samples.size}  分帧: {total_chunks} × {chunk_ms}ms")
    print(f"接入点: {provider.endpoint}")
    print(f"Resource ID: {provider.resource_id}")
    print(f"节奏: {'快速推送' if args.fast else f'实时（{chunk_ms}ms/包）'}")
    print("-" * 72)

    started_at = time.monotonic()
    first_interim_at: float | None = None
    interim_count = 0
    definite_count = 0
    committed_length = 0
    last_text = ""
    session = None

    try:
        session = asyncio.run(provider.create_session(language=args.language, context=args.context))
        print(f"握手成功  connect_id={session.connect_id}  x-tt-logid={session.log_id or '(未返回)'}")

        for index in range(total_chunks):
            chunk = samples[index * chunk_samples : (index + 1) * chunk_samples]
            update = session.append_audio(chunk)

            committed = session.committed_text
            if len(committed) > committed_length:
                definite_count += 1
                committed_length = len(committed)

            if update.text and update.text != last_text:
                elapsed = time.monotonic() - started_at
                if first_interim_at is None:
                    first_interim_at = elapsed
                interim_count += 1
                print(f"[{elapsed:6.2f}s] {update.text}")
                last_text = update.text

            if not args.fast:
                # 按真实节奏推流：让服务端按 200ms 一包处理
                target = started_at + (index + 1) * (chunk_ms / 1000.0)
                sleep_seconds = target - time.monotonic()
                if sleep_seconds > 0:
                    time.sleep(sleep_seconds)

        final_update = session.finish()
        total_elapsed = time.monotonic() - started_at
    except SaucSessionError as error:
        print(f"\n❌ 会话失败: {error}")
        print(f"   kind={error.kind} retryable={error.retryable} code={error.provider_code}")
        return 1

    final_text = (final_update.text or "").strip()
    billed_ms = session.audio_duration_ms
    billed_seconds = billed_ms / 1000.0

    print("-" * 72)
    print(f"最终文本: {final_text or '(空)'}")
    print(f"总耗时: {total_elapsed:.2f}s   计费时长: {billed_seconds:.2f}s")

    checks = [
        ("握手成功", True),
        ("收到中间结果", interim_count > 0),
        ("收到已确认结果", definite_count > 0 or bool(final_text)),
        ("中间 / 最终可区分", bool(final_text)),
        (
            "首句回显 ≤ 2s（按首个中间结果）",
            first_interim_at is not None and first_interim_at <= 2.0,
        ),
        (
            "计费时长与音频时长一致（±5%）",
            duration > 0 and abs(billed_seconds - duration) <= max(0.2, duration * 0.05),
        ),
    ]

    print("\n判据:")
    for label, passed in checks:
        print(f"  {'✅' if passed else '❌'} {label}")

    if first_interim_at is not None:
        print(f"\n首个中间结果延迟: {first_interim_at * 1000:.0f} ms")

    return 0 if all(passed for _, passed in checks) else 1


if __name__ == "__main__":
    raise SystemExit(main())
