#!/usr/bin/env python3
"""Regenerate Steward icon assets.

Produces:
- assets/steward-dark.png  : the dark tray glyph on a rounded launcher tile
- assets/icon-dark.ico     : multi-size ICO of the dark icon (tray resource 2)

The light/navy glyph (assets/steward.png) is recolored with the launcher's
accent blue (#89b4fa) and placed on the launcher background tile (#232332), so
the tray icon matches the main window's dark theme.
"""

from pathlib import Path

from PIL import Image, ImageDraw

ROOT = Path(__file__).resolve().parent.parent
SRC = ROOT / "assets" / "steward.png"
DARK_PNG = ROOT / "assets" / "steward-dark.png"
DARK_ICO = ROOT / "assets" / "icon-dark.ico"

TILE = (35, 35, 50, 255)        # #232332 — launcher background
TILE_EDGE = (49, 50, 68, 255)   # #313244 — hover surface
GLYPH = (137, 180, 250, 255)    # #89b4fa — launcher accent

SIZE = 1254
TILE_RATIO = 0.92
TILE_RADIUS = 0.14
GLYPH_RATIO = 0.66


def main() -> None:
    glyph = Image.open(SRC).convert("RGBA")
    assert glyph.size == (SIZE, SIZE), f"expected {SIZE}x{SIZE} source, got {glyph.size}"

    canvas = Image.new("RGBA", (SIZE, SIZE), (0, 0, 0, 0))
    draw = ImageDraw.Draw(canvas)
    tile = int(SIZE * TILE_RATIO)
    radius = int(SIZE * TILE_RADIUS)
    offset = (SIZE - tile) // 2
    draw.rounded_rectangle(
        [offset, offset, offset + tile, offset + tile],
        radius=radius,
        fill=TILE,
        outline=TILE_EDGE,
        width=max(2, SIZE // 256),
    )

    glyph_size = int(SIZE * GLYPH_RATIO)
    glyph_small = glyph.resize((glyph_size, glyph_size), Image.LANCZOS)
    solid = Image.new("RGBA", glyph_small.size, GLYPH)
    solid.putalpha(glyph_small.getchannel("A"))
    canvas.alpha_composite(solid, ((SIZE - glyph_size) // 2, (SIZE - glyph_size) // 2))

    canvas.save(DARK_PNG, format="PNG")
    canvas.save(
        DARK_ICO,
        format="ICO",
        sizes=[(16, 16), (24, 24), (32, 32), (48, 48), (64, 64), (128, 128), (256, 256)],
    )
    print(f"wrote {DARK_PNG} ({DARK_PNG.stat().st_size} bytes)")
    print(f"wrote {DARK_ICO} ({DARK_ICO.stat().st_size} bytes)")


if __name__ == "__main__":
    main()
