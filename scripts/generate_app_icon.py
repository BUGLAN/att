#!/usr/bin/env python
# -*- coding: utf-8 -*-
"""生成 ATT 应用图标（金色圆角方 + 繁体「錄」）。

设计参考用户提供的入口图：圆角方形 + 内嵌一圈细描边 + 正中心单个汉字。
主题色改用应用唯一强调金（--accent 家族 #c9a227），字取 --accent-ink 深色，
这样 16px 小尺寸下依然有足够对比度，不会糊成一团金。

字形用 Noto Serif SC（SIL OFL 1.1，可随包分发），宋体骨架比黑体更接近
参考图中那个带笔锋的「記」。变量字体按 wght=700 取粗，笔画在小尺寸下更挺。

产出：
  electron/icons/icon.png    512×512   Electron BrowserWindow 图标
  electron/icons/icon.ico    多尺寸    Windows 打包（electron-builder.yml: win.icon）
  electron/icons/icon.icns   多尺寸    macOS 打包（electron-builder.yml: mac.icon）

应用内（标题栏）的品牌标是矢量，由 scripts/extract_logo_glyph.py 另出
src/assets/logo-mark.svg —— 两者共用同一套金色令牌与同一个字形。

用法：python scripts/generate_app_icon.py
"""

import os
import sys

import numpy as np
from PIL import Image, ImageDraw, ImageFont

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
ICON_DIR = os.path.join(ROOT, "electron", "icons")

CHAR = "\u9304"  # 錄

# ---------- 令牌取值（与 src/styles/tokens.css 同源，改色先改这里） ----------
# 渐变刻意收窄：金色一旦压太暗就会发橄榄绿，不再读作「金黄色」。
GOLD_HI = (242, 210, 108)     # #f2d26c 高光金
GOLD_MID = (201, 162, 39)     # #c9a227 --accent 唯一金
GOLD_DEEP = (166, 131, 28)    # #a6831c 深金（右下收暗，仍保持金相）
GOLD_LINE = (250, 236, 186)   # #faecba 内描边（参考图的浅色双线）
INK = (26, 20, 0)             # #1a1400 --accent-ink 金底专用深字

# 优先 OFL 可分发字体；系统自带的微软雅黑/宋体仅作本机兜底，不随包分发。
FONT_CANDIDATES = [
    r"C:\Windows\Fonts\NotoSerifSC-VF.ttf",
    r"C:\Windows\Fonts\NotoSansSC-VF.ttf",
    r"C:\Windows\Fonts\simsun.ttc",
    r"C:\Windows\Fonts\msyh.ttc",
    "/usr/share/fonts/opentype/noto/NotoSerifCJK-Regular.ttc",
    "/System/Library/Fonts/PingFang.ttc",
]

MASTER = 1024   # 主稿基准尺寸（实际按 MASTER*4 超采样后缩小）


def load_font(px):
    """按候选顺序找第一个能渲染出「錄」的字体，并取粗体字重。"""
    for path in FONT_CANDIDATES:
        if not os.path.exists(path):
            continue
        try:
            font = ImageFont.truetype(path, px, index=0)
            # 变量字体统一取 wght=700：小尺寸下笔画更清晰
            try:
                font.set_variation_by_axes([700])
            except Exception:
                pass
            if font.getmask(CHAR).getbbox() is None:
                continue  # 字体缺该字，换下一个
            return font, path
        except Exception:
            continue
    raise RuntimeError("找不到可渲染「%s」的字体，请安装 Noto Serif SC" % CHAR)


def diagonal_gradient(size):
    """左上亮金 → 右下深金的三段对角渐变，给平面色块一点体积。"""
    yy, xx = np.mgrid[0:size, 0:size].astype(np.float32)
    t = (xx + yy) / (2.0 * (size - 1))
    stops = np.array([GOLD_HI, GOLD_MID, GOLD_DEEP], dtype=np.float32)
    out = np.zeros((size, size, 3), dtype=np.float32)
    half = 0.5
    lo = t <= half
    out[lo] = stops[0] + (stops[1] - stops[0]) * (t[lo] / half)[:, None]
    hi = ~lo
    out[hi] = stops[1] + (stops[2] - stops[1]) * ((t[hi] - half) / half)[:, None]
    return Image.fromarray(out.astype(np.uint8))


def render_glyph(px, font):
    """把「錄」渲染成深色 RGBA，四周留白便于后续按墨迹包围盒裁切。"""
    pad = int(px * 0.4)
    canvas = Image.new("RGBA", (px + 2 * pad, px + 2 * pad), (0, 0, 0, 0))
    ImageDraw.Draw(canvas).text(
        (pad, pad), CHAR, font=font, fill=INK + (255,), anchor="lt"
    )
    return canvas


def build_master(size, font):
    margin = int(round(size * 0.045))          # 透明外边距：让圆角方形独立成形
    box = (margin, margin, size - margin, size - margin)
    span = size - 2 * margin
    radius = int(round(span * 0.225))          # 接近 iOS squircle 的圆角比例

    # 1. 圆角方形遮罩 + 金色渐变填充
    mask = Image.new("L", (size, size), 0)
    ImageDraw.Draw(mask).rounded_rectangle(box, radius=radius, fill=255)
    img = Image.new("RGBA", (size, size), (0, 0, 0, 0))
    img.paste(diagonal_gradient(size).convert("RGBA"), (0, 0), mask)

    draw = ImageDraw.Draw(img)

    # 2. 内嵌细描边（参考图那圈浅色双线）
    inset = int(round(span * 0.085))
    line_w = max(1, int(round(size * 0.0075)))
    draw.rounded_rectangle(
        (box[0] + inset, box[1] + inset, box[2] - inset, box[3] - inset),
        radius=max(1, radius - inset),
        outline=GOLD_LINE + (232,),
        width=line_w,
    )

    # 3. 顶部内高光：一条很淡的亮弧，暗示光源在上方
    hl = Image.new("RGBA", (size, size), (0, 0, 0, 0))
    ImageDraw.Draw(hl).rounded_rectangle(
        (box[0] + line_w, box[1] + line_w, box[2] - line_w, box[3] - line_w),
        radius=max(1, radius - line_w),
        outline=(255, 255, 255, 46),
        width=max(1, int(round(size * 0.004))),
    )
    img.alpha_composite(hl)

    # 4. 正中心「錄」：按墨迹包围盒居中，而不是按字体行框
    glyph = render_glyph(int(round(span * 0.50)), font)
    ink_box = glyph.getbbox()
    glyph = glyph.crop(ink_box)
    img.alpha_composite(
        glyph,
        ((size - glyph.width) // 2, (size - glyph.height) // 2),
    )

    return img


def main():
    os.makedirs(ICON_DIR, exist_ok=True)

    # 主稿按 4 倍超采样渲染，再缩到目标尺寸，边缘更干净
    hi_size = MASTER * 4
    font, font_path = load_font(int(round(hi_size * 0.45)))
    print("字形来源:", font_path)
    print("主稿尺寸:", hi_size)

    hi = build_master(hi_size, font)

    def export(px):
        return hi.resize((px, px), Image.LANCZOS)

    icon_png = export(512)
    icon_png.save(os.path.join(ICON_DIR, "icon.png"), format="PNG")

    icon_png.save(
        os.path.join(ICON_DIR, "icon.ico"),
        format="ICO",
        sizes=[(16, 16), (24, 24), (32, 32), (48, 48), (64, 64), (128, 128), (256, 256)],
    )

    export(1024).save(os.path.join(ICON_DIR, "icon.icns"), format="ICNS")

    for name in ("icon.png", "icon.ico", "icon.icns"):
        p = os.path.join(ICON_DIR, name)
        print("  %-12s %8d bytes" % (name, os.path.getsize(p)))
    print("应用内品牌标请用 scripts/extract_logo_glyph.py 生成 src/assets/logo-mark.svg")


if __name__ == "__main__":
    sys.exit(main())
