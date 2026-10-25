# The project's synthetic test pattern for AVIF fixtures: four flat-ish quadrants (red, green, blue, grey)
# with a gentle horizontal gradient, so tiles and rotation are recognisable after lossy coding.
import struct, zlib, sys
def png(path, w, h, px):
    raw = b''.join(b'\x00' + bytes(px[y*w*3:(y+1)*w*3]) for y in range(h))
    def chunk(t, d): return struct.pack('>I', len(d)) + t + d + struct.pack('>I', zlib.crc32(t + d) & 0xffffffff)
    open(path, 'wb').write(b'\x89PNG\r\n\x1a\n' + chunk(b'IHDR', struct.pack('>IIBBBBB', w, h, 8, 2, 0, 0, 0)) + chunk(b'IDAT', zlib.compress(raw)) + chunk(b'IEND', b''))
QUAD = [(200, 40, 40), (40, 180, 60), (40, 60, 200), (128, 128, 128)]
def pattern(w, h):
    px = []
    for y in range(h):
        for x in range(w):
            q = (1 if x >= w // 2 else 0) + (2 if y >= h // 2 else 0)
            r, g, b = QUAD[q]
            d = (x * 20) // w - 10
            px += [max(0, min(255, c + d)) for c in (r, g, b)]
    return px
w, h = int(sys.argv[2]), int(sys.argv[3])
png(sys.argv[1], w, h, pattern(w, h))
