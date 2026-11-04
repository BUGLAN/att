#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""生成「落墨」品牌图标与托盘四态图标（墨底金字「墨」，版式说明见 docs/架构.md §8「品牌与应用图标」）。

应用图标：墨底金字「墨」。托盘：待命同品牌标「墨」，录音 / 转写 / 断开用与悬浮窗同色的状态章「聽 轉 斷」。

为什么不用 `tauri icon` 一键缩放：
旧图标的 16~48px 都是大图直接缩下来的，字只占方块约 55%、笔画落在半个像素上，
Windows 任务栏 / 资源管理器里就是一团发灰的糊；ico 里也缺 20 / 40px（125% / 150% 缩放），
系统只能再缩一次。这里每个尺寸都从字体矢量直接栅格化：

- 大图（≥64px）：墨底 + 内描边 + 渐变金字。角上不放任何色块：Dock / 任务栏会把右下角的红块读成未读角标；
- 小图（≤48px）：去掉内框、外缘一像素淡描边，字放大到方块的 74%~84%，纯亮金，≤24px 再略加粗；
  并在 1/8 像素内微调字的位置与大小，挑「覆盖率最接近 0 或 1」的那组，让主笔画落在整像素上
  （一种自动的轻量 hinting）；
- 抗锯齿用整数倍超采样 + 盒式下采样（预乘 alpha），边缘不发黑。

字形来源 Noto Serif TC（SIL OFL 1.1，可随包分发；繁体状态字只有 TC 才全），可变字体按 wght=600 实例化。
字体文件不入库，运行时指定（Google Fonts：github.com/google/fonts/tree/main/ofl/notoseriftc）：
  python3 scripts/generate_brand_icons.py --font /path/to/NotoSerifTC[wght].ttf
依赖：fontTools、Pillow、numpy；icns 需在 macOS 上用 iconutil 打包。

产出：
  src-tauri/icons/icon.ico   16/20/24/32/40/48/64/128/256（PNG 压缩条目）
  src-tauri/icons/icon.png   512，铺满（Linux / 默认窗口图标）
  src-tauri/icons/icon.icns  按 Apple 网格留白（主体 824/1024）+ 柔和投影
  src/assets/brand-mark.svg  应用内矢量标（标题栏用，小图版式）
  src-tauri/icons/tray/win-<态>-<px>.png    16/20/24/32/40/48，tray.rs 按主屏缩放挑「16×缩放」档
  src-tauri/icons/tray/macos-<态>.png       44（菜单栏 22pt @2x）
"""

import argparse
import io
import os
import shutil
import struct
import subprocess
import sys
import tempfile

import numpy as np
from fontTools.pens.basePen import BasePen
from fontTools.pens.boundsPen import BoundsPen
from fontTools.pens.svgPathPen import SVGPathPen
from fontTools.ttLib import TTFont
from fontTools.varLib.instancer import instantiateVariableFont
from PIL import Image, ImageDraw, ImageFilter

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
ICONS = os.path.join(ROOT, "src-tauri", "icons")
SVG_OUT = os.path.join(ROOT, "src", "assets", "brand-mark.svg")

CHAR = "墨"
WEIGHT = 600

# 色板：墨底取 tokens.css 灰阶最深两档的延伸，金取 --accent 家族
INK_TOP = (43, 43, 49)
INK_BOTTOM = (12, 12, 14)
GOLD_HI = (242, 210, 108)
GOLD_MID = (220, 182, 58)
GOLD_SMALL = (232, 195, 79)   # 小图纯色：比渐变中段更亮，深底上对比更足
GOLD_LINE = (201, 162, 39, 120)

ICO_SIZES = [16, 20, 24, 32, 40, 48, 64, 128, 256]
SMALL_MAX = 48

BRAND = {"top": INK_TOP, "bottom": INK_BOTTOM, "fg": GOLD_SMALL}
# 托盘四态：底色 / 字色取 tokens.css 的 --seal-*（与悬浮窗印章同值），顶部提亮一档做出微弱的光照
TRAY = {
    "idle": ("墨", BRAND),
    "recording": ("聽", {"top": (214, 86, 74), "bottom": (184, 62, 52), "fg": (251, 231, 198)}),
    "transcribing": ("轉", {"top": (84, 104, 228), "bottom": (58, 76, 196), "fg": (243, 210, 122)}),
    "disconnected": ("斷", {"top": (92, 96, 106), "bottom": (66, 69, 78), "fg": (214, 216, 222)}),
}
TRAY_WIN_SIZES = [16, 20, 24, 32, 40, 48]
TRAY_MAC_SIZE = 44


# ---------------- 字形 ----------------

class FlattenPen(BasePen):
    """把字形轮廓拍平成折线（每段曲线细分），供 PIL 填充。"""

    def __init__(self, glyph_set, steps=12):
        super().__init__(glyph_set)
        self.steps = steps
        self.contours = []
        self.cur = None

    def _moveTo(self, pt):
        self.cur = [pt]

    def _lineTo(self, pt):
        self.cur.append(pt)

    def _qCurveToOne(self, p1, p2):
        p0 = self.cur[-1]
        for i in range(1, self.steps + 1):
            t = i / self.steps
            a = (1 - t) ** 2
            b = 2 * (1 - t) * t
            c = t * t
            self.cur.append((a * p0[0] + b * p1[0] + c * p2[0], a * p0[1] + b * p1[1] + c * p2[1]))

    def _curveToOne(self, p1, p2, p3):
        p0 = self.cur[-1]
        for i in range(1, self.steps + 1):
            t = i / self.steps
            mt = 1 - t
            self.cur.append((
                mt ** 3 * p0[0] + 3 * mt * mt * t * p1[0] + 3 * mt * t * t * p2[0] + t ** 3 * p3[0],
                mt ** 3 * p0[1] + 3 * mt * mt * t * p1[1] + 3 * mt * t * t * p2[1] + t ** 3 * p3[1],
            ))

    def _closePath(self):
        if self.cur:
            self.contours.append(np.array(self.cur, dtype=np.float64))
        self.cur = None

    _endPath = _closePath


def open_font(font_path):
    font = TTFont(font_path)
    if "fvar" in font:
        font = instantiateVariableFont(font, {"wght": WEIGHT})
    return font


def load_glyph(font, char):
    glyph_set = font.getGlyphSet()
    name = font.getBestCmap()[ord(char)]
    pen = FlattenPen(glyph_set)
    glyph_set[name].draw(pen)
    bounds = BoundsPen(glyph_set)
    glyph_set[name].draw(bounds)
    svg = SVGPathPen(glyph_set)
    glyph_set[name].draw(svg)
    return pen.contours, bounds.bounds, svg


def glyph_mask(contours, bounds, size, box, adjust=(1.0, 0.0, 0.0), embolden=0):
    """按非零环绕规则把字形填进 size×size 的灰度蒙版。

    box = (cx, cy, extent)：字的墨迹包围盒按 extent 等比放进以 (cx, cy) 为中心的方框；
    adjust = (缩放系数, dx, dy) 用于小图微调。逐轮廓按方向 ±1 累加覆盖，≠0 即为墨。
    """
    x_min, y_min, x_max, y_max = bounds
    k, dx, dy = adjust
    cx, cy, extent = box
    scale = extent * k / max(x_max - x_min, y_max - y_min)
    gx = (x_min + x_max) / 2
    gy = (y_min + y_max) / 2
    acc = np.zeros((size, size), dtype=np.int16)
    for pts in contours:
        xs = cx + dx + (pts[:, 0] - gx) * scale
        ys = cy + dy - (pts[:, 1] - gy) * scale  # 字体 y 向上，画布 y 向下
        area = np.sum(xs * np.roll(ys, -1) - np.roll(xs, -1) * ys)
        layer = Image.new("L", (size, size), 0)
        ImageDraw.Draw(layer).polygon(list(zip(xs, ys)), fill=1)
        acc += np.asarray(layer, dtype=np.int16) * (1 if area > 0 else -1)
    mask = Image.fromarray(np.where(acc != 0, 255, 0).astype(np.uint8), "L")
    if embolden:
        mask = mask.filter(ImageFilter.MaxFilter(embolden))
    return mask


# ---------------- 版式 ----------------

def layout(px, small):
    """返回 (方块外边距比例, 圆角比例, 字框比例, 字竖直偏移比例)。"""
    if small:
        extent = 0.84 if px <= 24 else 0.76
        return 0.0, 0.2, extent, -0.01
    return 0.03, 0.225, 0.6, -0.015


def vertical_gradient(size, top, bottom):
    t = np.linspace(0, 1, size, dtype=np.float64)[:, None]
    rgb = np.array(top, dtype=np.float64) * (1 - t) + np.array(bottom, dtype=np.float64) * t
    arr = np.repeat(rgb[:, None, :], size, axis=1).reshape(size, size, 3)
    return Image.fromarray(arr.astype(np.uint8), "RGB")


def best_adjust(contours, bounds, px, ss, box, embolden):
    """在 1/8 像素与 ±3% 缩放内搜索，让下采样后的字形覆盖率尽量非 0 即 1。"""
    best = None
    for k in (0.97, 0.985, 1.0, 1.015, 1.03):
        for sx in range(-4, 5):
            for sy in range(-4, 5):
                adj = (k, sx / 8 * ss, sy / 8 * ss)
                mask = glyph_mask(contours, bounds, px * ss, box, adj, embolden)
                cov = np.asarray(mask.reduce(ss), dtype=np.float64) / 255
                score = float(np.mean(cov * (1 - cov)))
                if best is None or score < best[0]:
                    best = (score, adj)
    return best[1]


def render(contours, bounds, px, small=None, palette=BRAND):
    """渲染一个 px×px 的图标（RGBA）；palette 只作用于小图版式（托盘各态）。"""
    small = px <= SMALL_MAX if small is None else small
    ss = max(4, -(-1024 // px)) if not small else 16
    size = px * ss
    margin, radius, extent, lift = layout(px, small)
    m = margin * size
    tile = (m, m, size - m - 1, size - m - 1)
    side = size - 2 * m

    img = Image.new("RGBA", (size, size), (0, 0, 0, 0))
    tile_mask = Image.new("L", (size, size), 0)
    ImageDraw.Draw(tile_mask).rounded_rectangle(tile, radius=radius * side, fill=255)
    top, bottom = (palette["top"], palette["bottom"]) if small else (INK_TOP, INK_BOTTOM)
    img.paste(vertical_gradient(size, top, bottom), (0, 0), tile_mask)
    if small:
        # 一像素极淡描边：深色任务栏上墨底方块不至于整个隐没
        rim = Image.new("L", (size, size), 0)
        ImageDraw.Draw(rim).rounded_rectangle(tile, radius=radius * side, outline=30, width=ss)
        img.paste((255, 255, 255), (0, 0), rim)

    if not small:
        # 顶边一线极淡高光 + 内描边（与旧金图标同构的 8.5% 内框）
        hi = Image.new("L", (size, size), 0)
        d = ImageDraw.Draw(hi)
        d.rounded_rectangle(tile, radius=radius * side, outline=18, width=max(1, int(side * 0.006)))
        img.paste((255, 255, 255), (0, 0), hi)
        inset = m + side * 0.085
        ring = Image.new("L", (size, size), 0)
        ImageDraw.Draw(ring).rounded_rectangle(
            (inset, inset, size - inset - 1, size - inset - 1),
            radius=side * 0.14, outline=GOLD_LINE[3], width=max(1, int(side * 0.008)))
        img.paste(GOLD_LINE[:3], (0, 0), ring)

    box = (size / 2, size / 2 + lift * side, extent * side)
    embolden = 5 if small and px <= 24 else (3 if small else 0)
    adjust = best_adjust(contours, bounds, px, ss, box, embolden) if small else (1.0, 0.0, 0.0)
    mask = glyph_mask(contours, bounds, size, box, adjust, embolden)
    if small:
        img.paste(palette["fg"], (0, 0), mask)
    else:
        img.paste(vertical_gradient(size, GOLD_HI, GOLD_MID), (0, 0), mask)

    # 预乘 alpha 后再盒式下采样，透明边缘不发黑
    return img.convert("RGBa").reduce(ss).convert("RGBA")


def apple_canvas(contours, bounds, px):
    """macOS：主体占 824/1024 居中，带柔和投影（与 f867b9a 的网格一致）。"""
    body_px = round(px * 824 / 1024)
    body = render(contours, bounds, body_px, small=px <= 64)
    if px <= 64:
        # 小尺寸只留主体，不加投影（投影在 16/32 上只会让边缘发灰）
        canvas = Image.new("RGBA", (px, px), (0, 0, 0, 0))
        off = (px - body_px) // 2
        canvas.alpha_composite(body, (off, off))
        return canvas
    canvas = Image.new("RGBA", (px, px), (0, 0, 0, 0))
    off = (px - body_px) // 2
    shadow = Image.new("RGBA", (px, px), (0, 0, 0, 0))
    alpha = body.getchannel("A").point(lambda a: a * 0.32)
    shadow.paste((0, 0, 0), (off, off + round(px * 0.012)), alpha)
    canvas.alpha_composite(shadow.filter(ImageFilter.GaussianBlur(px * 0.014)))
    canvas.alpha_composite(body, (off, off))
    return canvas


# ---------------- 打包 ----------------

def write_ico(images, path):
    entries = []
    for im in images:
        buf = io.BytesIO()
        im.save(buf, "PNG", optimize=True)
        entries.append((im.size[0], buf.getvalue()))
    header = struct.pack("<HHH", 0, 1, len(entries))
    offset = 6 + 16 * len(entries)
    table = b""
    blobs = b""
    for size, data in entries:
        dim = 0 if size >= 256 else size
        table += struct.pack("<BBBBHHII", dim, dim, 0, 0, 1, 32, len(data), offset + len(blobs))
        blobs += data
    with open(path, "wb") as f:
        f.write(header + table + blobs)


def write_icns(contours, bounds, path):
    if not shutil.which("iconutil"):
        print("跳过 icns：iconutil 只在 macOS 上可用", file=sys.stderr)
        return
    with tempfile.TemporaryDirectory() as tmp:
        iconset = os.path.join(tmp, "icon.iconset")
        os.mkdir(iconset)
        for base in (16, 32, 128, 256, 512):
            for scale in (1, 2):
                px = base * scale
                name = f"icon_{base}x{base}{'@2x' if scale == 2 else ''}.png"
                apple_canvas(contours, bounds, px).save(os.path.join(iconset, name))
        subprocess.run(["iconutil", "-c", "icns", iconset, "-o", path], check=True)


def write_svg(svg_pen, bounds, path):
    """应用内矢量标：小图版式（墨底、亮金、无内框），标题栏 22px 也清楚。"""
    x_min, y_min, x_max, y_max = bounds
    box = 1000
    _, radius, extent, lift = layout(32, True)
    scale = extent * box / max(x_max - x_min, y_max - y_min)
    cx = box / 2 - (x_min + x_max) / 2 * scale
    cy = box / 2 + lift * box + (y_min + y_max) / 2 * scale
    gold = "#%02x%02x%02x" % GOLD_SMALL
    top = "#%02x%02x%02x" % INK_TOP
    bottom = "#%02x%02x%02x" % INK_BOTTOM
    svg = f'''<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {box} {box}" role="img" aria-label="落墨">
  <!-- 落墨品牌标：墨底 + 亮金「墨」。字形 Noto Serif TC wght=600（SIL OFL 1.1） -->
  <!-- 本文件由 scripts/generate_brand_icons.py 生成，请勿手改 -->
  <defs>
    <linearGradient id="luomoInk" x1="0" y1="0" x2="0" y2="1">
      <stop offset="0" stop-color="{top}"/>
      <stop offset="1" stop-color="{bottom}"/>
    </linearGradient>
  </defs>
  <rect width="{box}" height="{box}" rx="{radius * box:.0f}" fill="url(#luomoInk)"/>
  <path transform="matrix({scale:.5f} 0 0 {-scale:.5f} {cx:.2f} {cy:.2f})" fill="{gold}" d="{svg_pen.getCommands()}"/>
</svg>
'''
    with open(path, "w", encoding="utf-8") as f:
        f.write(svg)


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--font", default=os.environ.get("ATT_BRAND_FONT"), help="NotoSerifTC[wght].ttf 路径（或环境变量 ATT_BRAND_FONT）")
    parser.add_argument("--preview", help="只输出各尺寸预览拼图到该路径，不改动仓库文件")
    args = parser.parse_args()
    if not args.font or not os.path.isfile(args.font):
        parser.error("找不到字体：用 --font 或 ATT_BRAND_FONT 指定 Noto Serif TC（OFL，可从 Google Fonts 下载）")

    font = open_font(args.font)
    contours, bounds, svg_pen = load_glyph(font, CHAR)
    ico_images = [render(contours, bounds, px) for px in ICO_SIZES]
    tray = {}
    for state, (char, palette) in TRAY.items():
        t_contours, t_bounds, _ = load_glyph(font, char)
        tray[state] = {px: render(t_contours, t_bounds, px, small=True, palette=palette)
                       for px in TRAY_WIN_SIZES + [TRAY_MAC_SIZE]}

    if args.preview:
        # 每个尺寸按原像素 + 最近邻放大并排，便于检查小尺寸是否发糊
        zoom = 6
        width = sum(im.size[0] * zoom + 12 for im in ico_images[:6]) + 12
        sheet = Image.new("RGBA", (width, 48 * zoom + 24 + 300 + 4 * 60), (36, 36, 40, 255))
        x = 12
        for im in ico_images[:6]:
            sheet.alpha_composite(im.resize((im.size[0] * zoom, im.size[1] * zoom), Image.NEAREST), (x, 12))
            x += im.size[0] * zoom + 12
        x = 12
        for im in ico_images:
            sheet.alpha_composite(im, (x, 48 * zoom + 24))
            x += im.size[0] + 12
        # 托盘四态各一行：实际像素
        y = 48 * zoom + 24 + 300
        for state in TRAY:
            x = 12
            for px in TRAY_WIN_SIZES + [TRAY_MAC_SIZE]:
                sheet.alpha_composite(tray[state][px], (x, y))
                x += px + 12
            y += 60
        sheet.save(args.preview)
        return

    write_ico(ico_images, os.path.join(ICONS, "icon.ico"))
    render(contours, bounds, 512).save(os.path.join(ICONS, "icon.png"), optimize=True)
    write_icns(contours, bounds, os.path.join(ICONS, "icon.icns"))
    write_svg(svg_pen, bounds, SVG_OUT)
    tray_dir = os.path.join(ICONS, "tray")
    for state, images in tray.items():
        for px in TRAY_WIN_SIZES:
            images[px].save(os.path.join(tray_dir, f"win-{state}-{px}.png"), optimize=True)
        images[TRAY_MAC_SIZE].save(os.path.join(tray_dir, f"macos-{state}.png"), optimize=True)
    print("已生成 icon.ico / icon.png / icon.icns / brand-mark.svg / 托盘四态")


if __name__ == "__main__":
    main()
