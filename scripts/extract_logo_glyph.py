#!/usr/bin/env python
# -*- coding: utf-8 -*-
"""生成应用内品牌标（金色圆角方 + 繁体「錄」）的 SVG。

标题栏 / 悬浮窗用矢量而不是位图：任意 DPI 都锐利，且与 electron/icons
里的 OS 图标同源（同一套金色令牌、同一个字形），不会出现"系统图标和应用内
标长得不一样"的分裂。

字形来源 Noto Serif SC（SIL OFL 1.1，可随包分发）。变量字体取 wght=700，
笔画在小尺寸下更挺。

用法：python scripts/extract_logo_glyph.py
产出：src/assets/logo-mark.svg
"""

import os
import sys

from fontTools.misc.transform import Transform
from fontTools.pens.boundsPen import BoundsPen
from fontTools.pens.svgPathPen import SVGPathPen
from fontTools.pens.transformPen import TransformPen
from fontTools.ttLib import TTFont
from fontTools.varLib.instancer import instantiateVariableFont

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
OUT = os.path.join(ROOT, "src", "assets", "logo-mark.svg")

CHAR = 0x9304  # 錄
FONT = r"C:\Windows\Fonts\NotoSerifSC-VF.ttf"
WEIGHT = 700
BOX = 1000        # viewBox 边长
PAD = 132         # 字在圆角方内的四周留白
RADIUS = 225      # 圆角半径（与 generate_app_icon.py 的 0.225 比例一致）

# 与 generate_app_icon.py 同一套金色令牌，改色两处要一起改
GOLD_HI = "#f2d26c"
GOLD_MID = "#c9a227"
GOLD_DEEP = "#a6831c"
GOLD_LINE = "#faecba"
INK = "#1a1400"


def build_glyph_path():
    """按墨迹包围盒缩放居中，返回 (path_d, 墨迹宽, 墨迹高)。"""
    font = TTFont(FONT, fontNumber=0)
    font = instantiateVariableFont(font, {"wght": WEIGHT}, inplace=False)
    glyph_set = font.getGlyphSet()
    gname = font.getBestCmap()[CHAR]

    bounds_pen = BoundsPen(glyph_set)
    glyph_set[gname].draw(bounds_pen)
    x_min, y_min, x_max, y_max = bounds_pen.bounds

    inner = BOX - 2 * PAD
    scale = min(inner / (x_max - x_min), inner / (y_max - y_min))

    # 字体坐标 y 向上、SVG y 向下：先翻转再缩放，最后按包围盒居中
    transform = (
        Transform()
        .translate(PAD, PAD)
        .scale(scale, -scale)
        .translate(-x_min, -y_max)
    )

    # 坐标保留 1 位小数：轮廓在 32px 下已远超肉眼分辨，同时把体积压到 ~3KB
    pen = SVGPathPen(glyph_set, ntos=lambda v: format(round(v, 1), "g"))
    glyph_set[gname].draw(TransformPen(pen, transform))
    return pen.getCommands()


def main():
    if not os.path.exists(FONT):
        print("找不到字体: %s" % FONT, file=sys.stderr)
        return 1

    d = build_glyph_path()

    svg = """<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {box} {box}" role="img" aria-label="ATT">
  <!-- ATT 品牌标：金色圆角方 + 繁体「錄」 -->
  <!-- 字形：Noto Serif SC wght={weight}（SIL OFL 1.1） -->
  <!-- 本文件由 scripts/extract_logo_glyph.py 生成，请勿手改 -->
  <defs>
    <linearGradient id="attGold" x1="0" y1="0" x2="1" y2="1">
      <stop offset="0" stop-color="{hi}"/>
      <stop offset="0.5" stop-color="{mid}"/>
      <stop offset="1" stop-color="{deep}"/>
    </linearGradient>
  </defs>
  <rect x="0" y="0" width="{box}" height="{box}" rx="{radius}" fill="url(#attGold)"/>
  <rect x="{inset}" y="{inset}" width="{inner_side}" height="{inner_side}" rx="{inner_rx}"
        fill="none" stroke="{line}" stroke-opacity="0.9" stroke-width="{line_w}"/>
  <path d="{d}" fill="{ink}"/>
</svg>
""".format(
        box=BOX,
        weight=WEIGHT,
        hi=GOLD_HI,
        mid=GOLD_MID,
        deep=GOLD_DEEP,
        radius=RADIUS,
        inset=85,
        inner_side=BOX - 170,
        inner_rx=RADIUS - 85,
        line=GOLD_LINE,
        line_w=7.5,
        d=d,
        ink=INK,
    )

    os.makedirs(os.path.dirname(OUT), exist_ok=True)
    with open(OUT, "w", encoding="utf-8", newline="\n") as fh:
        fh.write(svg)

    print("写入 %s (%d bytes)" % (os.path.relpath(OUT, ROOT), os.path.getsize(OUT)))
    return 0


if __name__ == "__main__":
    sys.exit(main())
