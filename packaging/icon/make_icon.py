"""Draws the FinderMaxxing icon (a folder with a spark) at any size, as PNG."""
import math, struct, sys, zlib

def png(path, size, pixels):
    raw = b"".join(b"\x00" + bytes(pixels[y * size * 4:(y + 1) * size * 4]) for y in range(size))
    def chunk(kind, data):
        return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data) & 0xFFFFFFFF)
    with open(path, "wb") as f:
        f.write(b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", size, size, 8, 6, 0, 0, 0))
                + chunk(b"IDAT", zlib.compress(raw, 9)) + chunk(b"IEND", b""))

def rrect(x, y, cx, cy, hw, hh, r):
    qx, qy = abs(x - cx) - hw + r, abs(y - cy) - hh + r
    return math.hypot(max(qx, 0), max(qy, 0)) + min(max(qx, qy), 0) - r

def cover(d, px):  # signed distance (in units) -> coverage over one pixel
    return min(1.0, max(0.0, 0.5 - d / px))

def star(x, y, cx, cy, r):  # 4-point spark: astroid-like, supersampled
    u, v = abs(x - cx) / r, abs(y - cy) / r
    return 1.0 if u ** 0.5 + v ** 0.5 <= 1.0 else 0.0

def over(dst, src, a):
    return tuple(s * a + d * (1 - a) for s, d in zip(src, dst))

def draw(size):
    px = 1.0 / size
    out = bytearray(size * size * 4)
    top, bottom = (0.33, 0.55, 1.0), (0.42, 0.22, 0.86)
    for j in range(size):
        y = (j + 0.5) * px
        bg = tuple(t + (b - t) * y for t, b in zip(top, bottom))
        for i in range(size):
            x = (i + 0.5) * px
            # macOS grid: the plate is 824/1024 of the canvas, radius ~ 185/1024.
            a = cover(rrect(x, y, 0.5, 0.5, 0.4023, 0.4023, 0.18), px)
            if a == 0:
                continue
            c = bg
            shade = cover(rrect(x, y + 0.012, 0.5, 0.56, 0.27, 0.19, 0.04), px) * 0.25
            c = over(c, (0.1, 0.05, 0.3), shade)
            tab = cover(rrect(x, y, 0.36, 0.335, 0.13, 0.06, 0.035), px)
            c = over(c, (0.86, 0.9, 1.0), tab)
            body = cover(rrect(x, y, 0.5, 0.55, 0.27, 0.19, 0.04), px)
            c = over(c, (1.0, 1.0, 1.0), body)
            if 0.52 < x < 0.72 and 0.47 < y < 0.67:
                n = 3 if size >= 64 else 4
                s = sum(star(x + (a1 - (n - 1) / 2) * px / n, y + (b1 - (n - 1) / 2) * px / n, 0.62, 0.57, 0.09)
                        for a1 in range(n) for b1 in range(n)) / (n * n)
                c = over(c, (0.45, 0.3, 0.95), s)
            k = (j * size + i) * 4
            out[k:k + 4] = bytes([round(c[0] * 255), round(c[1] * 255), round(c[2] * 255), round(a * 255)])
    return out

if __name__ == "__main__":
    size, path = int(sys.argv[1]), sys.argv[2]
    png(path, size, draw(size))
