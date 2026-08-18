#!/usr/bin/env python3
"""Generate the AudioDistillery app icon (1024x1024 RGBA PNG) without any
image library: shapes are rasterized with supersampling and written through
zlib. Design: the "Alambic" copper drop (the Still wordmark) with waveform
bars cut out, on a dark rounded square — matching the app's design system.

Usage: python3 scripts/gen-icon.py design/icon-1024.png
"""
import math
import struct
import sys
import zlib

SIZE = 1024
SS = 2  # supersampling factor

# Design tokens (Alambic).
BG_TOP = (0x21, 0x1B, 0x13)
BG_BOT = (0x10, 0x0D, 0x09)
COPPER_TOP = (0xF4, 0xB2, 0x69)
COPPER_BOT = (0xC0, 0x6D, 0x28)
BAR = (0x17, 0x13, 0x0E)

# Rounded-square with macOS-like margins.
MARGIN = 0.09
RADIUS = 0.215

# Drop geometry (in icon units, center x = 0.5).
APEX_Y = 0.175
CIRCLE_CY = 0.585
CIRCLE_R = 0.235

# Waveform bars inside the drop: (x offset from center, half-height).
BARS = [(-0.132, 0.045), (-0.066, 0.095), (0.0, 0.062), (0.066, 0.115), (0.132, 0.075)]
BAR_W = 0.040
BAR_CY = 0.585


def rounded_sq(x, y):
    lo, hi, r = MARGIN, 1.0 - MARGIN, RADIUS
    if x < lo or x > hi or y < lo or y > hi:
        return False
    dx = max(lo + r - x, 0, x - (hi - r))
    dy = max(lo + r - y, 0, y - (hi - r))
    return dx * dx + dy * dy <= r * r


def in_drop(x, y):
    """Teardrop: circle + tangent cone up to the apex."""
    ax, ay = 0.5, APEX_Y
    cx, cy, r = 0.5, CIRCLE_CY, CIRCLE_R
    if (x - cx) ** 2 + (y - cy) ** 2 <= r * r:
        return True
    d = cy - ay
    if d <= r or y < ay or y > cy:
        return False
    # Half-width of the cone at height y (linear from apex to tangent point).
    beta = math.acos(r / d)  # angle C->A to C->tangent point
    ty = cy - r * math.cos(beta)  # tangent point height
    tx = r * math.sin(beta)  # tangent point half-width
    if y > ty:
        return False
    half = tx * (y - ay) / (ty - ay)
    return abs(x - cx) <= half


def in_bar(x, y):
    for off, hh in BARS:
        bx = 0.5 + off
        if abs(x - bx) <= BAR_W / 2 and abs(y - BAR_CY) <= hh:
            # Rounded bar ends.
            if abs(y - BAR_CY) > hh - BAR_W / 2:
                ey = BAR_CY + math.copysign(hh - BAR_W / 2, y - BAR_CY)
                if (x - bx) ** 2 + (y - ey) ** 2 > (BAR_W / 2) ** 2:
                    continue
            return True
    return False


def lerp(a, b, t):
    return tuple(int(a[i] + (b[i] - a[i]) * t) for i in range(3))


def pixel(px, py):
    """Average SS*SS samples -> (r, g, b, a)."""
    acc = [0, 0, 0, 0]
    for sy in range(SS):
        for sx in range(SS):
            x = (px + (sx + 0.5) / SS) / SIZE
            y = (py + (sy + 0.5) / SS) / SIZE
            if not rounded_sq(x, y):
                continue
            if in_drop(x, y) and not in_bar(x, y):
                t = (y - APEX_Y) / (CIRCLE_CY + CIRCLE_R - APEX_Y)
                c = lerp(COPPER_TOP, COPPER_BOT, max(0.0, min(1.0, t)))
            elif in_drop(x, y):
                c = BAR
            else:
                c = lerp(BG_TOP, BG_BOT, y)
            acc[0] += c[0]
            acc[1] += c[1]
            acc[2] += c[2]
            acc[3] += 255
    n = SS * SS
    if acc[3] == 0:
        return (0, 0, 0, 0)
    a = acc[3] // n
    return (acc[0] // n, acc[1] // n, acc[2] // n, a)


def write_png(path, w, h, rows):
    def chunk(tag, data):
        raw = tag + data
        return struct.pack(">I", len(data)) + raw + struct.pack(">I", zlib.crc32(raw))

    sig = b"\x89PNG\r\n\x1a\n"
    ihdr = struct.pack(">IIBBBBB", w, h, 8, 6, 0, 0, 0)
    body = b"".join(b"\x00" + row for row in rows)
    with open(path, "wb") as f:
        f.write(sig)
        f.write(chunk(b"IHDR", ihdr))
        f.write(chunk(b"IDAT", zlib.compress(body, 9)))
        f.write(chunk(b"IEND", b""))


def main():
    out = sys.argv[1] if len(sys.argv) > 1 else "design/icon-1024.png"
    rows = []
    for py in range(SIZE):
        row = bytearray()
        for px in range(SIZE):
            r, g, b, a = pixel(px, py)
            row += bytes((r, g, b, a))
        rows.append(bytes(row))
        if py % 128 == 0:
            print(f"  row {py}/{SIZE}")
    write_png(out, SIZE, SIZE, rows)
    print(f"wrote {out}")


if __name__ == "__main__":
    main()
