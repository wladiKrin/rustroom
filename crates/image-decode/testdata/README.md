# HEIF fixtures (M10.19a) — all CONSTRUCTED, none from a camera or a phone

Made 2026-11-11 with libheif 1.23.7's `heif-enc` and ImageMagick. They prove self-consistency and libheif's
handling of what they declare; ⚠ a real iPhone or camera file may carry what they do not (HDR gain maps,
grids, auxiliary images).

| File | Made from | Declares |
|---|---|---|
| `srgb.heic` | a 64 × 48 JPEG, left half (255, 0, 0), right half (128, 128, 128), with the EXIF `mkexif.py` writes (Apple, iPhone 15 Pro, f/1.8, 2026:07:14 18:30:05) — `heif-enc -q 95 --colour_primaries 1 --transfer_characteristic 13 --matrix_coefficients 6` | nclx sRGB/709, EXIF |
| `p3.heic` | an 8 × 8 PNG of (0, 200, 0) — `heif-enc -L --colour_primaries 12 --transfer_characteristic 13` | nclx Display P3, lossless RGB |
| `icc.heic` | the same PNG with Rustroom's own Display P3 ICC (`export::icc_for(DisplayP3)`) embedded — `heif-enc -L` | an ICC profile (`prof`) |
| `ten.heic` | an 8 × 8 16-bit PNG at 50 % grey — `heif-enc -L -b 10` | 10 bits, monochrome |
| `rot.heic` | a 60 × 40 JPEG with a red stripe down its left 10 columns and EXIF orientation 6 — `heif-enc -q 95` | the container's rotation (`irot` 270° ccw), shown 40 × 60 |
