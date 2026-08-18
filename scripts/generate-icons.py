#!/usr/bin/env python3
"""Generate placeholder app icons for the Tauri shell.

Draws a rounded-square gradient with a white music note, then writes the
png / icns / ico files Tauri expects under src-tauri/icons/.  Stdlib only
(no Pillow), so it runs anywhere with Python 3.

Usage: python3 scripts/generate-icons.py
"""
import os
import struct
import zlib

HERE = os.path.dirname(os.path.abspath(__file__))
OUT_DIR = os.path.join(os.path.dirname(HERE), "src-tauri", "icons")


# ---------------------------------------------------------------------------
# PNG encoding (RGBA, 8-bit, stdlib only)
# ---------------------------------------------------------------------------

def make_png(width, height, pixel_fn):
    """pixel_fn(x, y) -> (r, g, b, a); returns encoded PNG bytes."""
    def chunk(tag, data):
        return (struct.pack(">I", len(data)) + tag + data
                + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF))

    rows = [b"\x00" + bytes(v for x in range(width) for v in pixel_fn(x, y))
            for y in range(height)]
    ihdr = struct.pack(">IIBBBBB", width, height, 8, 6, 0, 0, 0)
    return (b"\x89PNG\r\n\x1a\n"
            + chunk(b"IHDR", ihdr)
            + chunk(b"IDAT", zlib.compress(b"".join(rows), 9))
            + chunk(b"IEND", b""))


# ---------------------------------------------------------------------------
# Drawing helpers
# ---------------------------------------------------------------------------

def in_rounded_rect(x, y, size, radius):
    s = size - 1
    if not (0 <= x <= s and 0 <= y <= s):
        return False
    cx = min(max(x, radius), s - radius)
    cy = min(max(y, radius), s - radius)
    return (x - cx) ** 2 + (y - cy) ** 2 <= radius ** 2


def in_circle(x, y, cx, cy, radius):
    return (x - cx) ** 2 + (y - cy) ** 2 <= radius ** 2


def in_triangle(x, y, tri):
    def sign(p1, p2):
        return ((p2[0] - p1[0]) * (y - p1[1])
                - (p2[1] - p1[1]) * (x - p1[0]))
    d = (sign(tri[0], tri[1]), sign(tri[1], tri[2]), sign(tri[2], tri[0]))
    has_neg = any(v < 0 for v in d)
    has_pos = any(v > 0 for v in d)
    return not (has_neg and has_pos)


def design_pixel_fn(size):
    """Build the per-pixel function for the app icon at a given canvas size."""
    radius = 0.24 * size
    # Background gradient: deep blue (top) -> violet (bottom)
    top = (38, 132, 255)
    bottom = (106, 17, 203)
    # Music note geometry (white)
    head_cx, head_cy, head_r = 0.40 * size, 0.62 * size, 0.115 * size
    stem_x0, stem_x1 = 0.50 * size, 0.545 * size
    stem_y0 = 0.26 * size
    flag = [
        (0.545 * size, 0.26 * size),
        (0.75 * size, 0.305 * size),
        (0.545 * size, 0.50 * size),
    ]

    def pixel_fn(x, y):
        if not in_rounded_rect(x, y, size, radius):
            return (0, 0, 0, 0)
        t = y / (size - 1)
        bg = (
            int(top[0] + (bottom[0] - top[0]) * t),
            int(top[1] + (bottom[1] - top[1]) * t),
            int(top[2] + (bottom[2] - top[2]) * t),
        )
        if (in_circle(x, y, head_cx, head_cy, head_r)
                or (stem_x0 <= x <= stem_x1 and stem_y0 <= y <= head_cy)
                or in_triangle(float(x), float(y), flag)):
            return (255, 255, 255, 255)
        return bg + (255,)

    return pixel_fn


# ---------------------------------------------------------------------------
# icns / ico containers
# ---------------------------------------------------------------------------

def make_icns(entries):
    """entries: list of (fourcc, png_bytes)."""
    data = b"".join(struct.pack(">4sI", code.encode(), 8 + len(png)) + png
                    for code, png in entries)
    return b"icns" + struct.pack(">I", 8 + len(data)) + data


def make_ico(images):
    """images: list of (width, png_bytes); emits PNG-compressed ICO entries."""
    count = len(images)
    header = struct.pack("<HHH", 0, 1, count)
    offset = 6 + 16 * count
    entries = b""
    payload = b""
    for width, png in images:
        dim = 0 if width >= 256 else width
        entries += struct.pack("<BBBBHHII", dim, dim, 0, 0, 1, 32, len(png), offset)
        payload += png
        offset += len(png)
    return header + entries + payload


# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------

def main():
    os.makedirs(OUT_DIR, exist_ok=True)

    def write(name, data):
        with open(os.path.join(OUT_DIR, name), "wb") as f:
            f.write(data)
        print(f"  {name:20s} {len(data):>7} bytes")

    print(f"Generating icons into {OUT_DIR}")

    # PNGs (different sizes drawn directly, no resizing)
    for size, name in ((512, "icon.png"),
                       (128, "128x128.png"),
                       (32, "32x32.png")):
        write(name, make_png(size, size, design_pixel_fn(size)))
    write("128x128@2x.png", make_png(256, 256, design_pixel_fn(256)))

    # .icns (PNG-compressed entries; modern macOS accepts them)
    icns = make_icns([
        ("ic07", make_png(128, 128, design_pixel_fn(128))),
        ("ic08", make_png(256, 256, design_pixel_fn(256))),
        ("ic09", make_png(512, 512, design_pixel_fn(512))),
    ])
    write("icon.icns", icns)

    # .ico (PNG-compressed entries)
    ico = make_ico([
        (256, make_png(256, 256, design_pixel_fn(256))),
        (128, make_png(128, 128, design_pixel_fn(128))),
        (48, make_png(48, 48, design_pixel_fn(48))),
        (32, make_png(32, 32, design_pixel_fn(32))),
        (16, make_png(16, 16, design_pixel_fn(16))),
    ])
    write("icon.ico", ico)

    print("Done.")


if __name__ == "__main__":
    main()