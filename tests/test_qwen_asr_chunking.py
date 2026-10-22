"""Qwen3-ASR 长文件分块转写测试（设计文档 §3.3）。

覆盖：
- 静音切点检测：能量静音段中点、无静音/全静音边界
- 贪心装箱：窗内最靠右静音点优先、无静音点硬切、末块收尾
- 分块转写：时间戳偏移拼接、语言回传、空结果块跳过
- 短音频（≤ 窗口）走单发路径不分块
- 服务层注入：文件路径的 qwen 分块窗口来自 TRANSCRIBE_FILE_QWEN_CHUNK_SECONDS

模型与音频全部打桩（numpy 合成音频，不加载真模型、不读真文件）。
"""
import asyncio
from pathlib import Path

import numpy as np
import pytest

from app.config import settings
from app.services.file_transcription_service import FileTranscriptionService
from app.stt.providers.qwen_asr_provider import QwenASRProvider


def _provider(chunk_seconds: int = 30) -> QwenASRProvider:
    return QwenASRProvider(
        model_name="fake-qwen",
        device="cpu",
        dtype="auto",
        cache_dir="",
        max_audio_seconds=chunk_seconds,
    )


def _synthetic_audio(sample_rate: int, specs: list[tuple[int, int]]) -> np.ndarray:
    """按 (语音秒, 静音秒) 交替合成：语音=440Hz 正弦，静音=零。"""
    chunks = []
    for voice_seconds, silence_seconds in specs:
        voice = (0.3 * np.sin(
            2 * np.pi * 440 * np.arange(int(voice_seconds * sample_rate)) / sample_rate
        )).astype("float32")
        chunks.append(voice)
        chunks.append(np.zeros(int(silence_seconds * sample_rate), dtype="float32"))
    return np.concatenate(chunks)


# ---------------------------------------------------------------- 静音切点与装箱

def test_silence_cut_points_at_midpoints():
    provider = _provider()
    sr = 16000
    # 10s 语音 + 1s 静音 + 10s 语音：切点应在静音中点（约 10.5s）
    audio = _synthetic_audio(sr, [(10, 1), (10, 0)])
    cuts = provider._silence_cut_points(audio, sr)
    assert len(cuts) == 1
    assert abs(cuts[0] / sr - 10.5) < 0.05


def test_silence_cut_points_none_for_continuous_speech():
    provider = _provider()
    sr = 16000
    audio = _synthetic_audio(sr, [(60, 0)])  # 连续说话 60s，无静音
    assert provider._silence_cut_points(audio, sr) == []


def test_plan_chunks_prefers_latest_silence_and_hard_cuts():
    provider = _provider(chunk_seconds=30)
    sr = 16000
    limit = 30 * sr

    # 两个可用静音点（10.5s / 25.5s）：第一块切在 25.5s（窗内最靠右）；
    # 剩余 34.5s 超窗，窗内无静音点 → 硬切 30s，末块收尾 4.5s
    cuts = [int(10.5 * sr), int(25.5 * sr)]
    chunks = provider._plan_chunks(int(60 * sr), sr, cuts)
    assert [round(end / sr, 1) for _, end in chunks] == [25.5, 55.5, 60.0]

    # 无任何静音点：硬切 30s
    chunks = provider._plan_chunks(int(70 * sr), sr, [])
    assert [round((end - start) / sr, 1) for start, end in chunks] == [30.0, 30.0, 10.0]

    # 末块不足窗口：直接收尾
    chunks = provider._plan_chunks(int(25 * sr), sr, [])
    assert chunks == [(0, 25 * sr)]


def test_plan_chunks_no_infinite_loop_on_degenerate_cuts():
    provider = _provider(chunk_seconds=30)
    chunks = provider._plan_chunks(16000 * 45, 16000, [0, 100])  # 切点全部 ≤ 起点
    assert all(end > start for start, end in chunks)
    assert chunks[-1][1] == 16000 * 45


# ---------------------------------------------------------------- 分块转写（桩模型）

class _FakeQwenModel:
    """记录每块音频长度，按调用序返回写死结果（dict 形态，走 provider 的宽容读取）。"""

    def __init__(self):
        self.chunk_seconds = []

    def transcribe(self, *, audio, language=None, return_time_stamps=False):
        samples, sample_rate = audio
        seconds = samples.size / sample_rate
        self.chunk_seconds.append(round(seconds, 1))
        index = len(self.chunk_seconds)
        return [{
            "text": f"第{index}块文本",
            "language": "Chinese",
            "time_stamps": [],
        }]


def _patch_io(monkeypatch, provider, audio: np.ndarray, sample_rate: int, duration: float):
    model = _FakeQwenModel()
    monkeypatch.setattr(provider, "_load_model", lambda: model)  # 固定同一实例，测试可回读
    monkeypatch.setattr(provider, "_load_audio_input", lambda path: (audio, sample_rate))
    monkeypatch.setattr(provider, "_read_duration", lambda path: duration)


def test_chunked_transcribe_offsets_and_joins(monkeypatch):
    provider = _provider(chunk_seconds=30)
    sr = 16000
    # 28s 语音 + 1s 静音 + 28s 语音 + 1s 静音 + 10s 语音 ≈ 68s → 3 块
    audio = _synthetic_audio(sr, [(28, 1), (28, 1), (10, 0)])
    _patch_io(monkeypatch, provider, audio, sr, audio.size / sr)

    result = asyncio.run(provider.transcribe("fake.wav", language="zh"))

    model = provider._load_model()
    assert len(model.chunk_seconds) == 3
    assert model.chunk_seconds[0] <= 30.0  # 每块都不超窗口
    assert result.text == "第1块文本 第2块文本 第3块文本"
    assert result.language == "zh"  # Chinese → zh 归一
    # 时间戳按块偏移：第三块起点应 ≈ 58s（28+1+28+1）
    assert abs(result.segments[2].start - 58.0) < 1.5
    assert result.segments[0].start == pytest.approx(0.0, abs=1.0)


def test_short_audio_single_shot(monkeypatch):
    provider = _provider(chunk_seconds=30)
    sr = 16000
    audio = _synthetic_audio(sr, [(10, 0)])
    _patch_io(monkeypatch, provider, audio, sr, 10.0)

    result = asyncio.run(provider.transcribe("fake.wav", language="zh"))
    model = provider._load_model()
    assert model.chunk_seconds == [10.0]  # 单发路径，未分块
    assert result.text == "第1块文本"


def test_chunk_with_empty_result_skipped(monkeypatch):
    provider = _provider(chunk_seconds=30)
    sr = 16000
    audio = _synthetic_audio(sr, [(28, 1), (28, 0)])
    _patch_io(monkeypatch, provider, audio, sr, audio.size / sr)

    model = _FakeQwenModel()
    replies = [[{"text": "有内容", "language": "Chinese", "time_stamps": []}], []]
    model.transcribe = lambda **kwargs: replies.pop(0)
    monkeypatch.setattr(provider, "_load_model", lambda: model)

    result = asyncio.run(provider.transcribe("fake.wav", language="zh"))
    assert result.text == "有内容"  # 空块跳过，不产生空段
    assert len(result.segments) == 1


# ---------------------------------------------------------------- 服务层窗口注入

def test_service_injects_qwen_chunk_window(monkeypatch):
    monkeypatch.setattr(settings, "transcribe_file_qwen_chunk_seconds", 25)
    service = FileTranscriptionService()

    from app.stt import factory as stt_factory
    from app.stt.models import ASRProviderConfig, ASRProviderType

    monkeypatch.setattr(
        stt_factory, "get_asr_provider_config",
        lambda: ASRProviderConfig(
            provider=ASRProviderType.QWEN_ASR, language="zh",
            whisper={}, qwen_asr={"max_audio_seconds": 30}, doubao_asr={},
        ),
    )
    engine, config = service._resolve_engine("qwen_asr")
    assert engine == ASRProviderType.QWEN_ASR
    assert config.qwen_asr["max_audio_seconds"] == 25  # 文件路径窗口覆盖
