#!/usr/bin/env python3
"""Generate the application icon (SVG, PNGs and a Windows .ico) from one block design.

The icon is a white block "p" on a blue rounded tile. All shapes are rectangles on a 32x32
grid, so this script renders them itself and needs only the Python standard library.

Usage: python3 scripts/gen_icons.py [OUTPUT_DIR [EXTENSION_ICON_DIR]]
       (defaults: assets/icon and extension/icons)
"""

import struct
import sys
import zlib
from pathlib import Path

GRID = 32
TILE_RADIUS = 7

BLUE = (0x0D, 0x7F, 0xDA)
WHITE = (0xFF, 0xFF, 0xFF)
PALE_BLUE = (0xB8, 0xD4, 0xEA)

# The Linux package ships the 256 px PNG with its desktop entry; the .ico embeds all ICO_SIZES.
PNG_SIZES = [256]
# Sizes the browser extension's manifest lists.
EXTENSION_SIZES = [16, 32, 48, 128]
ICO_SIZES = [16, 20, 24, 32, 40, 48, 64, 128, 256]


def ring(x, y, size, stroke, color):
    return [
        (x, y, x + size, y + stroke, color),
        (x, y + size - stroke, x + size, y + size, color),
        (x, y + stroke, x + stroke, y + size - stroke, color),
        (x + size - stroke, y + stroke, x + size, y + size - stroke, color),
    ]


# A square ring with a stem hanging from its left side.
SHAPES = ring(10, 4, 12, 4, WHITE) + [(10, 16, 14, 27, PALE_BLUE)]


def hex_color(c):
    return "#" + bytes(c).hex()


def svg():
    lines = [
        f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {GRID} {GRID}" shape-rendering="crispEdges">',
        f'  <rect width="{GRID}" height="{GRID}" rx="{TILE_RADIUS}" fill="{hex_color(BLUE)}"/>',
    ]
    for x0, y0, x1, y1, c in SHAPES:
        lines.append(
            f'  <rect x="{x0}" y="{y0}" width="{x1 - x0}" height="{y1 - y0}" fill="{hex_color(c)}"/>'
        )
    lines.append("</svg>")
    return "\n".join(lines) + "\n"


def in_tile(gx, gy):
    r = TILE_RADIUS
    cx = min(max(gx, r), GRID - r)
    cy = min(max(gy, r), GRID - r)
    return (gx - cx) ** 2 + (gy - cy) ** 2 <= r * r


def render(size, ss=4):
    """Rows of (r, g, b, a) pixels with ss x ss supersampling."""
    unit = size / GRID
    rows = []
    for j in range(size):
        row = []
        for i in range(size):
            pr = pg = pb = pa = 0
            for sj in range(ss):
                for si in range(ss):
                    gx = (i + (si + 0.5) / ss) / unit
                    gy = (j + (sj + 0.5) / ss) / unit
                    if not in_tile(gx, gy):
                        continue
                    color = BLUE
                    for x0, y0, x1, y1, c in SHAPES:
                        if x0 <= gx < x1 and y0 <= gy < y1:
                            color = c
                    pr += color[0]
                    pg += color[1]
                    pb += color[2]
                    pa += 1
            n = ss * ss
            if pa == 0:
                row.append((0, 0, 0, 0))
            else:
                row.append((pr // pa, pg // pa, pb // pa, round(255 * pa / n)))
        rows.append(row)
    return rows


def png_bytes(rows):
    h, w = len(rows), len(rows[0])
    raw = b"".join(b"\x00" + bytes(v for px in row for v in px) for row in rows)

    def chunk(tag, data):
        body = tag + data
        return struct.pack(">I", len(data)) + body + struct.pack(">I", zlib.crc32(body) & 0xFFFFFFFF)

    return (
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 6, 0, 0, 0))
        + chunk(b"IDAT", zlib.compress(raw, 9))
        + chunk(b"IEND", b"")
    )


def dib_bytes(rows):
    """A 32-bit BGRA DIB with an AND mask, as stored inside .ico files."""
    h, w = len(rows), len(rows[0])
    header = struct.pack("<IiiHHIIiiII", 40, w, h * 2, 1, 32, 0, 0, 0, 0, 0, 0)
    pixels = b"".join(bytes((b, g, r, a)) for row in reversed(rows) for (r, g, b, a) in row)
    stride = (w + 31) // 32 * 4
    mask = bytearray()
    for row in reversed(rows):
        bits = bytearray(stride)
        for i, (_, _, _, a) in enumerate(row):
            if a == 0:
                bits[i // 8] |= 0x80 >> (i % 8)
        mask += bits
    return header + pixels + bytes(mask)


def ico_bytes(images):
    """images: list of (size, data). Sizes of 256 are stored as PNG, smaller ones as DIB."""
    count = len(images)
    out = struct.pack("<HHH", 0, 1, count)
    offset = 6 + 16 * count
    entries = b""
    blobs = b""
    for size, data in images:
        dim = 0 if size >= 256 else size
        entries += struct.pack("<BBBBHHII", dim, dim, 0, 0, 1, 32, len(data), offset)
        blobs += data
        offset += len(data)
    return out + entries + blobs


def main():
    out_dir = Path(sys.argv[1] if len(sys.argv) > 1 else "assets/icon")
    extension_dir = Path(sys.argv[2] if len(sys.argv) > 2 else "extension/icons")
    out_dir.mkdir(parents=True, exist_ok=True)
    extension_dir.mkdir(parents=True, exist_ok=True)
    (out_dir / "icon.svg").write_text(svg(), encoding="utf-8")

    sizes = set(PNG_SIZES) | set(ICO_SIZES) | set(EXTENSION_SIZES)
    rendered = {size: render(size) for size in sorted(sizes)}
    for size in PNG_SIZES:
        (out_dir / f"icon-{size}.png").write_bytes(png_bytes(rendered[size]))
    ico = ico_bytes(
        [(s, png_bytes(rendered[s]) if s >= 256 else dib_bytes(rendered[s])) for s in ICO_SIZES]
    )
    (out_dir / "icon.ico").write_bytes(ico)
    for size in EXTENSION_SIZES:
        (extension_dir / f"icon-{size}.png").write_bytes(png_bytes(rendered[size]))
    for d in (out_dir, extension_dir):
        print("wrote", d, ", ".join(sorted(p.name for p in d.iterdir())))


if __name__ == "__main__":
    main()
