"""output_cleaning 契约测试：三层清洗的边界与安全线。

锁死的点（docs/OpenLess-调研与借鉴清单.md §4.1）：
1. 思考块只剥成对的 <think>…</think>；<thinking> 与未闭合标签不动；
2. 围栏只在「整个输出恰好一对 ```」时剥，正文中间的围栏（用户口述的代码）不动；
3. 开头套话剥到第一个句号/冒号/换行；找不到边界、剥完为空都不剥；
4. 正常输出一个字符都不能动。
"""
from __future__ import annotations

from app.llm.output_cleaning import (
    clean_output,
    strip_leading_boilerplate,
    strip_markdown_fences,
    strip_think_blocks,
)


# ==================== 第一层：思考块 ====================

def test_strip_think_blocks_removes_paired_block():
    assert strip_think_blocks("<think>推理过程</think>正文") == "正文"


def test_strip_think_blocks_case_insensitive_and_with_attributes():
    assert strip_think_blocks('<THINK type="x">a</THINK>正文') == "正文"
    assert strip_think_blocks("<Think >a</Think >正文") == "正文"


def test_strip_think_blocks_ignores_thinking_tag_and_unclosed():
    # <thinking> 是另一个标签名，不能当成思考块剥掉
    assert strip_think_blocks("<thinking>正文</thinking>") == "<thinking>正文</thinking>"
    # 未闭合的 <think> 可能是正文里的普通文本，不动
    assert strip_think_blocks("<think>只有开头没有闭合") == "<think>只有开头没有闭合"


def test_strip_think_blocks_removes_multiple_blocks():
    assert strip_think_blocks("<think>a</think>第一段<think>b</think>第二段") == "第一段第二段"


def test_strip_think_blocks_think_only_yields_empty():
    """整段只有思考块 → 空串是合法结果（正文确实没有内容）。"""
    assert strip_think_blocks("<think>全程推理</think>") == ""


def test_strip_think_blocks_multiline_body():
    assert strip_think_blocks("<think>第一行\n第二行</think>\n正文") == "\n正文"


# ==================== 第二层：围栏 ====================

def test_strip_markdown_fences_removes_single_paired_fence():
    assert strip_markdown_fences("```text\n整理后的正文\n```") == "整理后的正文"
    assert strip_markdown_fences("```\n整理后的正文\n```") == "整理后的正文"
    assert strip_markdown_fences("  ```python\nprint(1)\n```  ") == "print(1)"


def test_strip_markdown_fences_leaves_inner_fences_untouched():
    """正文中间还有围栏（可能是用户口述的代码）时，外层判断不成立，整段不动。"""
    text = "```markdown\n前文\n```code\nx = 1\n```\n后文\n```"
    assert strip_markdown_fences(text) == text


def test_strip_markdown_fences_leaves_unpaired_and_lone_fence_untouched():
    assert strip_markdown_fences("```python\nprint(1)") == "```python\nprint(1)"
    assert strip_markdown_fences("```") == "```"
    assert strip_markdown_fences("没有围栏的正文") == "没有围栏的正文"


def test_strip_markdown_fences_fence_with_content_on_close_line():
    assert strip_markdown_fences("```text\n正文```") == "正文"


# ==================== 第三层：开头套话 ====================

def test_strip_leading_boilerplate_cuts_at_colon_and_newline():
    assert strip_leading_boilerplate("以下是整理后的内容：\n正文第一句。") == "正文第一句。"
    assert strip_leading_boilerplate("我整理如下:\n正文") == "正文"
    assert strip_leading_boilerplate("根据您提供的内容，我整理如下。正文") == "正文"


def test_strip_leading_boilerplate_cuts_at_first_period():
    assert strip_leading_boilerplate("以下是优化后的内容。正文继续。") == "正文继续。"


def test_strip_leading_boilerplate_strips_nested_rounds():
    text = "以下是整理后的内容：\n我整理如下：\n正文"
    assert strip_leading_boilerplate(text) == "正文"


def test_strip_leading_boilerplate_keeps_text_when_no_boundary_found():
    """套话之后没有边界符：宁可漏剥，不可把正文一起吃掉。"""
    text = "以下是整理后的内容后面直接跟正文没有边界"
    assert strip_leading_boilerplate(text) == text


def test_strip_leading_boilerplate_keeps_text_when_all_boilerplate():
    """整段都是套话 → 保留原文，不能把输出洗成空串。"""
    text = "以下是整理后的内容："
    assert strip_leading_boilerplate(text) == text


def test_strip_leading_boilerplate_ignores_normal_openings():
    """用户可能真的口述这些短语，不在表里的开头一律不动。"""
    assert strip_leading_boilerplate("以下是会议的三个要点：") == "以下是会议的三个要点："
    assert strip_leading_boilerplate("正文如下") == "正文如下"


# ==================== 组合：clean_output ====================

def test_clean_output_applies_all_layers_in_order():
    raw = "<think>先想一想</think>\n```text\n以下是整理后的内容：\n正文在这里。\n```"
    assert clean_output(raw) == "正文在这里。"


def test_clean_output_leaves_normal_text_untouched():
    text = "今天修了缓存 bug，还把超时从 60 秒调到了 120 秒。"
    assert clean_output(text) == text


def test_clean_output_think_only_returns_empty():
    assert clean_output("<think>只有思考</think>") == ""
    assert clean_output("   <think>\n只有思考\n</think>   ") == ""


def test_clean_output_whitespace_only_returns_empty():
    assert clean_output("   \n\t ") == ""


def test_clean_output_strips_boilerplate_after_fence_removal():
    raw = "```\n优化如下：\n整理后的正文\n```"
    assert clean_output(raw) == "整理后的正文"
