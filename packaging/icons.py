#!/usr/bin/env python3
"""Generate Brazap's installer and desktop icons from `assets/logo.png`.

The logo stays the only source of truth. Like `build.rs`, this crops the square
mark out of the wide artwork (318, 64, 894 x 894). It needs only the Python
standard library, so it runs unchanged on the Linux, macOS and Windows runners.

    icons.py linux   OUT   OUT/hicolor/<size>x<size>/apps/brazap.png
    icons.py macos   OUT   OUT/Brazap.iconset/icon_<size>x<size>[@2x].png (for iconutil)
    icons.py windows OUT   OUT/brazap.ico (the installer's icon)
"""

import argparse
import math
import operator
import struct
import sys
import zlib
from pathlib import Path

LOGO = Path(__file__).resolve().parents[1] / "assets" / "logo.png"
# Keep in sync with the crop in build.rs, which builds the executable's icon.
CROP_X, CROP_Y, CROP_SIZE = 318, 64, 894


def read_crop(path):
    """Decode the cropped square of an 8-bit, non-interlaced RGBA PNG."""
    data = path.read_bytes()
    if data[:8] != b"\x89PNG\r\n\x1a\n":
        sys.exit(f"{path} is not a PNG file")
    offset, idat, header = 8, [], None
    while offset < len(data):
        (length,) = struct.unpack(">I", data[offset : offset + 4])
        kind = data[offset + 4 : offset + 8]
        body = data[offset + 8 : offset + 8 + length]
        if kind == b"IHDR":
            header = struct.unpack(">IIBBBBB", body)
        elif kind == b"IDAT":
            idat.append(body)
        offset += 12 + length
    width, height, depth, color, _, _, interlace = header
    if (depth, color, interlace) != (8, 6, 0):
        sys.exit(f"{path} must be an 8-bit RGBA PNG without interlacing")
    if CROP_X + CROP_SIZE > width or CROP_Y + CROP_SIZE > height:
        sys.exit(f"{path} is smaller than the icon crop")
    raw = zlib.decompress(b"".join(idat))
    stride = width * 4
    previous = bytearray(stride)
    planes = [[], [], [], []]
    for y in range(CROP_Y + CROP_SIZE):
        start = y * (stride + 1)
        kind, line = raw[start], bytearray(raw[start + 1 : start + 1 + stride])
        if kind == 1:
            for i in range(4, stride):
                line[i] = (line[i] + line[i - 4]) & 255
        elif kind == 2:
            line = bytearray(map(lambda a, b: (a + b) & 255, line, previous))
        elif kind == 3:
            for i in range(stride):
                left = line[i - 4] if i >= 4 else 0
                line[i] = (line[i] + ((left + previous[i]) >> 1)) & 255
        elif kind == 4:
            for i in range(stride):
                a = line[i - 4] if i >= 4 else 0
                b = previous[i]
                c = previous[i - 4] if i >= 4 else 0
                p = a + b - c
                pa, pb, pc = abs(p - a), abs(p - b), abs(p - c)
                line[i] = (line[i] + (a if pa <= pb and pa <= pc else b if pb <= pc else c)) & 255
        elif kind != 0:
            sys.exit(f"{path} uses an unknown PNG filter")
        previous = line
        if y >= CROP_Y:
            pixels = line[CROP_X * 4 : (CROP_X + CROP_SIZE) * 4]
            alpha = pixels[3::4]
            # Premultiplied channels keep transparent edges from darkening when resized.
            for channel in range(3):
                planes[channel].append([v * a / 255.0 for v, a in zip(pixels[channel::4], alpha)])
            planes[3].append([float(a) for a in alpha])
    return planes


def lanczos(x, lobes=3.0):
    if x == 0:
        return 1.0
    if abs(x) >= lobes:
        return 0.0
    x *= math.pi
    return lobes * math.sin(x) * math.sin(x / lobes) / (x * x)


def resize_rows(rows, size):
    """Resample every row to `size` samples with a Lanczos-3 filter."""
    source = len(rows[0])
    scale = source / size
    support = 3.0 * max(scale, 1.0)
    taps = []
    for out in range(size):
        center = (out + 0.5) * scale
        first = max(0, math.floor(center - support))
        last = min(source, math.ceil(center + support))
        weights = [lanczos((i + 0.5 - center) / max(scale, 1.0)) for i in range(first, last)]
        total = sum(weights)
        taps.append((first, last, [w / total for w in weights]))
    return [[sum(map(operator.mul, weights, row[first:last])) for first, last, weights in taps] for row in rows]


def resize(planes, size):
    resized = []
    for plane in planes:
        across = resize_rows(plane, size)
        down = resize_rows([list(column) for column in zip(*across)], size)
        resized.append([list(row) for row in zip(*down)])
    return resized


def place(planes, canvas, inset):
    """Center a square image on a transparent canvas."""
    size = len(planes[0])
    out = []
    for plane in planes:
        rows = [[0.0] * canvas for _ in range(canvas)]
        for y, row in enumerate(plane):
            rows[inset + y][inset : inset + size] = row
        out.append(rows)
    return out


def rgba(planes):
    """Straight-alpha RGBA bytes, one bytearray per row."""
    rows = []
    for r, g, b, a in zip(*planes):
        row = bytearray()
        for pr, pg, pb, pa in zip(r, g, b, a):
            alpha = min(255.0, max(0.0, pa))
            if alpha < 0.5:
                row += b"\0\0\0\0"
                continue
            unmultiply = 255.0 / alpha
            row += bytes(min(255, max(0, round(c * unmultiply))) for c in (pr, pg, pb))
            row.append(round(alpha))
        rows.append(row)
    return rows


def png(rows):
    width = len(rows[0]) // 4

    def chunk(kind, body):
        return struct.pack(">I", len(body)) + kind + body + struct.pack(">I", zlib.crc32(kind + body))

    pixels = b"".join(b"\0" + bytes(row) for row in rows)
    return (
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", struct.pack(">IIBBBBB", width, len(rows), 8, 6, 0, 0, 0))
        + chunk(b"IDAT", zlib.compress(pixels, 9))
        + chunk(b"IEND", b"")
    )


def dib(rows):
    """A 32-bit icon bitmap: bottom-up BGRA plus the (unused) 1-bit AND mask."""
    size = len(rows)
    header = struct.pack("<IiiHHIIiiII", 40, size, size * 2, 1, 32, 0, 0, 0, 0, 0, 0)
    pixels = b"".join(bytes(row[i + j] for i in range(0, len(row), 4) for j in (2, 1, 0, 3)) for row in reversed(rows))
    mask = bytes(((size + 31) // 32) * 4 * size)
    return header + pixels + mask


def ico(frames):
    """Small frames as bitmaps for older shell code paths, 256 px as PNG."""
    images = [(size, png(rows) if size >= 256 else dib(rows)) for size, rows in frames]
    offset = 6 + 16 * len(images)
    directory, payload = b"", b""
    for size, data in images:
        directory += struct.pack("<BBBBHHII", size % 256, size % 256, 0, 0, 1, 32, len(data), offset + len(payload))
        payload += data
    return struct.pack("<HHH", 0, 1, len(images)) + directory + payload


def write(path, data):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(data)
    print(path)


def shaped(source, canvas, inset_ratio, cache):
    """The mark, which has a transparent background, with platform-style margins."""
    body = round(canvas * (1 - 2 * inset_ratio))
    if body not in cache:
        # Derive small sizes from a nearby larger rendering to keep this quick.
        larger = [s for s in cache if s >= body * 2]
        base = cache[min(larger)] if larger else source
        cache[body] = resize(base, body)
    inset = (canvas - body) // 2
    return rgba(place(cache[body], canvas, inset))


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("platform", choices=["linux", "macos", "windows"])
    parser.add_argument("out", type=Path)
    args = parser.parse_args()
    source = read_crop(LOGO)
    cache = {}
    if args.platform == "macos":
        # Apple's icon grid keeps artwork within 824 px of a 1024 px canvas.
        for points in (512, 256, 128, 32, 16):
            for scale in (2, 1):
                pixels = points * scale
                name = f"icon_{points}x{points}{'@2x' if scale == 2 else ''}.png"
                rows = shaped(source, pixels, 100 / 1024, cache)
                write(args.out / "Brazap.iconset" / name, png(rows))
    elif args.platform == "linux":
        for size in (512, 256, 128, 64, 48, 32, 24, 16):
            rows = shaped(source, size, 1 / 32, cache)
            write(args.out / "hicolor" / f"{size}x{size}" / "apps" / "brazap.png", png(rows))
    else:
        # Match the executable's full-bleed icon from build.rs.
        frames = []
        for size in (256, 128, 64, 48, 32, 24, 16):
            larger = [s for s in cache if s >= size * 2]
            cache[size] = resize(cache[min(larger)] if larger else source, size)
            frames.append((size, rgba(cache[size])))
        write(args.out / "brazap.ico", ico(sorted(frames)))


if __name__ == "__main__":
    main()
