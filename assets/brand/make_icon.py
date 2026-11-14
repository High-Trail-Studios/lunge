#!/usr/bin/env python3
"""Builds Lunge's app icon from the text-free logo.

The icon is the logo centred on a brand-navy rounded square, laid out on
Apple's icon grid (an 824px tile with a soft shadow on a 1024px canvas), so
it sits beside other apps in the macOS Dock. macOS shows a runtime icon
as-is, without masking it, so the rounded square is drawn here. Linux uses
the same picture.

Run from the repo root after changing the logo (needs Pillow):

    python3 assets/brand/make_icon.py

Writes:
  assets/brand/icon-1024.png                 master, for a future .app/.icns
  crates/lunge-gui/assets/icon.png           512px, built into the binary
  crates/lunge-gui/assets/icon-256.png       256px, for Linux packages
"""

from pathlib import Path

from PIL import Image, ImageChops, ImageDraw, ImageFilter

ROOT = Path(__file__).resolve().parents[2]
LOGO = ROOT / "assets/brand/logo_transparent_notext.png"
NAVY = (0x14, 0x1B, 0x35)

CANVAS = 1024
TILE = 824  # Apple's macOS icon grid: 824px tile, 100px margin.
TILE_ORIGIN = (CANVAS - TILE) // 2
LOGO_BOX = 600  # The logo's longer side, inside the tile.
SUPERSAMPLE = 4


def tile_mask(size: int) -> Image.Image:
    """Rounded square with Apple's corner radius (185 of 824), antialiased
    by drawing large and scaling down."""
    big = size * SUPERSAMPLE
    mask = Image.new("L", (big, big), 0)
    ImageDraw.Draw(mask).rounded_rectangle(
        (0, 0, big - 1, big - 1), radius=round(big * 185 / 824), fill=255
    )
    return mask.resize((size, size), Image.LANCZOS)


def build() -> Image.Image:
    canvas = Image.new("RGBA", (CANVAS, CANVAS), (0, 0, 0, 0))
    mask = tile_mask(TILE)

    # Soft shadow under the tile, as in Apple's template.
    shadow_alpha = Image.new("L", (CANVAS, CANVAS), 0)
    shadow_alpha.paste(mask, (TILE_ORIGIN, TILE_ORIGIN + 10))
    shadow_alpha = shadow_alpha.filter(ImageFilter.GaussianBlur(14))
    shadow_alpha = shadow_alpha.point(lambda a: a * 0.35)
    shadow = Image.new("RGBA", (CANVAS, CANVAS), (0, 0, 0, 255))
    shadow.putalpha(shadow_alpha)
    canvas.alpha_composite(shadow)

    # Navy tile with a slight top-to-bottom lift so it doesn't read flat.
    tile = Image.new("RGBA", (TILE, TILE))
    top = tuple(min(255, c + 14) for c in NAVY)
    draw = ImageDraw.Draw(tile)
    for y in range(TILE):
        f = y / (TILE - 1)
        draw.line(
            [(0, y), (TILE, y)],
            fill=tuple(round(top[i] + (NAVY[i] - top[i]) * f) for i in range(3)),
        )

    logo = Image.open(LOGO).convert("RGBA")
    logo = logo.crop(logo.getchannel("A").getbbox())
    scale = LOGO_BOX / max(logo.size)
    logo = logo.resize(
        (round(logo.width * scale), round(logo.height * scale)), Image.LANCZOS
    )
    tile.alpha_composite(
        logo, ((TILE - logo.width) // 2, (TILE - logo.height) // 2)
    )
    tile.putalpha(ImageChops.multiply(tile.getchannel("A"), mask))
    canvas.alpha_composite(tile, (TILE_ORIGIN, TILE_ORIGIN))
    return canvas


def main() -> None:
    icon = build()
    icon.save(ROOT / "assets/brand/icon-1024.png", optimize=True)
    for size, name in [(512, "icon.png"), (256, "icon-256.png")]:
        icon.resize((size, size), Image.LANCZOS).save(
            ROOT / "crates/lunge-gui/assets" / name, optimize=True
        )


if __name__ == "__main__":
    main()
