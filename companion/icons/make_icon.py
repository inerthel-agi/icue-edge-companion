# Generates icons/icon.ico (16/32/48/256 px PNG frames) and icons/icon.svg with the stdlib only.
# An "E" for Edge made of three level bars: Codex (blue), media (green), Claude (clay).
import struct, zlib, pathlib

BG, SPINE = (15, 18, 20), (236, 236, 236)
CODEX, MEDIA, CLAUDE = (156, 200, 236), (29, 185, 84), (224, 169, 140)
# (x, y, width, height, radius, colour) on a 256-unit canvas, painted in order.
SHAPES = [
    (8, 8, 240, 240, 56, BG),
    (60, 52, 26, 152, 13, SPINE),
    (60, 52, 140, 30, 15, CODEX),
    (60, 113, 104, 30, 15, MEDIA),
    (60, 174, 140, 30, 15, CLAUDE),
]
SS = 4  # samples per pixel side, for smooth edges at 16 px

def inside(px, py, x, y, w, h, r):
    dx = max(x + r - px, 0, px - (x + w - r))
    dy = max(y + r - py, 0, py - (y + h - r))
    return x <= px <= x + w and y <= py <= y + h and dx * dx + dy * dy <= r * r

def pixel(col, row, n):
    acc, alpha = [0, 0, 0], 0
    for sy in range(SS):
        for sx in range(SS):
            px, py = (col + (sx + 0.5) / SS) * 256 / n, (row + (sy + 0.5) / SS) * 256 / n
            colour = None
            for x, y, w, h, r, c in SHAPES:
                if inside(px, py, x, y, w, h, r):
                    colour = c
            if colour:
                alpha += 1
                for i in range(3):
                    acc[i] += colour[i]
    if not alpha:
        return (0, 0, 0, 0)
    return (*(round(v / alpha) for v in acc), round(255 * alpha / SS ** 2))

def png(n):
    raw = b"".join(b"\0" + b"".join(bytes(pixel(x, y, n)) for x in range(n)) for y in range(n))
    chunk = lambda t, d: struct.pack(">I", len(d)) + t + d + struct.pack(">I", zlib.crc32(t + d) & 0xFFFFFFFF)
    return b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", n, n, 8, 6, 0, 0, 0)) + chunk(b"IDAT", zlib.compress(raw, 9)) + chunk(b"IEND", b"")

here = pathlib.Path(__file__).parent
sizes = [16, 32, 48, 256]
images = [png(n) for n in sizes]
header = struct.pack("<HHH", 0, 1, len(sizes))
offset = 6 + 16 * len(sizes)
entries = b""
for n, data in zip(sizes, images):
    entries += struct.pack("<BBBBHHII", n % 256, n % 256, 0, 0, 1, 32, len(data), offset)
    offset += len(data)
ico = here / "icon.ico"
ico.write_bytes(header + entries + b"".join(images))
(here / "icon-256.png").write_bytes(images[-1])

hexc = lambda c: "#%02x%02x%02x" % c
svg = '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 256 256">\n' + "".join(
    f'  <rect x="{x}" y="{y}" width="{w}" height="{h}" rx="{r}" fill="{hexc(c)}"/>\n' for x, y, w, h, r, c in SHAPES) + "</svg>\n"
(here / "icon.svg").write_text(svg, encoding="utf-8")
print(ico, ico.stat().st_size, "bytes")
