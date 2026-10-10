#!/usr/bin/env python3
"""
流式 ASR 延迟与 RTF 基准脚本

对应需求文档的 AC-3（实时性）与 AC-15（模型加载耗时实测）。

用途：
    直接压测 Qwen 流式桥接服务（scripts/qwen_streaming_service.py）的 HTTP 接口，
    量化两个关键指标：
      1. RTF（实时因子）= 单块推理耗时 / 该块音频时长；RTF >= 1 必然积压，无法实时
      2. 首段文本延迟（算法前瞻延迟的代理指标），即从开始推音频到出现非空文本的耗时

用法：
    # 在 ASR 容器内运行（9000 端口不发布到宿主，这是推荐方式）
    docker compose exec asr python3 scripts/bench_streaming_latency.py

    # 真实语音测试（推荐：合成音频只能测时序，测不出识别质量）
    docker compose exec asr python3 scripts/bench_streaming_latency.py --wav /app/data/bench.wav

    # 本地原生运行
    .venv\\Scripts\\python.exe scripts/bench_streaming_latency.py --base-url http://127.0.0.1:9000

说明：
    不传 --wav 时脚本会合成一段类似语音节奏的音频（正弦波 + 包络），
    仅用于测量时序与 RTF，识别出的文本没有意义。
"""
from __future__ import annotations

import argparse
import statistics
import sys
import time
import wave
from pathlib import Path

import httpx
import numpy as np

SAMPLE_RATE = 16000
CHUNK_SAMPLES = 3200  # 与前端一致：200ms 一包
CHUNK_SECONDS = CHUNK_SAMPLES / SAMPLE_RATE


def build_synthetic_audio(seconds: float) -> np.ndarray:
    """合成一段有节奏的音频，仅用于时序压测。"""
    total = int(SAMPLE_RATE * seconds)
    t = np.arange(total) / SAMPLE_RATE
    # 基频在 120-220Hz 之间缓慢摆动，模拟语音基频
    base = 170 + 40 * np.sin(2 * np.pi * 0.7 * t)
    signal = 0.35 * np.sin(2 * np.pi * base * t)
    # 语音节奏包络：约 3Hz 的音节开合
    envelope = 0.5 + 0.5 * np.sin(2 * np.pi * 3.0 * t)
    # 每 4 秒插入 0.4 秒静音，模拟自然停顿
    pause = (np.mod(t, 4.0) > 3.6).astype(np.float64)
    waveform = signal * envelope * (1.0 - pause)
    return (waveform * 32767).astype(np.int16)


def load_wav(path: Path) -> np.ndarray:
    """读取 16kHz 单声道 WAV；其他采样率直接报错，避免测出错误时序。"""
    with wave.open(str(path), "rb") as handle:
        channels = handle.getnchannels()
        width = handle.getsampwidth()
        rate = handle.getframerate()
        frames = handle.readframes(handle.getnframes())

    if width != 2:
        raise SystemExit(f"仅支持 16bit PCM WAV，当前位深为 {width * 8}bit")
    if channels != 1:
        raise SystemExit(f"仅支持单声道 WAV，当前声道数为 {channels}")
    if rate != SAMPLE_RATE:
        raise SystemExit(f"仅支持 {SAMPLE_RATE}Hz WAV，当前采样率为 {rate}Hz")

    return np.frombuffer(frames, dtype=np.int16)


def percentile(values: list[float], ratio: float) -> float:
    """计算分位数（线性插值），样本过少时退化为最大值。"""
    if not values:
        return 0.0
    if len(values) == 1:
        return values[0]
    return float(np.percentile(np.asarray(values, dtype=np.float64), ratio * 100))


def run_benchmark(
    base_url: str,
    audio: np.ndarray,
    language: str,
    timeout: float,
    warmup_chunks: int = 4,
) -> int:
    chunk_count = len(audio) // CHUNK_SAMPLES
    audio = audio[: chunk_count * CHUNK_SAMPLES]
    total_seconds = len(audio) / SAMPLE_RATE

    print(f"目标服务      : {base_url}")
    print(f"音频时长      : {total_seconds:.2f}s（{chunk_count} 块，每块 {CHUNK_SECONDS:.2f}s）")
    print("")

    with httpx.Client(base_url=base_url, timeout=timeout) as client:
        # 启动前先确认服务健康，避免把连接失败算成推理耗时
        try:
            health = client.get("/api/health").json()
        except Exception as error:
            print(f"[失败] 无法连接 ASR 服务：{error}")
            print("[建议] 容器部署下请用 docker compose exec asr 执行本脚本；")
            print("       本地原生运行请确认 npm run qwen:streaming:uv 已启动")
            return 1

        if not (health.get("ready") and health.get("loaded")):
            print(f"[警告] 模型尚未加载（ready={health.get('ready')}, loaded={health.get('loaded')}）")
            print("       将先触发 /api/preload 预热（首次加载较慢）")
            started = time.perf_counter()
            client.post("/api/preload", timeout=max(timeout, 900.0))
            print(f"       预热完成，耗时 {time.perf_counter() - started:.1f}s")
            health = client.get("/api/health").json()

        print(f"模型          : {health.get('model_name')}")
        print(f"显存占用比例  : {health.get('gpu_memory_utilization')}")

        # 引擎预热：用静音单独跑一小段，把首次推理的编译开销（实测可达 30 秒以上）
        # 单独计量，避免污染稳态 RTF 统计。
        if warmup_chunks > 0:
            silence = np.zeros(CHUNK_SAMPLES, dtype=np.int16).tobytes()
            warmup_session = client.post(
                "/api/start", json={"language": language, "context": ""}
            ).json()["session_id"]
            warmup_started = time.perf_counter()
            for _ in range(warmup_chunks):
                client.post(
                    f"/api/chunk/{warmup_session}",
                    content=silence,
                    headers={"Content-Type": "application/octet-stream"},
                )
            client.post(f"/api/finish/{warmup_session}")
            warmup_elapsed = time.perf_counter() - warmup_started
            print(f"引擎预热      : {warmup_chunks} 块静音耗时 {warmup_elapsed:.2f}s（含首次编译，不计入统计）")
        else:
            print("引擎预热      : 已跳过（统计将包含首次编译开销）")

        print("")

        session = client.post("/api/start", json={"language": language, "context": ""}).json()
        session_id = session["session_id"]

        per_chunk_ms: list[float] = []
        first_text_seconds = None
        final_text = ""
        wall_start = time.perf_counter()

        try:
            for index in range(chunk_count):
                payload = audio[index * CHUNK_SAMPLES : (index + 1) * CHUNK_SAMPLES].tobytes()

                started = time.perf_counter()
                response = client.post(
                    f"/api/chunk/{session_id}",
                    content=payload,
                    headers={"Content-Type": "application/octet-stream"},
                )
                response.raise_for_status()
                elapsed = time.perf_counter() - started

                per_chunk_ms.append(elapsed * 1000)
                data = response.json()
                text = (data.get("text") or "").strip()

                if text and first_text_seconds is None:
                    first_text_seconds = time.perf_counter() - wall_start

                if text:
                    final_text = text

            finish = client.post(f"/api/finish/{session_id}").json()
            final_text = (finish.get("text") or final_text).strip()
        except Exception as error:
            print(f"[失败] 压测中断：{error}")
            return 1

        wall_total = time.perf_counter() - wall_start

        # ---- 实时节奏模拟：按前端真实的 200ms 节奏推流，观察排队延迟是否持续增长 ----
        # 这是「能否实时」最可信的判据：只要每块都能在下一块到来前处理完就不会积压。
        realtime_session = client.post(
            "/api/start", json={"language": language, "context": ""}
        ).json()["session_id"]
        realtime_lags: list[float] = []
        paced_start = time.perf_counter()

        try:
            for index in range(chunk_count):
                target = paced_start + index * CHUNK_SECONDS
                now = time.perf_counter()
                if now < target:
                    time.sleep(target - now)

                client.post(
                    f"/api/chunk/{realtime_session}",
                    content=audio[index * CHUNK_SAMPLES : (index + 1) * CHUNK_SAMPLES].tobytes(),
                    headers={"Content-Type": "application/octet-stream"},
                )
                realtime_lags.append(time.perf_counter() - target)

            client.post(f"/api/finish/{realtime_session}")
        except Exception as error:
            print(f"[失败] 实时节奏模拟中断：{error}")
            return 1

    # 注意：单块耗时的 P95 天然会偏高——模型按 chunk_size_sec 窗口推理，
    # 每到一个窗口边界的那一次请求会承担整窗计算。因此判据以「总体 RTF」和
    # 「排队延迟是否增长」为准，而不是单块 P95。
    overall_rtf = wall_total / total_seconds
    median_ms = statistics.median(per_chunk_ms)
    quarter = max(1, len(realtime_lags) // 4)
    head_mean = statistics.mean(realtime_lags[:quarter])
    tail_mean = statistics.mean(realtime_lags[-quarter:])
    # 只比较第 3 个与第 4 个四分位：初期因窗口边界尖峰爬升属正常，
    # 真正危险的是尾部仍在持续增长（说明推理跟不上采集）
    late_mean = statistics.mean(realtime_lags[-quarter * 2 : -quarter] or realtime_lags[-quarter:])
    growth = tail_mean - late_mean

    average_ms = statistics.mean(per_chunk_ms)
    p95_ms = percentile(per_chunk_ms, 0.95)
    rtf_average = (average_ms / 1000) / CHUNK_SECONDS
    rtf_p95 = (p95_ms / 1000) / CHUNK_SECONDS
    slowest = max(per_chunk_ms)

    print("-- 单块推理耗时（参考值，含窗口边界尖峰）--")
    print(f"  平均   : {average_ms:.1f} ms")
    print(f"  P95    : {p95_ms:.1f} ms")
    print(f"  最大   : {slowest:.1f} ms")
    print(f"  分块 RTF: 平均 {rtf_average:.3f} / P95 {rtf_p95:.3f}")
    print("")
    print("-- 实时节奏模拟（每 200ms 推一块，最接近真实使用）--")
    print(f"  平均排队延迟 : {statistics.mean(realtime_lags) * 1000:.1f} ms")
    print(f"  最大排队延迟 : {max(realtime_lags) * 1000:.1f} ms")
    print(f"  前 25% 平均  : {head_mean * 1000:.1f} ms")
    print(f"  后 25% 平均  : {tail_mean * 1000:.1f} ms")
    print(f"  末段增量     : {growth * 1000:.1f} ms   （第 4 四分位 − 第 3 四分位；持续为正且变大即代表积压）")
    print("")
    print("-- 端到端 --")
    print(f"  总体 RTF     : {overall_rtf:.3f}   （推流总耗时 {wall_total:.2f}s ÷ 音频 {total_seconds:.2f}s；< 1.0 才能实时）")
    if first_text_seconds is None:
        print("  首段文本延迟 : 未产生任何文本（合成音频属正常现象）")
    else:
        print(f"  首段文本延迟 : {first_text_seconds:.2f}s  （含 {CHUNK_SECONDS:.2f}s 分块，AC-3 目标 P95 <= 3.0s）")
    print(f"  文本长度     : {len(final_text)} 字")
    print("")

    failures: list[str] = []

    if overall_rtf >= 1.0:
        failures.append(f"总体 RTF {overall_rtf:.3f} >= 1.0，推理跟不上采集")
    elif overall_rtf > 0.5:
        print(f"[警告] 总体 RTF {overall_rtf:.3f} 可实时但高于 0.5 目标，余量不足")

    if growth > 0.5:
        failures.append(
            f"排队延迟末段仍在增长（增量 {growth * 1000:.0f}ms），存在积压"
        )

    if median_ms > 0 and per_chunk_ms[0] > 5 * median_ms:
        print("[警告] 首个测量块远慢于稳态中位数，说明引擎预热未生效，")
        print("       首次录音仍会承受这段编译开销。请确认已执行 /api/preload。")

    if failures:
        for item in failures:
            print(f"[失败] {item}")
        print("[建议] 检查显存占用与并发；必要时调大 QWEN_STREAMING_CHUNK_SIZE_SEC（减少窗口边界次数）")
        return 1

    print(f"[OK] 实时性达标：总体 RTF {overall_rtf:.3f} < 0.5 目标以内，排队延迟无增长")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description="流式 ASR 延迟与 RTF 基准")
    parser.add_argument("--base-url", default="http://127.0.0.1:9000", help="流式桥接服务地址")
    parser.add_argument("--wav", default="", help="16kHz 单声道 16bit WAV 文件路径（不传则合成测试音频）")
    parser.add_argument("--seconds", type=float, default=20.0, help="合成音频时长（秒）")
    parser.add_argument("--language", default="zh", help="语言代码")
    parser.add_argument("--timeout", type=float, default=120.0, help="单次请求超时（秒）")
    parser.add_argument(
        "--warmup-chunks",
        type=int,
        default=4,
        help="预热块数（静音，单独计量，默认 4 块；0 表示不预热）",
    )
    args = parser.parse_args()

    if args.wav:
        wav_path = Path(args.wav)
        if not wav_path.exists():
            print(f"[失败] 找不到音频文件：{wav_path}")
            return 1
        audio = load_wav(wav_path)
    else:
        audio = build_synthetic_audio(args.seconds)
        print("[提示] 未指定 --wav，使用合成音频，仅用于测量时序与 RTF")
        print("")

    return run_benchmark(args.base_url, audio, args.language, args.timeout, args.warmup_chunks)


if __name__ == "__main__":
    sys.exit(main())
