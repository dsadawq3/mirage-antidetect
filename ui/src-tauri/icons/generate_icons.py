#!/usr/bin/env python3
"""Generate Mirage app icons (dark rounded square + cyan layer chevrons).

Run from this directory:  python generate_icons.py
Writes icon.ico, icon.png and the PNG sizes referenced by tauri.conf.json.
Requires Pillow (pip install pillow).
"""
import os
from PIL import Image, ImageDraw

HERE = os.path.dirname(os.path.abspath(__file__))
SS = 8  # supersampling factor for clean edges

BG = (18, 23, 34, 255)        # #121722
ACCENT = (34, 211, 238, 255)  # #22d3ee


def chevron(draw, cx, top, half, depth, thick, color):
    pts = [
        (cx - half, top),
        (cx, top + depth),
        (cx + half, top),
        (cx + half, top + thick),
        (cx, top + depth + thick),
        (cx - half, top + thick),
    ]
    draw.polygon(pts, fill=color)


def render(size):
    s = size * SS
    img = Image.new("RGBA", (s, s), (0, 0, 0, 0))
    d = ImageDraw.Draw(img)

    radius = int(s * 0.22)
    d.rounded_rectangle([0, 0, s - 1, s - 1], radius=radius, fill=BG)

    cx = s / 2.0
    half = s * 0.27
    depth = s * 0.135
    thick = s * 0.058
    top0 = s * 0.255
    step = s * 0.155

    for i in range(3):
        chevron(d, cx, top0 + i * step, half, depth, thick, ACCENT)

    return img.resize((size, size), Image.LANCZOS)


def main():
    master = render(512)
    master.save(os.path.join(HERE, "icon.png"))

    for name, size in (("32x32.png", 32), ("128x128.png", 128), ("128x128@2x.png", 256)):
        render(size).save(os.path.join(HERE, name))

    # Multi-resolution .ico used by the Windows resource compiler.
    render(256).save(
        os.path.join(HERE, "icon.ico"),
        sizes=[(16, 16), (24, 24), (32, 32), (48, 48), (64, 64), (128, 128), (256, 256)],
    )
    print("icons written to", HERE)


if __name__ == "__main__":
    main()
