"""Render Rustroom's icon from its master: every PNG size, and the Windows .ico.

    python packaging/icons/render.py        (needs `resvg` on PATH: `cargo install resvg`)

The rasters are committed, so a build never needs this script — run it after changing `rustroom.svg`.
macOS's .icns is made from the PNGs on the Mac (piece 3b, `iconutil`). Standard library only.
"""

import struct
import subprocess
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
MASTER = HERE / "rustroom.svg"
PNG = HERE / "png"
SIZES = [16, 24, 32, 48, 64, 128, 256, 512, 1024]
# What Windows asks an .ico for: the small sizes for lists and the title bar, 256 for large icon views.
ICO_SIZES = [16, 24, 32, 48, 64, 128, 256]


def render() -> None:
    PNG.mkdir(exist_ok=True)
    for n in SIZES:
        out = PNG / f"rustroom-{n}.png"
        subprocess.run(["resvg", "-w", str(n), "-h", str(n), str(MASTER), str(out)], check=True)


def ico() -> None:
    """An .ico whose images are PNGs (Windows Vista and later read them at every size)."""
    images = [(n, (PNG / f"rustroom-{n}.png").read_bytes()) for n in ICO_SIZES]
    header = struct.pack("<HHH", 0, 1, len(images))
    offset = len(header) + 16 * len(images)
    entries, data = b"", b""
    for n, png in images:
        side = 0 if n == 256 else n  # 0 means 256 in an ICONDIRENTRY
        entries += struct.pack("<BBBBHHII", side, side, 0, 0, 1, 32, len(png), offset + len(data))
        data += png
    (HERE / "rustroom.ico").write_bytes(header + entries + data)


if __name__ == "__main__":
    render()
    ico()
    print(f"wrote {len(SIZES)} PNGs and rustroom.ico", file=sys.stderr)
