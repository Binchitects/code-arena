# A screenshot of the virtual screen (or of one window, by its title) as a PNG, with only libX11 and zlib, for UI
# tests in the code-arena-gui image.
#
#   python3 scripts/ui/shot.py out.png                 # the whole screen
#   python3 scripts/ui/shot.py out.png "Code Arena"    # one window (the first one of that title)
import ctypes
import ctypes.util
import struct
import subprocess
import sys
import zlib

out = sys.argv[1]
title = sys.argv[2] if len(sys.argv) > 2 else None

x = ctypes.CDLL(ctypes.util.find_library("X11"))
x.XOpenDisplay.restype = ctypes.c_void_p
x.XDefaultRootWindow.restype = ctypes.c_ulong
x.XDefaultRootWindow.argtypes = [ctypes.c_void_p]
x.XGetImage.restype = ctypes.c_void_p
x.XGetImage.argtypes = [ctypes.c_void_p, ctypes.c_ulong, ctypes.c_int, ctypes.c_int, ctypes.c_uint, ctypes.c_uint,
                        ctypes.c_ulong, ctypes.c_int]
x.XGetGeometry.argtypes = [ctypes.c_void_p, ctypes.c_ulong, ctypes.POINTER(ctypes.c_ulong)] + \
    [ctypes.POINTER(ctypes.c_int)] * 2 + [ctypes.POINTER(ctypes.c_uint)] * 4
d = ctypes.c_void_p(x.XOpenDisplay(None))
if not d.value:
    sys.exit("no X display (is DISPLAY set and Xvfb running?)")

if title:
    # The window of that title, or of the folder's name before it ("work · Code Arena", as the main window is called).
    ids = subprocess.run(["xdotool", "search", "--name", f"{title}$"], capture_output=True, text=True).stdout.split()
    name = lambda w: subprocess.run(["xdotool", "getwindowname", w], capture_output=True, text=True).stdout.strip()
    found = [w for w in ids if name(w) == title or name(w).endswith(f" \u00b7 {title}")]
    if not found:
        sys.exit(f"no window named {title!r}")
    win = int(found[0])
else:
    win = x.XDefaultRootWindow(d)

root = ctypes.c_ulong()
gx, gy = ctypes.c_int(), ctypes.c_int()
w, h, bw, depth = ctypes.c_uint(), ctypes.c_uint(), ctypes.c_uint(), ctypes.c_uint()
x.XGetGeometry(d, win, ctypes.byref(root), ctypes.byref(gx), ctypes.byref(gy), ctypes.byref(w), ctypes.byref(h),
               ctypes.byref(bw), ctypes.byref(depth))
w, h = w.value, h.value
img = x.XGetImage(d, win, 0, 0, w, h, 0xFFFFFFFF, 2)


class XImage(ctypes.Structure):
    _fields_ = [("width", ctypes.c_int), ("height", ctypes.c_int), ("xoffset", ctypes.c_int), ("format", ctypes.c_int),
                ("data", ctypes.POINTER(ctypes.c_ubyte)), ("byte_order", ctypes.c_int), ("bitmap_unit", ctypes.c_int),
                ("bitmap_bit_order", ctypes.c_int), ("bitmap_pad", ctypes.c_int), ("depth", ctypes.c_int),
                ("bytes_per_line", ctypes.c_int), ("bits_per_pixel", ctypes.c_int)]


im = ctypes.cast(img, ctypes.POINTER(XImage)).contents
bpl = im.bytes_per_line
raw = ctypes.string_at(im.data, bpl * h)
rows = []
for y in range(h):
    r = raw[y * bpl:y * bpl + w * 4]
    px = bytearray(w * 3)
    px[0::3] = r[2::4]
    px[1::3] = r[1::4]
    px[2::3] = r[0::4]
    rows.append(b"\0" + bytes(px))


def chunk(kind, data):
    return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data) & 0xFFFFFFFF)


png = b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 2, 0, 0, 0)) \
    + chunk(b"IDAT", zlib.compress(b"".join(rows), 6)) + chunk(b"IEND", b"")
open(out, "wb").write(png)
print(out, w, h)
