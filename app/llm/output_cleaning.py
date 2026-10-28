"""输出清洗：模型不听话时的最后一道兜底。

背景（`docs/OpenLess-调研与借鉴清单.md` §4.1）：提示词已经用清单禁止了
开场白、自述与围栏，但总有模型不守规矩。与其继续往提示词里堆措辞，不如在
后处理里直接剥掉——这是确定安全的文本变换，不依赖模型配合。

三层清洗（对应 openless `output_cleaning.rs` 的前三层；第四层「相邻句近似
重复检测」在原项目里也只做观测不改写，暂不引入）：

1. **剥思考块**：`<think>…</think>`（大小写不敏感、开标签带属性都认；
   `<thinking>` 与未闭合的 `<think>` 不动——后者可能是正文里的普通文本）；
2. **剥成对围栏**：整个输出恰好被一对 ``` 围栏包住时剥掉围栏行；
   围栏出现在正文中间不动（那可能是用户口述的代码）；
3. **循环剥开头套话**：「以下是整理后的内容：」一类前缀，剥到第一个
   句号/冒号/换行（含）为止，最多剥 4 轮（套话嵌套套话）。

安全约束（每一层都必须满足）：

- 前缀后找不到边界符（句号/冒号/换行）→ 不剥，宁可漏剥不可误伤；
- 剥完正文为空 → 该层回退到剥之前的结果（整段都是套话时保留原文，
  不能把输出洗成空串）；**唯一例外**是思考块层——剥完为空说明正文
  本来就只有思考链，此时返回空串由调用方按「无有效正文」处理。

本模块是纯函数，不碰网络与日志，便于单测（`tests/test_output_cleaning.py`）。
"""
from __future__ import annotations

import re

# 思考块：<think> 与 </think> 成对出现才剥。
# - `<think\b[^>]*>`：允许开标签带属性（<think type="reasoning">）；
#   \b 保证 <thinking> 不被误认（thinking 的 i 是单词字符，无边界）；
# - `</think\s*>`：允许闭标签内带空白；
# - 未闭合的 <think>（无对应闭标签）整段不动，避免误伤正文。
_THINK_BLOCK_RE = re.compile(
    r"<think\b[^>]*>.*?</think\s*>",
    re.IGNORECASE | re.DOTALL,
)

# 开头套话前缀表（命中其一才剥；按长度降序排列，保证长前缀优先命中）。
# 只收「模型自述行为」的措辞，不收可能出现在用户口述里的普通短语——
# 例如「以下是讨论纪要」就可能是用户真的说了这句话，绝不能进表。
_RAW_BOILERPLATE_PREFIXES = (
    "以下是整理后的内容",
    "以下是整理后的文字",
    "以下是整理后的文本",
    "以下是优化后的内容",
    "以下是优化后的文字",
    "以下是优化后的文本",
    "以下是整理好的内容",
    "以下是润色后的内容",
    "以下是结构化整理的内容",
    "结构化整理如下",
    "我整理如下",
    "整理如下",
    "优化如下",
    "润色如下",
    "根据你提供的内容",
    "根据您提供的内容",
    "根据你给的内容",
    "根据您给的内容",
    "根据上述内容",
    "根据以上内容",
)
BOILERPLATE_PREFIXES: tuple[str, ...] = tuple(
    sorted(_RAW_BOILERPLATE_PREFIXES, key=len, reverse=True)
)

# 套话的结束边界：剥到这些字符（含）为止
_BOUNDARY_CHARS = "。：:；;\n"
# 循环剥套话的轮数上限：正常输出最多一两轮，4 轮足够且能防病态输入
_MAX_BOILERPLATE_ROUNDS = 4


def strip_think_blocks(text: str) -> str:
    """剥掉成对的 <think>…</think> 思考块。剥完为空是合法结果（正文只有思考）。"""
    return _THINK_BLOCK_RE.sub("", text)


def strip_markdown_fences(text: str) -> str:
    """整个输出恰好被一对 ``` 围栏包住时，剥掉首尾围栏行。

    围栏数量不是恰好 2 个（正文中间还有围栏/可能有多段代码块）时不动，
    避免把用户口述的 markdown 洗坏。
    """
    stripped = text.strip()
    if not (stripped.startswith("```") and stripped.endswith("```")):
        return text
    if stripped.count("```") != 2:
        return text

    first_newline = stripped.find("\n")
    if first_newline == -1:
        return text  # 整段就是一行围栏，没有正文

    inner = stripped[first_newline + 1:]
    close_index = inner.rfind("```")
    if close_index == -1:
        return text
    body = inner[:close_index].strip("\n")
    if not body:
        return text  # 围栏里没有正文，保留原样
    return body


def strip_leading_boilerplate(text: str) -> str:
    """循环剥开头的自述套话，剥到第一个边界符（句号/冒号/换行，含）为止。

    两条安全线：前缀后找不到边界符不剥；剥完为空回退（整段都是套话时
    保留原文，不能把输出洗成空串）。
    """
    current = text.lstrip()
    for _ in range(_MAX_BOILERPLATE_ROUNDS):
        hit = next(
            (p for p in BOILERPLATE_PREFIXES if current.startswith(p)),
            None,
        )
        if hit is None:
            break

        # 从套话前缀之后找第一个边界符；找不到就不剥（宁可漏剥不可误伤）
        cut = -1
        for offset in range(len(hit), len(current)):
            if current[offset] in _BOUNDARY_CHARS:
                cut = offset + 1
                break
        if cut == -1:
            break

        candidate = current[cut:].lstrip()
        if not candidate:
            break  # 剥完就没了：整段都是套话，保留原文
        current = candidate
    return current


def clean_output(text: str) -> str:
    """对模型输出做三层清洗：思考块 → 围栏 → 开头套话。

    返回值约定：仅当输入本来就只有思考块/空白时才可能返回空串（由调用方按
    「无有效正文」处理）；其余情况至少保留原文。
    """
    if not text or not text.strip():
        return text.strip()

    # 思考块层允许清空（正文确实没有内容）；后两层各自保证不会清空
    out = strip_think_blocks(text)
    if not out.strip():
        return ""
    out = strip_markdown_fences(out)
    out = strip_leading_boilerplate(out)
    return out.strip()
