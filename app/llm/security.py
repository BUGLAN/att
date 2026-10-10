"""LLM 敏感信息脱敏工具。

硬约束：日志、异常文案与接口返回里都不得出现密钥（见 AGENTS.md §3.4）。

OpenAI 兼容上游（DeepSeek 等）在鉴权失败时会把密钥回显在错误体里，例如
``Authentication Fails, Your api key: sk-xxx is invalid``。这类异常经 ``str()``
展开后，会把明文密钥带进日志、记录 JSON 与前端提示，因此所有对外暴露的
错误文案都必须先过这里。
"""
from __future__ import annotations

import re
from typing import Iterable

MASK = "***"

# 形如 api key: xxx / api_key=xxx / access_token: xxx。
# 强制要求 : 或 = 分隔符，避免把 "authorization failed" 这类普通短语误判成密钥。
_KEY_VALUE_PATTERN = re.compile(
    r"(?i)\b(api[\s_-]?key|access[\s_-]?token|auth[\s_-]?token)\b"
    r"(\s*[:=]\s*)"
    r"(['\"]?)"
    r"([^\s'\"]{3,})"
    r"\3"
)

# 形如 Authorization: Bearer sk-xxx
_BEARER_PATTERN = re.compile(r"(?i)\b(bearer)\s+([A-Za-z0-9_\-\.\+/]{6,})")

# 裸的 OpenAI / Anthropic 风格密钥（sk-xxx、sk-ant-xxx）
_BARE_KEY_PATTERN = re.compile(r"\bsk-[A-Za-z0-9_\-]{4,}")


def redact_secrets(text: object, secrets: Iterable[str] = ()) -> str:
    """把文本中的密钥替换成掩码后返回。

    Args:
        text: 待脱敏内容，非字符串会先转成字符串（异常对象可直接传入）。
        secrets: 已知的密钥字面量。按值精确替换最可靠，优先于模式匹配；
            长度不足 3 的取值跳过，避免误伤普通文本。
    """
    if text is None:
        return ""

    result = str(text)

    for secret in secrets:
        secret_text = str(secret or "")
        if len(secret_text) >= 3:
            result = result.replace(secret_text, MASK)

    result = _KEY_VALUE_PATTERN.sub(
        lambda match: f"{match.group(1)}{match.group(2)}{match.group(3)}{MASK}{match.group(3)}",
        result,
    )
    result = _BEARER_PATTERN.sub(lambda match: f"{match.group(1)} {MASK}", result)
    return _BARE_KEY_PATTERN.sub(MASK, result)
