"""火山豆包 SAUC 协议编解码单测。

锁死的点（改这条链路前先看这里）：
1. 帧头字节位域、音频帧末包 flags；
2. gzip 与不压缩两种响应都能解析；
3. 错误帧按官方布局解析，并容忍「多一个标志字节」的形态；
4. 结构异常一律归 unknown，不抛错、不断流；
5. definite 连续前缀推进：累积文本不变时也要能定稿（历史缺陷）；
6. corpus.context 必须是 JSON 字符串，且按码点截尾；
7. 错误码 / 关闭码 / 握手状态码三类归一化。
"""
import gzip
import json
import struct

import pytest

from app.stt.providers.volcengine_sauc import (
    ASR_CONTEXT_MAX_CHARS,
    COMPRESSION_GZIP,
    COMPRESSION_NONE,
    FLAG_LAST_NO_SEQUENCE,
    FLAG_NO_SEQUENCE,
    KIND_AUTH,
    KIND_INPUT,
    KIND_NETWORK,
    KIND_NONE,
    KIND_PROTOCOL,
    KIND_QUOTA,
    KIND_SERVER,
    MSG_FULL_CLIENT_REQUEST,
    MSG_FULL_SERVER_RESPONSE,
    MSG_SERVER_ERROR_RESPONSE,
    SERIALIZATION_JSON,
    SaucUtterance,
    advance_definite_prefix,
    build_audio_request,
    build_bigmodel_request,
    build_dialogue_context_json,
    build_full_client_request,
    build_header,
    classify_close_code,
    classify_handshake_status,
    classify_sauc_error,
    parse_server_frame,
    pending_from_cumulative_text,
    truncate_context_text,
)


def build_server_result(payload: dict, *, sequence: int = 1, compress: bool = False, last: bool = False) -> bytes:
    """构造服务端结果帧（测试专用，与官方布局一致）。"""
    body = json.dumps(payload, ensure_ascii=False).encode("utf-8")
    if compress:
        body = gzip.compress(body)

    flags = 0b0011 if last else 0b0001
    header = build_header(
        MSG_FULL_SERVER_RESPONSE,
        flags,
        SERIALIZATION_JSON,
        COMPRESSION_GZIP if compress else COMPRESSION_NONE,
    )
    return header + struct.pack(">i", sequence) + struct.pack(">I", len(body)) + body


def build_server_error(code: int, message: str, *, extra_flag_byte: bool = False) -> bytes:
    """构造服务端错误帧。"""
    body = message.encode("utf-8")
    header = build_header(MSG_SERVER_ERROR_RESPONSE, FLAG_NO_SEQUENCE, SERIALIZATION_JSON, COMPRESSION_NONE)
    prefix = b"\x11" if extra_flag_byte else b""
    return header + prefix + struct.pack(">I", code) + struct.pack(">I", len(body)) + body


# ==================== 帧编解码 ====================


def test_full_client_request_header_layout():
    frame = build_full_client_request({"audio": {"format": "pcm"}})

    assert frame[0] == 0x11  # 协议版本 1 + 头长度 1（4 字节）
    assert frame[1] == 0x10  # 消息类型 0b0001（全量请求） + flags 0
    assert frame[2] == 0x10  # 序列化 JSON + 不压缩
    assert frame[3] == 0x00

    size = struct.unpack(">I", frame[4:8])[0]
    assert size == len(frame) - 8
    assert json.loads(frame[8:].decode("utf-8"))["audio"]["format"] == "pcm"


def test_audio_request_flags_for_final_packet():
    normal = build_audio_request(b"\x00" * 16)
    final = build_audio_request(b"\x00" * 16, final=True)

    assert (normal[1] >> 4) == 0b0010  # 纯音频请求
    assert (normal[1] & 0x0F) == FLAG_NO_SEQUENCE
    assert (final[1] & 0x0F) == FLAG_LAST_NO_SEQUENCE
    assert normal[2] == 0x00  # RAW + 不压缩
    assert struct.unpack(">I", normal[4:8])[0] == 16


def test_audio_request_gzip_roundtrip():
    payload = b"\x01\x02" * 3200
    frame = build_audio_request(payload, compress=True)

    assert (frame[2] & 0x0F) == COMPRESSION_GZIP
    assert gzip.decompress(frame[8:]) == payload


def test_parse_result_frame_with_gzip_and_sequence():
    payload = {
        "audio_info": {"duration": 3696},
        "result": {
            "text": "今天这场会主要过三件事。",
            "utterances": [
                {"text": "今天这场会主要过三件事。", "start_time": 0, "end_time": 1705, "definite": True}
            ],
        },
    }
    frame = parse_server_frame(build_server_result(payload, compress=True))

    assert frame.kind == "result"
    assert frame.sequence == 1
    assert frame.result.audio_duration_ms == 3696
    assert frame.result.text == "今天这场会主要过三件事。"
    assert frame.result.utterances[0].definite is True
    assert frame.result.utterances[0].end_ms == 1705


def test_parse_result_frame_marks_last_packet():
    frame = parse_server_frame(build_server_result({"result": {"text": "完"}}, last=True))

    assert frame.kind == "result"
    assert frame.is_last is True


def test_parse_error_frame_official_layout():
    frame = parse_server_frame(build_server_error(45000001, "invalid request"))

    assert frame.kind == "error"
    assert frame.error.code == 45000001
    assert frame.error.message == "invalid request"


def test_parse_error_frame_tolerates_extra_flag_byte():
    """调用文档记录的「多一个标志字节」形态也要能读到错误码。"""
    frame = parse_server_frame(build_server_error(45000001, "invalid request", extra_flag_byte=True))

    assert frame.kind == "error"
    assert frame.error.code == 45000001
    assert frame.error.message == "invalid request"


def test_parse_garbage_returns_unknown():
    assert parse_server_frame(b"").kind == "unknown"
    assert parse_server_frame(b"\x11\x91\x11").kind == "unknown"
    assert parse_server_frame(b"\x11\x91\x11\x00" + struct.pack(">i", 1) + struct.pack(">I", 99) + b"{}").kind == "unknown"


def test_parse_empty_payload_is_unknown_not_error():
    """payload_size=0 的帧必须静默忽略，不能当成错误。"""
    frame_bytes = (
        build_header(MSG_FULL_SERVER_RESPONSE, FLAG_NO_SEQUENCE, SERIALIZATION_JSON, COMPRESSION_NONE)
        + struct.pack(">I", 0)
    )

    assert parse_server_frame(frame_bytes).kind == "unknown"


def test_parse_broken_json_returns_unknown():
    body = b"{not json"
    frame_bytes = (
        build_header(MSG_FULL_SERVER_RESPONSE, FLAG_NO_SEQUENCE, SERIALIZATION_JSON, COMPRESSION_NONE)
        + struct.pack(">I", len(body))
        + body
    )

    assert parse_server_frame(frame_bytes).kind == "unknown"


# ==================== 请求体与上下文 ====================


def test_build_bigmodel_request_uses_full_and_utterances():
    payload = build_bigmodel_request(uid="att-desktop", language="zh")

    assert payload["request"]["result_type"] == "full"
    assert payload["request"]["show_utterances"] is True
    assert payload["audio"]["format"] == "pcm"
    assert payload["audio"]["rate"] == 16000
    assert payload["audio"]["bits"] == 16
    assert payload["audio"]["channel"] == 1
    assert payload["audio"]["language"] == "zh-CN"
    # 首录不带 corpus 冗余字段
    assert "corpus" not in payload["request"]


def test_build_bigmodel_request_context_is_json_string():
    payload = build_bigmodel_request(context_text="上一次我们聊了版本范围")

    context = payload["request"]["corpus"]["context"]
    assert isinstance(context, str)
    decoded = json.loads(context)
    assert decoded["context_type"] == "dialog_ctx"
    assert decoded["context_data"][0]["text"] == "上一次我们聊了版本范围"


def test_build_bigmodel_request_end_window_size_optional():
    payload = build_bigmodel_request(end_window_size=800, force_to_speech_time=1000)

    assert payload["request"]["end_window_size"] == 800
    assert payload["request"]["force_to_speech_time"] == 1000

    # 0 表示不设置，不能把 end_window_size=0 发给服务端
    assert "end_window_size" not in build_bigmodel_request(end_window_size=0)["request"]


def test_truncate_context_keeps_tail_by_code_point():
    text = "开" * (ASR_CONTEXT_MAX_CHARS + 10) + "尾部"
    trimmed = truncate_context_text(text)

    assert len(trimmed) == ASR_CONTEXT_MAX_CHARS
    assert trimmed.endswith("尾部")


def test_truncate_context_does_not_break_surrogate_pair():
    text = "🎙" * (ASR_CONTEXT_MAX_CHARS + 5)
    trimmed = truncate_context_text(text)

    assert trimmed == "🎙" * ASR_CONTEXT_MAX_CHARS


def test_build_dialogue_context_json_empty_returns_empty_string():
    assert build_dialogue_context_json("") == ""
    assert build_dialogue_context_json("   ").strip() != ""  # 空白也是内容，不做 strip 语义变更


# ==================== definite 前缀推进 ====================


def test_advance_prefix_commits_new_definite_utterance():
    utterances = (
        SaucUtterance(text="第一句。", definite=True),
        SaucUtterance(text="第二句", definite=False),
    )

    advance = advance_definite_prefix((), utterances)

    assert advance.committed == ("第一句。",)
    assert [item.text for item in advance.new_finals] == ["第一句。"]
    assert advance.pending == "第二句"


def test_advance_prefix_progresses_when_cumulative_text_unchanged():
    """历史缺陷：text 没变但句子变 definite，必须仍然推进（否则全程零 final）。"""
    before = (SaucUtterance(text="第一句。", definite=False),)
    after = (SaucUtterance(text="第一句。", definite=True),)

    first = advance_definite_prefix((), before)
    second = advance_definite_prefix(first.committed, after)

    assert first.committed == ()
    assert second.committed == ("第一句。",)
    assert [item.text for item in second.new_finals] == ["第一句。"]
    assert second.pending == ""


def test_advance_prefix_does_not_confirm_without_utterances():
    """拿不到句级信息就不确认：调用方只保留 pending。"""
    advance = advance_definite_prefix(("第一句。",), ())

    assert advance.committed == ("第一句。",)
    assert advance.new_finals == ()
    assert advance.pending == ""


def test_advance_prefix_shrinks_when_server_regresses():
    advance = advance_definite_prefix(("第一句。", "第二句。"), (SaucUtterance(text="第一句。", definite=True),))

    assert advance.committed == ("第一句。",)
    assert advance.new_finals == ()


def test_advance_prefix_keeps_duplicates_out():
    """同一批 definite 反复下发时不能重复提交。"""
    utterances = (
        SaucUtterance(text="第一句。", definite=True),
        SaucUtterance(text="第二句。", definite=True),
    )

    first = advance_definite_prefix((), utterances)
    second = advance_definite_prefix(first.committed, utterances)

    assert first.committed == ("第一句。", "第二句。")
    assert second.committed == ("第一句。", "第二句。")
    assert second.new_finals == ()


def test_pending_from_cumulative_text_strips_committed_prefix():
    assert pending_from_cumulative_text(("第一句。",), "第一句。第二句") == "第二句"
    # 服务端回退（不再以已确认前缀开头）时整段作为中间结果
    assert pending_from_cumulative_text(("第一句。",), "换了一版文本") == "换了一版文本"


# ==================== 错误归一化 ====================


@pytest.mark.parametrize(
    ("code", "kind", "retryable"),
    (
        (20000000, KIND_NONE, False),
        (45000001, KIND_PROTOCOL, False),
        (45000002, KIND_INPUT, False),
        (45000081, KIND_NETWORK, True),
        (45000151, KIND_INPUT, False),
        (55000031, KIND_QUOTA, True),
        (55000010, KIND_SERVER, True),
        (12345678, KIND_PROTOCOL, False),
    ),
)
def test_classify_sauc_error_table(code, kind, retryable):
    info = classify_sauc_error(code)

    assert info.kind == kind
    assert info.retryable is retryable
    assert info.provider_code == code
    if kind != KIND_NONE:
        assert info.next_action


def test_classify_sauc_error_keeps_server_message():
    info = classify_sauc_error(45000001, "missing audio")

    assert "missing audio" in info.message
    assert "45000001" not in info.message  # 文案面向用户，错误码只放进 provider_code


@pytest.mark.parametrize(
    ("code", "kind", "retryable"),
    (
        (None, KIND_NETWORK, True),
        (1000, KIND_NONE, False),
        (1002, KIND_PROTOCOL, False),
        (1007, KIND_PROTOCOL, False),
        (1008, KIND_PROTOCOL, False),
        (4401, KIND_PROTOCOL, False),
        (1011, KIND_SERVER, True),
        (1013, KIND_SERVER, True),
        (1006, KIND_NETWORK, True),
        (1015, KIND_NETWORK, True),
    ),
)
def test_classify_close_code_table(code, kind, retryable):
    info = classify_close_code(code)

    assert info.kind == kind
    assert info.retryable is retryable


@pytest.mark.parametrize(
    ("status", "kind", "retryable"),
    (
        (401, KIND_AUTH, False),
        (403, KIND_AUTH, False),
        (404, KIND_PROTOCOL, False),
        (429, KIND_QUOTA, True),
        (500, KIND_SERVER, True),
        (503, KIND_SERVER, True),
        (302, KIND_NETWORK, True),
    ),
)
def test_classify_handshake_status_table(status, kind, retryable):
    info = classify_handshake_status(status)

    assert info.kind == kind
    assert info.retryable is retryable


def test_classify_handshake_403_explains_resource_id_rule():
    """403 最常见原因是 Resource ID 不匹配，文案必须直接给出规律。"""
    info = classify_handshake_status(403)

    assert "volc.bigasr.sauc" in info.next_action
    assert "volc.seedasr.sauc" in info.next_action


def test_message_type_constants_match_protocol():
    assert MSG_FULL_CLIENT_REQUEST == 0b0001
    assert MSG_FULL_SERVER_RESPONSE == 0b1001
    assert MSG_SERVER_ERROR_RESPONSE == 0b1111
