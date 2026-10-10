#!/usr/bin/env python
# -*- coding: utf-8 -*-
"""生成 ATT 的应用图标与托盘状态图标（单一来源，改色 / 改字只改本文件）。

设计基线：圆角方形盘 + 内嵌一圈浅色细描边 + 正中心单个汉字。

一、应用图标（金盘 + 繁体「錄」）
  electron/icons/icon.png    512×512   Electron BrowserWindow 图标
  electron/icons/icon.ico    多尺寸    Windows 打包（electron-builder.yml: win.icon）
  electron/icons/icon.icns   多尺寸    macOS 打包（electron-builder.yml: mac.icon）
  应用内（标题栏）的品牌标是矢量，由 scripts/extract_logo_glyph.py 另出
  src/assets/logo-mark.svg —— 两者共用同一套金色令牌与同一个字形。

二、托盘状态图标（撞色方案 A）
  盘面与字形是一组互补 / 明度对撞色，且四种状态各配一个说明性汉字 ——
  小尺寸下即使字糊了也能靠盘色分辨，字不糊时直接读字就懂，不依赖记颜色。

    idle          錄 就绪     金盘   #f2d26c→#a6831c  + 墨字   #1a1400
    recording     聽 监听中   朱红盘 #e2564a→#92241e  + 米白字 #fff6e5
    transcribing  轉 转写中   靛蓝盘 #4a5fe0→#232f8a  + 亮金字 #f2d26c
    disconnected  斷 断开     石墨盘 #4a4f59→#33373e  + 淡灰字 #b9bdc6

  产出（每个状态一套多倍图，Electron 按系统 DPI 自动挑）：
    electron/tray/icons/win/{state}.png   16px 基准 + @1.25x~@3x（20/24/28/32/40/48）
    electron/tray/icons/macos/{state}.png 22px 基准 + @2x(44)

用法：
  uv run python scripts/generate_app_icon.py           # 出全部图标
  uv run python scripts/generate_app_icon.py --sheet   # 额外出评审图 docs/ui-audit/icon-states.png
"""

import os
import sys

import numpy as np
from PIL import Image, ImageDraw, ImageFont

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
ICON_DIR = os.path.join(ROOT, "electron", "icons")
TRAY_ICON_DIR = os.path.join(ROOT, "electron", "tray", "icons")
SHEET_PATH = os.path.join(ROOT, "docs", "ui-audit", "icon-states.png")

# ---------- 令牌取值（与 src/styles/tokens.css 同源，改色先改这里） ----------
# 渐变刻意收窄：金色一旦压太暗就会发橄榄绿，不再读作「金黄色」。
GOLD_HI = (242, 210, 108)     # #f2d26c 高光金
GOLD_MID = (201, 162, 39)     # #c9a227 --accent 唯一金
GOLD_DEEP = (166, 131, 28)    # #a6831c 深金（右下收暗，仍保持金相）
GOLD_LINE = (250, 236, 186)   # #faecba 内描边（参考图的浅色双线）
INK = (26, 20, 0)             # #1a1400 --accent-ink 金底专用深字

# 撞色盘面：红 / 蓝是金的第一撞色，灰用于「不可用」
GOLD_PLATE = (GOLD_HI, GOLD_MID, GOLD_DEEP)
RED_PLATE = ((226, 86, 74), (184, 50, 42), (146, 36, 30))     # #e2564a → #92241e
BLUE_PLATE = ((74, 95, 224), (58, 79, 208), (35, 47, 138))    # #4a5fe0 → #232f8a
GRAY_PLATE = ((74, 79, 89), (63, 67, 75), (51, 55, 62))       # #4a4f59 → #33373e

CREAM = (255, 246, 229)       # #fff6e5 红盘上的米白字
PALE_GRAY = (185, 189, 198)   # #b9bdc6 灰盘上的淡灰字

# 字形一律用繁体：同一字源体系，笔画也更挺
CHAR_IDLE = "\u9304"          # 錄
CHAR_RECORDING = "\u807d"     # 聽
CHAR_TRANSCRIBING = "\u8f49"  # 轉
CHAR_DISCONNECTED = "\u65b7"  # 斷

# 简体对照（仅评审图用：16px 下笔画少一半，明显更好认）
SIMPLIFIED_CHARS = {
    "idle": "\u5f55",           # 录
    "recording": "\u542c",      # 听
    "transcribing": "\u8f6c",   # 转
    "disconnected": "\u65ad",   # 断
}

# 托盘字形取繁体还是简体：看过 docs/ui-audit/icon-states.png 的可读性对照后再定。
# 16px 下简体（录/听/转/断）明显更好认；改成 True 重跑脚本即可，不用动别的。
USE_SIMPLIFIED_TRAY_CHARS = False


def tray_char(state, char):
    """按开关决定某个状态实际使用的汉字。"""
    return SIMPLIFIED_CHARS[state] if USE_SIMPLIFIED_TRAY_CHARS else char


# 托盘状态表：(state, 字, 中文说明, 盘面渐变, 字色, 内描边色)
# state 字符串必须与渲染层实际发送的一致（见 src/composables/useRecorder.js）。
TRAY_STATES = (
    ("idle", CHAR_IDLE, "就绪", GOLD_PLATE, INK, GOLD_LINE),
    ("recording", CHAR_RECORDING, "监听中", RED_PLATE, CREAM, CREAM),
    ("transcribing", CHAR_TRANSCRIBING, "转写中", BLUE_PLATE, GOLD_HI, GOLD_LINE),
    ("disconnected", CHAR_DISCONNECTED, "断开", GRAY_PLATE, PALE_GRAY, PALE_GRAY),
)

# 多倍图分档：Windows 托盘按 16px 逻辑尺寸取图，系统缩放 100%~300% 都会放大它，
# 所以把常见缩放档全出一遍（4K 屏常见 150%/200%），让系统永远拿到原生像素。
# macOS 菜单栏 22pt（@2x = 44）。
WIN_TRAY_SIZES = (
    (16, ""),
    (20, "@1.25x"),
    (24, "@1.5x"),
    (28, "@1.75x"),
    (32, "@2x"),
    (40, "@2.5x"),
    (48, "@3x")
)
MAC_TRAY_SIZES = ((22, ""), (44, "@2x"))

# 评审图配色（Win11 深色 / 浅色任务栏近似色）
SHEET_DARK = (32, 32, 32)
SHEET_LIGHT = (243, 243, 243)
SHEET_PANEL = (24, 24, 26)
SHEET_TEXT = (228, 228, 231)
SHEET_SUBTLE = (150, 150, 158)

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
WEIGHT = 700    # 变量字体统一取 wght=700：小尺寸下笔画更清晰


def load_font(px, char=CHAR_IDLE):
    """按候选顺序找第一个能渲染出目标汉字的字体，并取粗体字重。"""
    for path in FONT_CANDIDATES:
        if not os.path.exists(path):
            continue
        try:
            font = ImageFont.truetype(path, px, index=0)
            try:
                font.set_variation_by_axes([WEIGHT])
            except Exception:
                pass
            if font.getmask(char).getbbox() is None:
                continue  # 字体缺该字，换下一个
            return font, path
        except Exception:
            continue
    raise RuntimeError("找不到可渲染「%s」的字体，请安装 Noto Serif SC" % char)


def diagonal_gradient(size, stops):
    """左上亮 → 右下暗的三段对角渐变，给平面色块一点体积。"""
    yy, xx = np.mgrid[0:size, 0:size].astype(np.float32)
    t = (xx + yy) / (2.0 * (size - 1))
    arr = np.array(stops, dtype=np.float32)
    out = np.zeros((size, size, 3), dtype=np.float32)
    half = 0.5
    lo = t <= half
    out[lo] = arr[0] + (arr[1] - arr[0]) * (t[lo] / half)[:, None]
    hi = ~lo
    out[hi] = arr[1] + (arr[2] - arr[1]) * ((t[hi] - half) / half)[:, None]
    return Image.fromarray(out.astype(np.uint8))


def render_glyph(px, font, char, color):
    """把汉字渲染成指定色的 RGBA，四周留白便于后续按墨迹包围盒裁切。"""
    pad = int(px * 0.4)
    canvas = Image.new("RGBA", (px + 2 * pad, px + 2 * pad), (0, 0, 0, 0))
    ImageDraw.Draw(canvas).text(
        (pad, pad), char, font=font, fill=color + (255,), anchor="lt"
    )
    return canvas


def build_master(size, font, char, plate, glyph_color, line_color):
    """渲染一张主稿：圆角方盘 + 内描边 + 顶部高光 + 居中汉字。"""
    margin = int(round(size * 0.045))          # 透明外边距：让圆角方形独立成形
    box = (margin, margin, size - margin, size - margin)
    span = size - 2 * margin
    radius = int(round(span * 0.225))          # 接近 iOS squircle 的圆角比例

    # 1. 圆角方形遮罩 + 盘面渐变填充
    mask = Image.new("L", (size, size), 0)
    ImageDraw.Draw(mask).rounded_rectangle(box, radius=radius, fill=255)
    img = Image.new("RGBA", (size, size), (0, 0, 0, 0))
    img.paste(diagonal_gradient(size, plate).convert("RGBA"), (0, 0), mask)

    draw = ImageDraw.Draw(img)

    # 2. 内嵌细描边（参考图那圈浅色双线），跟着字色走
    inset = int(round(span * 0.085))
    line_w = max(1, int(round(size * 0.0075)))
    draw.rounded_rectangle(
        (box[0] + inset, box[1] + inset, box[2] - inset, box[3] - inset),
        radius=max(1, radius - inset),
        outline=line_color + (232,),
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

    # 4. 正中心汉字：按墨迹包围盒居中，而不是按字体行框
    glyph = render_glyph(int(round(span * 0.50)), font, char, glyph_color)
    glyph = glyph.crop(glyph.getbbox())
    img.alpha_composite(
        glyph,
        ((size - glyph.width) // 2, (size - glyph.height) // 2),
    )

    return img


def export_app_icons(hi):
    """应用图标：金盘 + 「錄」，与改造前逐字节一致。"""
    os.makedirs(ICON_DIR, exist_ok=True)

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
        path = os.path.join(ICON_DIR, name)
        print("  %-12s %8d bytes" % (name, os.path.getsize(path)))


def export_tray_icons(masters):
    """托盘状态图标：每个状态一套多倍图，供 Electron 按 DPI 自动挑选。"""
    total = 0
    for platform_dir, sizes in (("win", WIN_TRAY_SIZES), ("macos", MAC_TRAY_SIZES)):
        out_dir = os.path.join(TRAY_ICON_DIR, platform_dir)
        os.makedirs(out_dir, exist_ok=True)

        for state, char, _label, _plate, _glyph, _line in TRAY_STATES:
            for px, suffix in sizes:
                target = os.path.join(out_dir, "%s%s.png" % (state, suffix))
                masters[state].resize((px, px), Image.LANCZOS).save(target, format="PNG")
                total += 1
            print("  %-6s %-13s %s" % (
                platform_dir,
                "%s(%s)" % (state, char),
                " ".join("%d%s" % (px, suffix or "@1x") for px, suffix in sizes),
            ))

    print("托盘图标共 %d 个文件 -> %s" % (total, TRAY_ICON_DIR))


def _tile(master, px, zoom, bg, box_w, box_h):
    """把某个尺寸的图标贴到指定底色的格子中央；zoom>1 时用最近邻看真实像素块。"""
    if zoom > 1:
        base = master.resize((16, 16), Image.LANCZOS)
        tile = base.resize((base.width * zoom, base.height * zoom), Image.NEAREST)
    else:
        tile = master.resize((px, px), Image.LANCZOS)
    box = Image.new("RGB", (box_w, box_h), bg)
    box.paste(tile, ((box_w - tile.width) // 2, (box_h - tile.height) // 2), tile)
    return box


def render_review_sheet(masters):
    """评审图：① 四个状态在真实尺寸下的样子；② 小尺寸「繁体 vs 简体」可读性对照。

    用的是与落盘文件同一份 master、同一套缩放，所以这张图就是最终效果。
    """
    font, _ = load_font(18)
    f_title = font
    f_row, _ = load_font(17)
    f_small, _ = load_font(13)

    pad, left, head_h = 20, 300, 44

    # ① 四状态：详情 64 / win 32 / win 24 / macOS 22 / win 20 / win 16 / 16×4 深 / 16×4 浅
    cols_a = [
        (64, "详情 64", 0, SHEET_DARK, 92),
        (32, "@2x 32", 0, SHEET_DARK, 66),
        (24, "@1.5x 24", 0, SHEET_DARK, 66),
        (22, "macOS 22", 0, SHEET_DARK, 66),
        (20, "@1.25x 20", 0, SHEET_DARK, 70),
        (16, "@1x 16", 0, SHEET_DARK, 60),
        (16, "16×4 深栏", 4, SHEET_DARK, 96),
        (16, "16×4 浅栏", 4, SHEET_LIGHT, 96),
    ]

    # ② 小尺寸可读性：繁体 / 简体 各一行
    cols_b = [
        (16, "@1x 16", 0, SHEET_DARK, 60),
        (20, "@1.25x 20", 0, SHEET_DARK, 70),
        (24, "@1.5x 24", 0, SHEET_DARK, 66),
        (16, "16×6 深栏", 6, SHEET_DARK, 112),
        (16, "16×6 浅栏", 6, SHEET_LIGHT, 112),
    ]

    row_a, row_b = 78, 24 * 6 + 24
    width = max(left + sum(c[4] for c in cols_a), left + sum(c[4] for c in cols_b)) + pad * 2
    height = (pad + head_h * 3 + len(TRAY_STATES) * row_a
              + head_h * 2 + len(TRAY_STATES) * 2 * row_b + pad)
    sheet = Image.new("RGB", (width, height), SHEET_PANEL)
    draw = ImageDraw.Draw(sheet)

    def column_headers(cols, y):
        x = left
        for _px, title, _zoom, _bg, w in cols:
            draw.text((x + w // 2, y), title, font=f_small, fill=SHEET_SUBTLE, anchor="ma")
            x += w

    def draw_row(cols, y, row_h, name, sub, master):
        draw.text((pad, y + row_h // 2), name, font=f_row, fill=SHEET_TEXT, anchor="lm")
        draw.text((pad + 186, y + row_h // 2), sub, font=f_small, fill=SHEET_SUBTLE, anchor="lm")
        x = left
        for px, _title, zoom, bg, w in cols:
            sheet.paste(_tile(master, px, zoom, bg, w, row_h), (x, y))
            x += w

    y = pad
    draw.text((pad, y), "ATT 托盘图标 · 撞色方案 A（盤面与字色对撞）· 状态字自解释",
              font=f_title, fill=SHEET_TEXT, anchor="la")
    y += head_h

    draw.text((pad, y), "① 四种状态 · 真实尺寸", font=f_small, fill=SHEET_TEXT, anchor="la")
    y += head_h
    column_headers(cols_a, y)
    y += head_h - 8
    for state, char, label, _plate, _glyph, _line in TRAY_STATES:
        draw_row(cols_a, y, row_a, "%s「%s」" % (state, char), label, masters[state])
        y += row_a

    # 简体主稿只用于评审对照，2048 足够覆盖 ≤24px 与放大列
    draw.text((pad, y), "② 小尺寸可读性 · 繁体 vs 简体（简体笔画少一半，16px 更能认）",
              font=f_small, fill=SHEET_TEXT, anchor="la")
    y += head_h
    column_headers(cols_b, y)
    y += head_h - 8

    small_hi = MASTER * 2
    for state, char, label, plate, glyph, line in TRAY_STATES:
        simp_char = SIMPLIFIED_CHARS[state]
        simp_font, _ = load_font(int(round(small_hi * 0.45)), simp_char)
        simp_master = build_master(small_hi, simp_font, simp_char, plate, glyph, line)
        draw_row(cols_b, y, row_b, "%s「%s」繁体" % (state, char), label, masters[state])
        y += row_b
        draw_row(cols_b, y, row_b, "%s「%s」简体" % (state, simp_char), label, simp_master)
        y += row_b

    os.makedirs(os.path.dirname(SHEET_PATH), exist_ok=True)
    sheet.save(SHEET_PATH, format="PNG")
    print("评审图 -> %s %s" % (SHEET_PATH, sheet.size))


def main(argv):
    # Windows 控制台默认 GBK，先切 UTF-8，避免中文输出乱码
    if hasattr(sys.stdout, "reconfigure"):
        sys.stdout.reconfigure(encoding="utf-8")

    want_sheet = "--sheet" in argv

    # 主稿按 4 倍超采样渲染，再缩到目标尺寸，边缘更干净
    hi_size = MASTER * 4
    print("主稿尺寸:", hi_size)

    # 1. 应用图标：金盘 + 「錄」
    font, font_path = load_font(int(round(hi_size * 0.45)), CHAR_IDLE)
    print("字形来源:", font_path)
    export_app_icons(build_master(hi_size, font, CHAR_IDLE, GOLD_PLATE, INK, GOLD_LINE))

    # 2. 托盘状态图标：每个状态一种撞色 + 一个说明性汉字
    masters = {}
    for state, char, _label, plate, glyph, line in TRAY_STATES:
        char = tray_char(state, char)
        state_font = font
        if char != CHAR_IDLE:
            state_font, _ = load_font(int(round(hi_size * 0.45)), char)
        masters[state] = build_master(hi_size, state_font, char, plate, glyph, line)
    export_tray_icons(masters)

    if want_sheet:
        render_review_sheet(masters)

    print("应用内品牌标请用 scripts/extract_logo_glyph.py 生成 src/assets/logo-mark.svg")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
