"""自适应超时派生：按输入长度把「模型级定值超时」拆成三个阶段预算。

为什么要有这一层（做法借鉴同类开源产品 openless 的自适应超时，结论见
`docs/OpenLess-调研与借鉴清单.md` §4.2）：

一个总超时无法区分「模型还在写、只是文本长」和「服务端卡死」两种情况。
`ModelParams.timeout_s` 是每模型一个定值：长转写（几千字）按 60s 配置必然在
生成中途被掐断；反过来短文本配 300s 又会在服务端卡死时白等 5 分钟。

派生规则（数值按文档量级改写）：

- **生成总预算**（请求级，非流式与流式共用）：
  `max(配置值, 20 + 0.03 × 输入字数)`，封顶 600s（与 `ModelParams.timeout_s`
  的上限一致）。配置值是下界——用户显式调大超时永远生效；
- **首 token 预算**（仅流式）：`30 + 0.05 × 输入字数`，且不超过生成总预算。
  首 token 等待含思考模型首包前的推理时间，比块间空闲宽裕得多；
- **块间空闲预算**（仅流式）：固定 30s。相邻增量之间的间隔只反映服务端是否
  卡死，与输入长度无关，不需要按字数放大。

本模块是纯函数，不碰网络与配置，便于单测（`tests/test_llm_timeouts.py`）。
"""
from __future__ import annotations

import math
from dataclasses import dataclass

# 与 ModelParams.timeout_s 的 pydantic 校验上限（le=600）保持一致
MAX_TIMEOUT_S = 600

# 生成阶段：基线 20s + 每字符 0.03s（≈每秒 33 字的处理预算，覆盖长转写）
_GENERATION_BASE_S = 20.0
_GENERATION_PER_CHAR_S = 0.03

# 首 token 阶段：基线 30s + 每字符 0.05s（含思考模型开流后的推理等待）
_FIRST_TOKEN_BASE_S = 30.0
_FIRST_TOKEN_PER_CHAR_S = 0.05

# 块间空闲：固定值，不随输入长度变化
_CHUNK_IDLE_S = 30.0


@dataclass(frozen=True)
class RequestTimeouts:
    """一次请求的三个阶段超时（单位：秒，均为整数）。"""

    # 整次请求的总预算（LiteLLM 的 timeout 参数；流式同样是总上限）
    request_s: int
    # 流式专用：开流到第一条增量的等待上限
    first_token_s: int
    # 流式专用：相邻两条增量之间的空闲上限
    chunk_idle_s: int


def derive_timeouts(base_timeout_s: int, input_chars: int) -> RequestTimeouts:
    """按输入长度派生三个阶段超时。

    Args:
        base_timeout_s: 模型条目里配置的定值超时（作为总预算的下界）。
        input_chars: 本次输入的字符数（正文长度，不含提示词）。
    """
    chars = max(0, int(input_chars))
    generation = max(
        float(base_timeout_s),
        _GENERATION_BASE_S + _GENERATION_PER_CHAR_S * chars,
    )
    request_s = min(math.ceil(generation), MAX_TIMEOUT_S)

    first_token = _FIRST_TOKEN_BASE_S + _FIRST_TOKEN_PER_CHAR_S * chars
    first_token_s = min(request_s, math.ceil(first_token))

    chunk_idle_s = min(request_s, int(_CHUNK_IDLE_S))

    return RequestTimeouts(
        request_s=request_s,
        first_token_s=first_token_s,
        chunk_idle_s=chunk_idle_s,
    )
