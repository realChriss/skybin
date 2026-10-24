import math
from pathlib import Path
from PIL import Image, ImageDraw

OUT = Path(__file__).resolve().parent.parent / "assets"
S = 2048
K = S / 256
BORDER = 12
TILT = 8
SHADOW_OFFSET = (3, 6)


def circle(cx, cy, r, n=96):
    return ellipse(cx, cy, r, r, n)


def ellipse(cx, cy, rx, ry, n=128):
    return [(cx + rx * math.cos(2 * math.pi * i / n), cy + ry * math.sin(2 * math.pi * i / n)) for i in range(n)]


def quad(p0, p1, p2, n=16):
    return [
        ((1 - t) ** 2 * p0[0] + 2 * (1 - t) * t * p1[0] + t * t * p2[0],
         (1 - t) ** 2 * p0[1] + 2 * (1 - t) * t * p1[1] + t * t * p2[1])
        for t in (i / n for i in range(n + 1))
    ]


cloud = [
    circle(104, 70, 18), circle(128, 60, 24), circle(152, 72, 16),
    circle(98, 78, 10), circle(158, 78, 10), [(98, 68), (158, 68), (158, 88), (98, 88)],
]
rim = ellipse(128, 112, 62, 16)
body = [(66, 112), *quad((84, 194), (86, 204), (96, 204)), *quad((160, 204), (170, 204), (172, 194)), (190, 112)]
opening = ellipse(128, 112, 50, 10)


def fill(layer, shapes, color, grow=0, offset=(0, 0)):
    d = ImageDraw.Draw(layer)
    for shape in shapes:
        pts = [((x + offset[0]) * K, (y + offset[1]) * K) for x, y in shape]
        d.polygon(pts, fill=color)
        # rounding every vertex makes this an exact round-join outline, like an SVG stroke
        if grow:
            g = grow * K
            d.line(pts + [pts[0]], fill=color, width=round(2 * g))
            for x, y in pts:
                d.ellipse((x - g, y - g, x + g, y + g), fill=color)
    return layer


silhouette = cloud + [rim, body]
shadow = fill(Image.new("RGBA", (S, S)), silhouette, (10, 26, 51, 72), BORDER, SHADOW_OFFSET)
im = fill(Image.new("RGBA", (S, S)), silhouette, "#FFFFFF", BORDER)
fill(im, cloud, "#CFE0F7")
fill(im, [body], "#2D64B4")
fill(im, [rim], "#6E9BDB")
fill(im, [opening], "#163764")
im = Image.alpha_composite(shadow, im).rotate(TILT, resample=Image.BICUBIC, center=(S / 2, S / 2))

MARGIN = 0.02
im = im.crop(im.getbbox())
side = round(max(im.size) / (1 - 2 * MARGIN))
square = Image.new("RGBA", (side, side))
square.paste(im, ((side - im.width) // 2, (side - im.height) // 2))
im = square

sizes = [16, 20, 24, 32, 40, 48, 64, 256]
frames = [im.resize((s, s), Image.LANCZOS) for s in sizes]
frames[-1].save(OUT / "icon.ico", sizes=[(s, s) for s in sizes], append_images=frames)
frames[-1].save(OUT / "icon.png")

# macOS icons leave a margin around the artwork (824 of 1024 px on Apple's grid)
mac = Image.new("RGBA", (1024, 1024))
mac.paste(im.resize((824, 824), Image.LANCZOS), (100, 100))
mac.save(OUT / "icon.icns")
