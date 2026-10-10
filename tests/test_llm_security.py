"""LLM 敏感信息脱敏单测。

锁死的点（AGENTS.md §3.4：日志、异常文案与接口返回里都不得出现密钥）：

1. DeepSeek 鉴权失败的 ``Your api key: xxx is invalid`` 文案会被打码；
2. 裸 ``sk-`` 密钥与 ``Bearer`` 密钥会被打码；
3. 已知密钥字面量按值精确替换（模式匹配兜不住的场景）；
4. 普通短语（``authorization failed`` / ``api key 未配置``）不被误伤。
"""
from app.llm.security import MASK, redact_secrets


def test_redacts_deepseek_auth_error_key():
    """真实形态：上游把密钥回显在 401 错误体里。"""
    message = (
        "Error code: 401 - {'error': {'message': "
        "'Authentication Fails, Your api key: sk-421baeeacc2d40e5afc065d9137c6be6 is invalid'}}"
    )

    redacted = redact_secrets(message)

    assert "sk-421baeeacc2d40e5afc065d9137c6be6" not in redacted
    assert MASK in redacted


def test_redacts_bare_and_bearer_keys():
    bearer = redact_secrets("Authorization: Bearer sk-ant-api03-AbCdEf123456")
    assert "sk-ant-api03-AbCdEf123456" not in bearer

    bare = redact_secrets("request failed with sk-live-999888777")
    assert "sk-live-999888777" not in bare


def test_redacts_known_secret_by_value():
    """密钥出现在没有 key: 前缀的位置时，只能靠已知值精确替换。"""
    redacted = redact_secrets("boom: topsecret-value happened", ["topsecret-value"])

    assert "topsecret-value" not in redacted
    assert MASK in redacted


def test_keeps_plain_phrases_intact():
    """不能把普通短语误判成密钥，否则错误提示会变得不可读。"""
    assert redact_secrets("authorization failed for user") == "authorization failed for user"
    assert redact_secrets("api key 未配置") == "api key 未配置"


def test_handles_none_and_exception_objects():
    assert redact_secrets(None) == ""
    assert "boom" in redact_secrets(RuntimeError("boom"))
