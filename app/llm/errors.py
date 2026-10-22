"""LLM 错误分级：把各上游的异常归一成有限的 error_code，并生成中文建议文案。

设计约束：
- 本模块**不 import litellm**（保持懒加载边界，也让单测可以构造同名的假异常），
  通过异常类名与 status_code 属性做鸭子类型判定；
- message 一律先经 `redact_secrets` 脱敏——上游鉴权失败时会在错误体里回显完整
  密钥（DeepSeek/OpenAI 实测如此），绝不能让它流入日志/记录/前端（AGENTS.md §3.4）。
"""
from __future__ import annotations

from typing import Optional

from app.llm.security import redact_secrets

# 错误分级（对外契约，前端按 code 显示建议动作）
CODE_OK = "ok"
CODE_AUTH = "auth_failed"
CODE_MODEL_NOT_FOUND = "model_not_found"
CODE_QUOTA = "quota_exceeded"
CODE_TIMEOUT = "timeout"
CODE_NETWORK = "network_error"
CODE_PROVIDER = "provider_error"
CODE_UNSUPPORTED_PROVIDER = "unsupported_provider"
CODE_UNKNOWN = "unknown"

ERROR_CODES = {
    CODE_OK,
    CODE_AUTH,
    CODE_MODEL_NOT_FOUND,
    CODE_QUOTA,
    CODE_TIMEOUT,
    CODE_NETWORK,
    CODE_PROVIDER,
    CODE_UNSUPPORTED_PROVIDER,
    CODE_UNKNOWN,
}

# 面向用户的建议文案（P7：失败原因要可执行，而不是一坨异常字符串）
USER_HINTS: dict[str, str] = {
    CODE_AUTH: "检查 API Key 是否复制完整、是否被撤销或过期、账户是否欠费",
    CODE_MODEL_NOT_FOUND: "检查模型名是否正确（含服务商前缀），或从模型列表重新拉取",
    CODE_QUOTA: "账户欠费或触发限流，请充值或稍后重试",
    CODE_TIMEOUT: "端点不可达或响应过慢，请检查网络/代理，或在模型参数里调大超时",
    CODE_NETWORK: "网络连接失败，请检查网络与代理设置",
    CODE_PROVIDER: "上游服务故障，请稍后重试",
    CODE_UNSUPPORTED_PROVIDER: "当前接入层版本不支持该服务商前缀，可在档案编辑里改用「自定义 OpenAI 兼容」端点",
    CODE_UNKNOWN: "调用失败，请查看日志获取详情",
}


class LLMError(Exception):
    """带分级的 LLM 调用异常。message 保证已脱敏。"""

    def __init__(self, code: str, message: str, hint: bool = True):
        self.code = code if code in ERROR_CODES else CODE_UNKNOWN
        self.message = message
        self.user_message = f"{message}。建议：{USER_HINTS[self.code]}" if hint and message else (message or USER_HINTS[self.code])
        super().__init__(self.user_message)


# 异常类名 → 错误分级（覆盖 openai SDK 与 litellm 的异常命名，二者高度一致）
_CLASS_NAME_MAP: dict[str, str] = {
    "AuthenticationError": CODE_AUTH,
    "PermissionDeniedError": CODE_AUTH,
    "NotFoundError": CODE_MODEL_NOT_FOUND,
    "RateLimitError": CODE_QUOTA,
    "InsufficientBalanceError": CODE_QUOTA,
    "APIConnectionError": CODE_NETWORK,
    "APIConnectionTimeoutError": CODE_TIMEOUT,
    "APITimeoutError": CODE_TIMEOUT,
    "UnprocessableEntityError": CODE_PROVIDER,
    "InternalServerError": CODE_PROVIDER,
    "ServiceUnavailableError": CODE_PROVIDER,
    "BadRequestError": CODE_PROVIDER,
}


def _status_code(exc: BaseException) -> Optional[int]:
    return getattr(exc, "status_code", None)


def classify_exception(exc: BaseException, secrets: tuple[str, ...] = ()) -> LLMError:
    """把任意异常归类为 LLMError。secrets 是需要从文案里抹掉的密钥字面量。"""
    # 密钥必须最先参与脱敏，之后所有拼接都基于脱敏后的文本
    safe_text = redact_secrets(exc, secrets) or "调用失败"

    # LiteLLM 收到不认识的前缀时抛 BadRequestError，错误体带 "Unsupported Provider"
    if "unsupported provider" in safe_text.lower() or "llm provider not provided" in safe_text.lower():
        return LLMError(CODE_UNSUPPORTED_PROVIDER, safe_text)

    class_name = exc.__class__.__name__
    code = _CLASS_NAME_MAP.get(class_name, CODE_UNKNOWN)
    status = _status_code(exc)

    # 类名没匹配上时，用 HTTP 状态码兜底分类
    if code == CODE_UNKNOWN and status is not None:
        if status in (401, 403):
            code = CODE_AUTH
        elif status == 404:
            code = CODE_MODEL_NOT_FOUND
        elif status == 429:
            code = CODE_QUOTA
        elif 500 <= status < 600:
            code = CODE_PROVIDER
        elif 400 <= status < 500:
            code = CODE_PROVIDER

    # 文本兜底：网络超时类异常（socket.timeout / httpx.TimeoutException 等）
    if code == CODE_UNKNOWN:
        lowered = safe_text.lower()
        if "timed out" in lowered or "timeout" in lowered:
            code = CODE_TIMEOUT
        elif "connection" in lowered or "network" in lowered or "getaddrinfo" in lowered:
            code = CODE_NETWORK
        elif "invalid api key" in lowered or "unauthorized" in lowered:
            code = CODE_AUTH

    return LLMError(code, safe_text)
