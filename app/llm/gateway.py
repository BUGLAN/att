"""LiteLLMGateway：LiteLLM acompletion 的统一调用点。

职责（docs/LLM配置模块重设计.md §4）：
- 每次请求强制注入 `timeout` 与 `num_retries`（修复已知问题 #4：SDK 默认 600s，
  网络黑洞时优化最长挂 10 分钟）；
- 统一错误分级（app/llm/errors.py）与密钥脱敏；
- 服务商前缀在当前 LiteLLM 版本未收录时，自动回退 openai/ + 兼容端点重试一次
  （如 doubao，见 registry.ProviderSpec.openai_compat_base）；
- 验证类操作（档案/模型/草稿）与上游模型列表拉取。

注意：litellm 首次 import 约 2.5s（实测 1.106.2），**必须懒加载**，
不能放到模块层，否则会拖慢整个后端启动。
"""
from __future__ import annotations

import re
import time
from dataclasses import dataclass, field
from typing import Any, AsyncIterator, Optional

import httpx

from app.llm.errors import (
    CODE_OK,
    CODE_UNSUPPORTED_PROVIDER,
    LLMError,
    classify_exception,
)
from app.llm.registry import ProviderSpec, get_provider_spec
from app.utils.logging import get_logger

logger = get_logger(__name__)

# 上游模型列表拉取的固定超时（验证类操作用短超时，避免设置页长时间转圈）
VERIFY_TIMEOUT_S = 15


@dataclass
class GatewayChatResult:
    """一次补全调用的归一化结果。"""

    text: str
    model_string: str
    latency_ms: int
    prompt_tokens: int = 0
    completion_tokens: int = 0
    total_tokens: int = 0
    # 部分 OpenAI 兼容上游会把“看不见的思考 token”单列；没有该字段时保持 0
    reasoning_tokens: int = 0
    # stop=正常结束；length=输出达到 max_tokens 被截断（思考模式模型上很容易发生，
    # 思考 token 也计入 max_tokens）——截断必须可见，不能静默丢尾巴
    finish_reason: str = "stop"

    @property
    def truncated(self) -> bool:
        return self.finish_reason == "length"


@dataclass
class GatewayStreamChunk:
    """流式补全的一块增量：text 可能为空（仅携带 finish_reason / usage）。"""

    text: str = ""
    model_string: str = ""
    finish_reason: str = ""
    prompt_tokens: int = 0
    completion_tokens: int = 0
    total_tokens: int = 0
    reasoning_tokens: int = 0

    @property
    def truncated(self) -> bool:
        return self.finish_reason == "length"


@dataclass
class VerifyOutcome:
    """验证类操作的统一结果（对应 API 的验证端点响应）。"""

    ok: bool
    error_code: Optional[str] = None
    message: Optional[str] = None
    latency_ms: Optional[int] = None
    remote_models: list[str] = field(default_factory=list)

    def to_dict(self) -> dict:
        return {
            "ok": self.ok,
            "error_code": None if self.ok else (self.error_code or "unknown"),
            "message": self.message,
            "latency_ms": self.latency_ms,
            "remote_models": self.remote_models,
        }


def _extract_model_name(model_string: str) -> str:
    """deepseek/deepseek-flash -> deepseek-flash（回退组装时剥掉旧前缀）。"""
    return model_string.split("/", 1)[-1] if "/" in model_string else model_string


def _unsupported_provider_spec(model_string: str) -> Optional[ProviderSpec]:
    """判断模型串的前缀是否属于「注册表里有 openai 兼容兜底」的服务商。"""
    prefix = model_string.split("/", 1)[0] if "/" in model_string else ""
    for spec_id, _spec in _SPEC_CACHE.items():
        if _spec.litellm_prefix == prefix:
            spec = get_provider_spec(spec_id)
            if spec and spec.openai_compat_base and spec.id != "custom":
                return spec
    return None


_SPEC_CACHE: dict[str, ProviderSpec] = {}


def _warm_spec_cache() -> None:
    if not _SPEC_CACHE:
        from app.llm.registry import PROVIDER_MAP

        _SPEC_CACHE.update(PROVIDER_MAP)


class LiteLLMGateway:
    """所有 LLM 调用都从这里走：一个入口，统一超时/重试/分级/脱敏。"""

    async def chat(
        self,
        *,
        model_string: str,
        messages: list[dict[str, str]],
        api_key: str = "",
        api_base: Optional[str] = None,
        extra_headers: Optional[dict[str, str]] = None,
        temperature: float = 0.3,
        max_tokens: int = 4096,
        timeout_s: int = 60,
        retries: int = 2,
    ) -> GatewayChatResult:
        """执行一次补全。失败抛 LLMError（已分级、已脱敏）。"""
        _warm_spec_cache()
        # _chat_once 遇到 unsupported_provider 时返回 None（不抛），兜底与否在这里决策
        result = await self._chat_once(
            model_string=model_string,
            messages=messages,
            api_key=api_key,
            api_base=api_base,
            extra_headers=extra_headers,
            temperature=temperature,
            max_tokens=max_tokens,
            timeout_s=timeout_s,
            retries=retries,
        )
        if result is not None:
            return result

        fallback = self._plan_compat_fallback(model_string, api_base)
        if fallback is None:
            raise LLMError(
                CODE_UNSUPPORTED_PROVIDER,
                f"无法识别的服务商前缀，且未配置可用于兜底的 API Base：{model_string}",
            )

        # 存在兼容兜底：按计划换 openai/ 串重试一次
        fallback_model, fallback_base = fallback
        logger.warning(
            "服务商前缀未被当前 LiteLLM 版本收录，回退 openai 兼容端点重试",
            model_string=model_string,
            fallback_model=fallback_model,
        )
        retry = await self._chat_once(
            model_string=fallback_model,
            messages=messages,
            api_key=api_key,
            api_base=fallback_base,
            extra_headers=extra_headers,
            temperature=temperature,
            max_tokens=max_tokens,
            timeout_s=timeout_s,
            retries=retries,
        )
        if retry is None:
            raise LLMError(CODE_UNSUPPORTED_PROVIDER, f"服务商前缀不可用且兼容端点也失败：{model_string}")
        return retry

    async def chat_stream(
        self,
        *,
        model_string: str,
        messages: list[dict[str, str]],
        api_key: str = "",
        api_base: Optional[str] = None,
        extra_headers: Optional[dict[str, str]] = None,
        temperature: float = 0.3,
        max_tokens: int = 4096,
        timeout_s: int = 60,
        retries: int = 2,
    ) -> AsyncIterator[GatewayStreamChunk]:
        """流式补全：逐块 yield，失败抛 LLMError（已分级、已脱敏）。

        与 chat() 共用「前缀未收录时回退 openai/ 兼容端点」的策略；调用方应把
        「第一条增量之前就失败」视为可降级为 chat() 的信号。
        """
        _warm_spec_cache()
        stream = await self._open_stream_once(
            model_string=model_string,
            messages=messages,
            api_key=api_key,
            api_base=api_base,
            extra_headers=extra_headers,
            temperature=temperature,
            max_tokens=max_tokens,
            timeout_s=timeout_s,
            retries=retries,
        )
        if stream is None:
            fallback = self._plan_compat_fallback(model_string, api_base)
            if fallback is None:
                raise LLMError(
                    CODE_UNSUPPORTED_PROVIDER,
                    f"无法识别的服务商前缀，且未配置可用于兜底的 API Base：{model_string}",
                )

            fallback_model, fallback_base = fallback
            logger.warning(
                "服务商前缀未被当前 LiteLLM 版本收录，流式回退 openai 兼容端点重试",
                model_string=model_string,
                fallback_model=fallback_model,
            )
            stream = await self._open_stream_once(
                model_string=fallback_model,
                messages=messages,
                api_key=api_key,
                api_base=fallback_base,
                extra_headers=extra_headers,
                temperature=temperature,
                max_tokens=max_tokens,
                timeout_s=timeout_s,
                retries=retries,
            )
            if stream is None:
                raise LLMError(
                    CODE_UNSUPPORTED_PROVIDER,
                    f"服务商前缀不可用且兼容端点也失败：{model_string}",
                )
            model_string = fallback_model

        try:
            async for chunk in stream:
                parsed = self._parse_stream_chunk(chunk, model_string)
                if parsed is not None:
                    yield parsed
        except Exception as exc:
            err = classify_exception(exc, (api_key,) if api_key else ())
            logger.warning(
                "LLM 流式调用失败",
                model=model_string,
                error_code=err.code,
                error=err.message,
            )
            raise err from exc

    async def _open_stream_once(
        self,
        *,
        model_string: str,
        messages: list[dict[str, str]],
        api_key: str,
        api_base: Optional[str],
        extra_headers: Optional[dict[str, str]],
        temperature: float,
        max_tokens: int,
        timeout_s: int,
        retries: int,
    ) -> Optional[Any]:
        """打开一次流。前缀未收录返回 None；其余失败抛 LLMError。"""
        import litellm  # 懒加载：见模块 docstring

        kwargs: dict[str, Any] = {
            "model": model_string,
            "messages": messages,
            "temperature": temperature,
            "max_tokens": max_tokens,
            "timeout": float(timeout_s),
            "num_retries": int(retries),
            "stream": True,
            # 让 OpenAI 兼容上游在最后一块带 usage；不支持时下面会去掉重试一次
            "stream_options": {"include_usage": True},
        }
        if api_key:
            kwargs["api_key"] = api_key
        if api_base:
            kwargs["api_base"] = api_base
        if extra_headers:
            kwargs["extra_headers"] = extra_headers

        try:
            return await litellm.acompletion(**kwargs)
        except Exception as exc:
            err = classify_exception(exc, (api_key,) if api_key else ())
            if err.code == CODE_UNSUPPORTED_PROVIDER:
                return None

            # 少数 OpenAI 兼容代理不认 stream_options；去掉后重试一次，仍失败再上抛
            if "stream_options" in str(exc).lower():
                kwargs.pop("stream_options", None)
                try:
                    return await litellm.acompletion(**kwargs)
                except Exception as retry_exc:
                    retry_err = classify_exception(
                        retry_exc, (api_key,) if api_key else ()
                    )
                    if retry_err.code == CODE_UNSUPPORTED_PROVIDER:
                        return None
                    raise retry_err from retry_exc

            raise err from exc

    @staticmethod
    def _usage_counts(usage: Any) -> tuple[int, int, int, int]:
        """从 LiteLLM usage（对象或 dict）中取 prompt/completion/total/reasoning。"""
        if usage is None:
            return (0, 0, 0, 0)

        def value(obj: Any, name: str) -> Any:
            if isinstance(obj, dict):
                return obj.get(name)
            return getattr(obj, name, None)

        prompt = int(value(usage, "prompt_tokens") or 0)
        completion = int(value(usage, "completion_tokens") or 0)
        total = int(value(usage, "total_tokens") or 0)
        details = value(usage, "completion_tokens_details")
        reasoning = int(value(details, "reasoning_tokens") or 0) if details else 0
        return (prompt, completion, total, reasoning)

    @classmethod
    def _parse_stream_chunk(
        cls, chunk: Any, model_string: str
    ) -> Optional[GatewayStreamChunk]:
        """把 LiteLLM 的一块响应归一成 GatewayStreamChunk；空块返回 None。"""
        choices = getattr(chunk, "choices", None) or []
        text = ""
        finish_reason = ""

        if choices:
            choice = choices[0]
            delta = getattr(choice, "delta", None)
            if delta is not None:
                content = getattr(delta, "content", None)
                if isinstance(content, str):
                    text = content
            finish_reason = str(getattr(choice, "finish_reason", "") or "")

        usage = getattr(chunk, "usage", None)
        prompt, completion, total, reasoning = cls._usage_counts(usage)

        if not text and not finish_reason and not total:
            return None

        return GatewayStreamChunk(
            text=text,
            model_string=model_string,
            finish_reason=finish_reason,
            prompt_tokens=prompt,
            completion_tokens=completion,
            total_tokens=total,
            reasoning_tokens=reasoning,
        )

    def _plan_compat_fallback(
        self, model_string: str, api_base: Optional[str]
    ) -> Optional[tuple[str, str]]:
        """为 unsupported_provider 错误计算兜底方案。

        - 注册表前缀有 openai 兼容兜底端点（如 doubao）：剥掉旧前缀换 `openai/<裸名>`；
        - 配了 api_base（自定义网关，其模型 ID 常自带斜杠，如 `group/xxx`，
          首段 `group` 会被 LiteLLM 误当服务商）：把**完整模型串**包进 `openai/` 重试，
          调用时 LiteLLM 会剥掉这一层并把原样 ID 交给网关；
        - 都不满足：返回 None，由调用方抛出原始错误。
        """
        spec = _unsupported_provider_spec(model_string)
        if spec is not None:
            base = (api_base or "").strip() or spec.openai_compat_base
            return f"openai/{_extract_model_name(model_string)}", base
        if (api_base or "").strip():
            return f"openai/{model_string}", (api_base or "").strip()
        return None

    async def _chat_once(
        self,
        *,
        model_string: str,
        messages: list[dict[str, str]],
        api_key: str,
        api_base: Optional[str],
        extra_headers: Optional[dict[str, str]],
        temperature: float,
        max_tokens: int,
        timeout_s: int,
        retries: int,
    ) -> Optional[GatewayChatResult]:
        """单次调用；「前缀未收录且可兜底」时返回 None 交给上层重试，其余失败抛 LLMError。"""
        import litellm  # 懒加载：见模块 docstring

        kwargs: dict[str, Any] = {
            "model": model_string,
            "messages": messages,
            "temperature": temperature,
            "max_tokens": max_tokens,
            # 已知问题 #4 的修复点：超时与重试必须显式注入，绝不依赖 SDK 默认值
            "timeout": float(timeout_s),
            "num_retries": int(retries),
        }
        if api_key:
            kwargs["api_key"] = api_key
        if api_base:
            kwargs["api_base"] = api_base
        if extra_headers:
            kwargs["extra_headers"] = extra_headers

        start = time.perf_counter()
        try:
            response = await litellm.acompletion(**kwargs)
        except Exception as exc:
            err = classify_exception(exc, (api_key,) if api_key else ())
            if err.code == CODE_UNSUPPORTED_PROVIDER:
                # 兜底与否由上层 _plan_compat_fallback 决策（注册表兼容端点 / 自定义 api_base）
                return None
            logger.warning(
                "LLM 调用失败",
                model=model_string,
                error_code=err.code,
                error=err.message,  # 已脱敏文案
            )
            raise err from exc

        latency_ms = int((time.perf_counter() - start) * 1000)
        try:
            text = response.choices[0].message.content or ""
        except (AttributeError, IndexError) as exc:
            raise LLMError("provider_error", "上游返回了无法解析的响应结构") from exc

        usage = getattr(response, "usage", None)
        completion_details = getattr(usage, "completion_tokens_details", None)
        result = GatewayChatResult(
            text=text,
            model_string=model_string,
            latency_ms=latency_ms,
            prompt_tokens=int(getattr(usage, "prompt_tokens", 0) or 0),
            completion_tokens=int(getattr(usage, "completion_tokens", 0) or 0),
            total_tokens=int(getattr(usage, "total_tokens", 0) or 0),
            reasoning_tokens=int(getattr(completion_details, "reasoning_tokens", 0) or 0),
            finish_reason=str(getattr(response.choices[0], "finish_reason", "") or "stop"),
        )
        logger.info(
            "LLM 调用成功",
            model=model_string,
            latency_ms=latency_ms,
            prompt_tokens=result.prompt_tokens,
            completion_tokens=result.completion_tokens,
            total_tokens=result.total_tokens,
            # 思考 token 不计入可见输出，但会计费/计入耗时；单列出来才能解释“短文本为何要 7 秒”
            reasoning_tokens=result.reasoning_tokens,
            finish_reason=result.finish_reason,
        )
        if result.truncated:
            logger.warning(
                "LLM 输出达到 max_tokens 上限被截断（思考 token 也计入），建议调大该模型的 Max Tokens",
                model=model_string,
                max_tokens=max_tokens,
            )
        return result

    # ==================== 验证类 ====================

    async def verify_model(
        self,
        *,
        model_string: str,
        api_key: str = "",
        api_base: Optional[str] = None,
        extra_headers: Optional[dict[str, str]] = None,
        timeout_s: int = VERIFY_TIMEOUT_S,
    ) -> VerifyOutcome:
        """轻量验证：约 5 token 的补全请求，验证「档案凭据 + 模型名」都可用。"""
        start = time.perf_counter()
        try:
            await self.chat(
                model_string=model_string,
                messages=[{"role": "user", "content": "Hi"}],
                api_key=api_key,
                api_base=api_base,
                extra_headers=extra_headers,
                max_tokens=5,
                timeout_s=timeout_s,
                retries=0,
            )
        except LLMError as err:
            return VerifyOutcome(ok=False, error_code=err.code, message=err.user_message)
        return VerifyOutcome(ok=True, latency_ms=int((time.perf_counter() - start) * 1000))

    async def verify_profile(
        self,
        *,
        spec: ProviderSpec,
        api_key: str = "",
        api_base: Optional[str] = None,
        extra_headers: Optional[dict[str, str]] = None,
    ) -> VerifyOutcome:
        """档案级验证：鉴权 + 尽力列取上游模型列表（不支持列取时视为通过）。"""
        outcome = await self.verify_model(
            model_string=self._probe_model_string(spec),
            api_key=api_key,
            api_base=api_base,
            extra_headers=extra_headers,
        )
        if not outcome.ok:
            # 档案级探测用的是建议模型名：模型不存在不代表凭据有问题
            if outcome.error_code == "model_not_found":
                outcome = VerifyOutcome(ok=True, latency_ms=outcome.latency_ms)
            else:
                return outcome

        if spec.supports_model_listing:
            models, list_err = await self.list_remote_models(
                api_key=api_key,
                api_base=api_base or spec.default_base_url,
                extra_headers=extra_headers,
            )
            if models is not None:
                outcome.remote_models = models
            elif list_err:
                # 列取失败不推翻凭据验证结论，只提示手输
                outcome.message = f"模型列表拉取失败（{list_err}），可手动输入模型名"
        return outcome

    def _probe_model_string(self, spec: ProviderSpec) -> str:
        """档案级探测用的模型名：优先注册表建议，兜底一个通用名。"""
        if spec.suggested_models:
            return f"{spec.litellm_prefix}/{spec.suggested_models[0]}"
        return f"{spec.litellm_prefix}/gpt-3.5-turbo"

    async def list_remote_models(
        self,
        *,
        api_key: str = "",
        api_base: str,
        extra_headers: Optional[dict[str, str]] = None,
        timeout_s: int = VERIFY_TIMEOUT_S,
    ) -> tuple[Optional[list[str]], Optional[str]]:
        """拉取 OpenAI 兼容 GET /models。成功返回 (ids, None)，失败返回 (None, 已脱敏原因)。"""
        base = (api_base or "").rstrip("/")
        if not base:
            return None, "未配置 API Base，无法拉取模型列表"
        headers: dict[str, str] = {"Accept": "application/json"}
        if api_key:
            headers["Authorization"] = f"Bearer {api_key}"
        if extra_headers:
            headers.update(extra_headers)
        try:
            async with httpx.AsyncClient(timeout=float(timeout_s)) as client:
                resp = await client.get(f"{base}/models", headers=headers)
                resp.raise_for_status()
                data = resp.json()
        except Exception as exc:
            from app.llm.security import redact_secrets

            return None, redact_secrets(exc, (api_key,) if api_key else ())
        items = data.get("data") if isinstance(data, dict) else data
        if not isinstance(items, list):
            return None, "模型列表响应格式无法解析"
        ids: list[str] = []
        for item in items:
            model_id = item.get("id") if isinstance(item, dict) else None
            if isinstance(model_id, str) and model_id:
                ids.append(model_id)
        return sorted(set(ids)), None


# 模型串里允许出现的字符（用于手输校验的错误信息提示，宽松处理）
_MODEL_NAME_RE = re.compile(r"^[A-Za-z0-9._:/-]+$")


def is_valid_model_string(model_string: str) -> bool:
    """模型串的轻量合法性检查（不发起网络请求）。"""
    return bool(model_string) and bool(_MODEL_NAME_RE.match(model_string))


# 模块级单例
_gateway: Optional[LiteLLMGateway] = None


def get_llm_gateway() -> LiteLLMGateway:
    global _gateway
    if _gateway is None:
        _gateway = LiteLLMGateway()
    return _gateway
