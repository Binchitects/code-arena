# Compares two tours' pictures (scripts/ui/tour.sh) step by step: for each step, whether the two are the same, or
# how many pixels differ and in which rectangle. A rectangle may be left out of the comparison (what moves by
# itself). With only zlib, so it runs anywhere the pictures are (shot.py's PNGs: 8 bits a channel, RGB or RGBA).
#
#   python3 scripts/ui/compare.py target/ui/before target/ui/after              # the whole window
#   python3 scripts/ui/compare.py target/ui/before target/ui/after 901,36,1280,664   # without x0,y0,x1,y1
#
# It ends with 1 when a step differs (or is missing from the second tour), so a script can stop on it.
import os
import struct
import sys
import zlib


def read(path):
    """A PNG's size, its bytes a pixel, and its rows as bytes."""
    data = open(path, "rb").read()
    if data[:8] != b"\x89PNG\r\n\x1a\n":
        sys.exit(f"{path}: not a PNG")
    at, packed, width, height, bpp = 8, b"", 0, 0, 3
    while at < len(data):
        length, kind = struct.unpack(">I4s", data[at:at + 8])
        body = data[at + 8:at + 8 + length]
        if kind == b"IHDR":
            width, height, depth, colour = struct.unpack(">IIBB", body[:10])
            if depth != 8 or colour not in (2, 6):
                sys.exit(f"{path}: only 8-bit RGB or RGBA pictures are read")
            bpp = 3 if colour == 2 else 4
        elif kind == b"IDAT":
            packed += body
        at += 12 + length
    raw = zlib.decompress(packed)
    stride = width * bpp
    rows, before = [], bytearray(stride)
    for y in range(height):
        start = y * (stride + 1)
        how, line = raw[start], bytearray(raw[start + 1:start + 1 + stride])
        if how == 1:
            for i in range(bpp, stride):
                line[i] = (line[i] + line[i - bpp]) & 255
        elif how == 2:
            for i in range(stride):
                line[i] = (line[i] + before[i]) & 255
        elif how == 3:
            for i in range(stride):
                left = line[i - bpp] if i >= bpp else 0
                line[i] = (line[i] + ((left + before[i]) >> 1)) & 255
        elif how == 4:
            for i in range(stride):
                a = line[i - bpp] if i >= bpp else 0
                b = before[i]
                c = before[i - bpp] if i >= bpp else 0
                p = a + b - c
                pa, pb, pc = abs(p - a), abs(p - b), abs(p - c)
                line[i] = (line[i] + (a if pa <= pb and pa <= pc else b if pb <= pc else c)) & 255
        rows.append(bytes(line))
        before = line
    return width, height, bpp, rows


def differ(a, b, without):
    """How many pixels differ between two pictures, and the rectangle they are in; none when they are the same."""
    wa, ha, ba, rows_a = read(a)
    wb, hb, bb, rows_b = read(b)
    if (wa, ha) != (wb, hb):
        return f"sizes differ: {wa}x{ha} and {wb}x{hb}"
    count, box = 0, None
    for y in range(ha):
        ra, rb = rows_a[y], rows_b[y]
        if ra == rb and ba == bb:
            continue
        for x in range(wa):
            if without and without[0] <= x < without[2] and without[1] <= y < without[3]:
                continue
            if ra[x * ba:x * ba + 3] != rb[x * bb:x * bb + 3]:
                count += 1
                box = (x, y, x + 1, y + 1) if box is None else (
                    min(box[0], x), min(box[1], y), max(box[2], x + 1), max(box[3], y + 1))
    return f"{count} px differ in {box}" if count else None


def main():
    if len(sys.argv) < 3:
        sys.exit("usage: compare.py BEFORE AFTER [x0,y0,x1,y1 left out]")
    before, after = sys.argv[1], sys.argv[2]
    without = tuple(int(n) for n in sys.argv[3].split(",")) if len(sys.argv) > 3 else None
    changed = 0
    for name in sorted(os.listdir(before)):
        if not name.endswith(".png"):
            continue
        other = os.path.join(after, name)
        said = differ(os.path.join(before, name), other, without) if os.path.exists(other) else f"not in {after}"
        print(f"{name}: {said or 'same'}")
        changed += 1 if said else 0
    sys.exit(1 if changed else 0)


if __name__ == "__main__":
    main()
