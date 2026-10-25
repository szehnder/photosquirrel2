# Test images

- The HEIC sample (a 1,276-byte `hvc1` primary item plus a thumbnail, made by the project) is the self-test
  sample of `ps-sysdecode`, `crates/ps-sysdecode/data/selftest.heic`; tests here include it from there.

## AVIF fixtures (ADR-0090)

Made once with `avifenc` 1.4.2 (libavif, AOM encoder v3.15.0) from the project's own synthetic pattern
(`avif_pattern.py`: four quadrants, red, green, blue and grey, with a gentle horizontal gradient), so no encoder
is a build or test dependency:

```sh
python3 avif_pattern.py p64.png 64 48
python3 avif_pattern.py p128.png 128 96
avifenc -q 90 -y 420 -d 8  p64.png  tiny.avif           # 8-bit 4:2:0, 64x48
avifenc -q 90 -y 420 -d 10 p64.png  tiny10.avif         # 10-bit 4:2:0, 64x48
avifenc -q 90 -y 420 -d 8 --grid 2x2 p128.png grid.avif # 2x2 grid of 64x48 tiles, 128x96
avifenc -q 90 -y 420 -d 8 --irot 1 p64.png rot.avif     # irot 1 (90 degrees anti-clockwise), stored 64x48
```

`photo.avif` (640x480, 8-bit 4:2:0, for scan tests that need a photo-sized AVIF) is
`ps_fixtures::media::jpeg(640, 480, 2026, 95)` written to `photo.jpg` and encoded with
`avifenc -q 60 -y 420 -d 8 photo.jpg photo.avif`.

All are full range, BT.601 matrix (`colr nclx` 1/13/6).

Reference colours (the centre of each quadrant of the source pattern, before coding; tests allow a tolerance of
24 per channel):

| Quadrant | RGB |
|---|---|
| top left | (200, 40, 40) |
| top right | (40, 180, 60) |
| bottom left | (40, 60, 200) |
| bottom right | (128, 128, 128) |
