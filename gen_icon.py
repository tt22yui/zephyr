# -*- coding: utf-8 -*-
"""Generate the new Zephyr app icon source PNG (1024x1024)."""
import math
from PIL import Image, ImageDraw, ImageFilter, ImageChops

CANVAS = 1024
SS = 4                      # supersample factor
W = CANVAS * SS

tile = (48, 48, 976, 976)   # tile in 1024 space
tile_ss = tuple(v * SS for v in tile)
radius = 220

img = Image.new("RGBA", (W, W), (0, 0, 0, 0))

def c_lerp(c1, c2, t):
    return tuple(int(round(c1[i] + (c2[i] - c1[i]) * t)) for i in range(3))

# ---- background: rounded tile with diagonal gradient ----
c_tl = (46, 56, 112)   # deep ink-navy top-left
c_br = (10, 26, 46)    # deep ink-navy bottom-right
wx0, wy0, wx1, wy1 = tile
diag = math.hypot(wx1 - wx0, wy1 - wy0) * SS

mask = Image.new("L", (W, W), 0)
ImageDraw.Draw(mask).rounded_rectangle(tile_ss, radius=radius * SS, fill=255)

bg = Image.new("RGBA", (W, W), (0, 0, 0, 0))
bpx = bg.load()
for y in range(W):
    for x in range(W):
        vx = x - wx0 * SS
        vy = y - wy0 * SS
        t = max(0.0, min(1.0, (vx + vy) / diag))
        col = c_lerp(c_tl, c_br, t)
        bpx[x, y] = (col[0], col[1], col[2], 255)
bg = Image.composite(bg, Image.new("RGBA", (W, W), (0, 0, 0, 0)), mask)

# ---- radial cyan glow behind mark ----
glow = Image.new("RGBA", (W, W), (0, 0, 0, 0))
gd = ImageDraw.Draw(glow)
gx, gy = 512 * SS, 560 * SS
gr = 520 * SS
for step in range(60):
    rr = gr * step / 59.0
    a = int(46 * (1 - step / 59.0) ** 2)
    gd.ellipse([gx - rr, gy - rr, gx + rr, gy + rr], fill=(86, 200, 235, a))
glow = glow.filter(ImageFilter.GaussianBlur(radius=60 * SS))
bg = Image.alpha_composite(bg, glow)

def poly(pts):
    return [(int(x * SS), int(y * SS)) for x, y in pts]

p1 = [(222, 304), (682, 304), (722, 396), (262, 396)]   # top, near-white
p2 = [(262, 410), (722, 410), (762, 502), (302, 502)]   # mid, mid-cyan
p3 = [(302, 516), (762, 516), (802, 608), (342, 608)]   # bottom, teal

mark = bg.copy()
md = ImageDraw.Draw(mark)
md.polygon(poly(p1), fill=(248, 252, 255, 255))
md.polygon(poly(p2), fill=(166, 226, 244, 255))
md.polygon(poly(p3), fill=(84, 196, 228, 255))

# top-light sheen across the mark for dimensional depth
mpoly = poly(p1) + poly(p2) + poly(p3)
mbbox = ImageDraw.ImageDraw(mark)
msk = Image.new("L", (W, W), 0)
ImageDraw.Draw(msk).polygon(mpoly, fill=255)
sheen = Image.new("RGBA", (W, W), (0, 0, 0, 0))
sx = sheen.load()
for y in range(W):
    t = max(0.0, min(1.0, 1.0 - y / W))
    a = int(70 * t * t)
    for x in range(W):
        sx[x, y] = (255, 255, 255, a)
sheen.putalpha(ImageChops.multiply(sheen.split()[3], msk))
mark = Image.alpha_composite(mark, sheen)

# amber wind orb (sun) upper-left
orb_c = (255, 180, 58)
o = Image.new("RGBA", (W, W), (0, 0, 0, 0))
od = ImageDraw.Draw(o)
cx, cy, cr = 268 * SS, 238 * SS, 38 * SS
for step in range(40):
    rr = cr * (1 + 1.6 * step / 39.0)
    a = int(120 * (1 - step / 39.0) ** 1.6)
    od.ellipse([cx - rr, cy - rr, cx + rr, cy + rr], fill=(255, 196, 92, a))
od.ellipse([cx - cr, cy - cr, cx + cr, cy + cr], fill=(orb_c[0], orb_c[1], orb_c[2], 255))
o = o.filter(ImageFilter.GaussianBlur(radius=12 * SS))
o = Image.alpha_composite(Image.new("RGBA", (W, W), (0, 0, 0, 0)), o)
od2 = ImageDraw.Draw(o)
od2.ellipse([cx - cr, cy - cr, cx + cr, cy + cr], fill=(orb_c[0], orb_c[1], orb_c[2], 255))
mark = Image.alpha_composite(mark, o)

img = Image.alpha_composite(img, mark)
img = Image.composite(img, Image.new("RGBA", (W, W), (0, 0, 0, 0)), mask)
img = img.resize((CANVAS, CANVAS), Image.LANCZOS)
img.save("icon-source.png")
print("saved icon-source.png", img.size)