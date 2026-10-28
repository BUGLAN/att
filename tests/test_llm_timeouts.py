"""timeouts 派生测试：三个阶段预算的取值规则。

锁死的点（docs/OpenLess-调研与借鉴清单.md §4.2 / §6-P2.8）：
1. 配置值是总预算的**下界**——用户显式调大超时永远生效；
2. 总预算按输入长度放大（20 + 0.03×字数），封顶 600（与 ModelParams 上限一致）；
3. 首 token 预算独立推导（30 + 0.05×字数），但不超过总预算；
4. 块间空闲固定 30s，同样不超过总预算。
"""
from __future__ import annotations

from app.llm.timeouts import MAX_TIMEOUT_S, derive_timeouts


def test_config_timeout_is_the_floor_for_short_text():
    """短文本：配置值（33s）大于推导值（约 20s），配置生效。"""
    timeouts = derive_timeouts(base_timeout_s=33, input_chars=4)
    assert timeouts.request_s == 33
    # 首 token = min(33, ceil(30 + 0.05×4)=31) = 31；空闲 = min(33, 30) = 30
    assert timeouts.first_token_s == 31
    assert timeouts.chunk_idle_s == 30


def test_long_text_scales_request_budget_beyond_config():
    """长转写：推导值超过配置值时按字数放大（几千字不再被 60s 定值掐断）。"""
    timeouts = derive_timeouts(base_timeout_s=60, input_chars=5_000)
    # 20 + 0.03×5000 = 170 > 60
    assert timeouts.request_s == 170
    # 首 token = min(170, ceil(30 + 0.05×5000)=280) → 受总预算约束
    assert timeouts.first_token_s == 170
    assert timeouts.chunk_idle_s == 30


def test_request_budget_is_capped_at_600():
    """超长输入也不能突破 ModelParams.timeout_s 的上限。"""
    timeouts = derive_timeouts(base_timeout_s=60, input_chars=30_000)
    # 20 + 0.03×30000 = 920 → 封顶 600
    assert timeouts.request_s == MAX_TIMEOUT_S == 600
    assert timeouts.first_token_s == 600


def test_first_token_uses_its_own_slope_when_roomy():
    """配置很大时，首 token 按自己的斜率取值（30 + 0.05×字数）。"""
    timeouts = derive_timeouts(base_timeout_s=200, input_chars=1_000)
    # 总预算 = max(200, 50) = 200；首 token = min(200, ceil(80)) = 80
    assert timeouts.request_s == 200
    assert timeouts.first_token_s == 80


def test_zero_char_input_gets_baseline_budgets():
    timeouts = derive_timeouts(base_timeout_s=10, input_chars=0)
    # 总预算 = max(10, 20) = 20；首 token = min(20, 30) = 20；空闲 = min(20, 30) = 20
    assert timeouts.request_s == 20
    assert timeouts.first_token_s == 20
    assert timeouts.chunk_idle_s == 20


def test_fractional_budgets_round_up():
    """0.03×334 = 10.02 → 总预算 30.02s 向上取整为 31，不向下少给一秒。"""
    timeouts = derive_timeouts(base_timeout_s=5, input_chars=334)
    assert timeouts.request_s == 31
