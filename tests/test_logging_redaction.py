"""日志脱敏处理器单测。

锁死的点（AGENTS.md §3.4：日志与异常文案里不得出现密钥）：

真实踩坑：``websockets`` 的 DEBUG 日志会把请求头原样打印，而根日志级别是 DEBUG
（``.env`` 里 ``DEBUG=true``），导致豆包 Access Token 以明文落了 23 行进
``logs/app.log``（形如 ``> X-Api-Key: b1d48988-...``）。这里用与来源无关的兜底
处理器把任何日志事件里的密钥洗掉。
"""
from app.utils.logging import redact_secrets_processor


def test_redacts_api_key_header_in_event():
    """第三方库原样打请求头时，密钥必须被打码。"""
    event = {"event": "> X-Api-Key: b1d48988-2472-4996-84c4-182d2eb6c9d5", "level": "debug"}

    result = redact_secrets_processor(None, "debug", event)

    assert "b1d48988-2472-4996-84c4-182d2eb6c9d5" not in result["event"]
    assert "***" in result["event"]


def test_redacts_nested_structures():
    """事件里嵌套的 dict / list 也要覆盖。"""
    event = {
        "detail": {
            "headers": ["Authorization: Bearer sk-abcdef123456", "keep-me"],
            "count": 2,
        }
    }

    result = redact_secrets_processor(None, "info", event)

    assert "sk-abcdef123456" not in str(result["detail"])
    assert result["detail"]["headers"][1] == "keep-me"
    assert result["detail"]["count"] == 2


def test_keeps_plain_values_untouched():
    """普通内容与非字符串字段不能被动到。"""
    event = {"event": "normal message", "count": 3, "ok": True}

    result = redact_secrets_processor(None, "info", event)

    assert result == {"event": "normal message", "count": 3, "ok": True}
