#!/usr/bin/env python3
"""Builds Lunge's logo files from the supplied originals, with the
"oversized L" mark.

The supplied mark (assets/brand/original/) is an L beside a cloud with a
keyhole, plus three slanted pieces to the left of and below the L. The
oversized-L mark drops those pieces and lets the L fill their space:

  1. The L is scaled up from the top-right corner of its stroke. That
     corner is on the stroke's inner edge, so the gap between the stroke
     and the cloud doesn't change, and the L's lower-left grows into the
     pieces' old footprint.
  2. The cloud slides down along the stroke by as much as the L's inner
     corner moved, so it sits in the larger crook of the L.
  3. The L's longer foot is trimmed with a cut parallel to the cloud's
     lower-left edge, at the original gap.
  4. The new mark is shifted right so it's centred where the old one was.

This works on the raster originals, so it's a stand-in until the mark is
redrawn as a vector; regenerate from here rather than editing outputs.

Run from the repo root (needs Pillow):

    python3 assets/brand/make_logos.py

Writes:
  assets/brand/logo.png                          white logo on brand navy
  assets/brand/logo_transparent_{black,white,notext}.png
  assets/brand/readme/logo-on-{light,dark}.png   README, edge-cleaned
  crates/lunge-gui/assets/logo-toolbar.png       toolbar: no tagline, 128px
and then runs make_icon.py for the app icons.
"""

from collections import deque
from pathlib import Path

from PIL import Image, ImageChops, ImageFilter

import make_icon

ROOT = Path(__file__).resolve().parents[2]
BRAND = ROOT / "assets/brand"
ORIGINAL = BRAND / "original"
NAVY = (0x14, 0x1B, 0x35)

# Geometry of the supplied mark, in the 1912x633 originals' pixels.
CORNER = (442, 28)  # top-right corner of the L's stroke
INNER = (241, 392)  # where the stroke's inner edge meets the foot's top
SCALE = 1.23  # the L's bottom-left then reaches the pieces' old bottom
CLOUD_EDGE = (430, 396)  # a point on the cloud's lower-left edge
CLOUD_SLOPE = -0.54  # that edge's x change per pixel down
GAP = 29  # horizontal gap between the foot's end and that edge
MARK_RIGHT = 760  # everything left of this is mark, right of it is text
RECENTER = 28  # shift that centres the new mark on the old one
ALPHA_MIN = 40  # alpha above this counts as part of a shape

# The README copies are the transparent versions cropped to their artwork
# (left/top at 29, 26) and scaled to 1200px wide.
README_SCALE = 1200 / 1842
README_ORIGIN = (-29 * README_SCALE, -26 * README_SCALE)

# The tagline ("by High Trail Studios"), left out of the toolbar logo.
TAGLINE = (835, 440, 1450, 522)
TOOLBAR_HEIGHT = 128


def shapes(alpha: Image.Image, right: int) -> list[tuple[int, set]]:
    """The separate opaque shapes left of x = right, largest first, as
    (pixel count, pixels)."""
    w, h = alpha.size
    a = alpha.load()
    seen = set()
    found = []
    for y in range(h):
        for x in range(min(right, w)):
            if a[x, y] <= ALPHA_MIN or (x, y) in seen:
                continue
            pixels = set()
            queue = deque([(x, y)])
            seen.add((x, y))
            while queue:
                cx, cy = queue.popleft()
                pixels.add((cx, cy))
                for nx, ny in ((cx + 1, cy), (cx - 1, cy), (cx, cy + 1), (cx, cy - 1)):
                    if (
                        0 <= nx < right
                        and 0 <= ny < h
                        and (nx, ny) not in seen
                        and a[nx, ny] > ALPHA_MIN
                    ):
                        seen.add((nx, ny))
                        queue.append((nx, ny))
            found.append((len(pixels), pixels))
    return sorted(found, key=lambda f: -f[0])


def layer(image: Image.Image, pixels: set, grow: int) -> Image.Image:
    """`image` with only `pixels` (plus `grow` px of their soft edge) kept."""
    mask = Image.new("L", image.size, 0)
    m = mask.load()
    for p in pixels:
        m[p] = 255
    mask = mask.filter(ImageFilter.MaxFilter(grow))
    out = image.copy()
    out.putalpha(ImageChops.multiply(image.getchannel("A"), mask))
    return out


def oversize(image: Image.Image, s: float = 1.0, origin=(0.0, 0.0)) -> Image.Image:
    """`image` with its mark replaced by the oversized-L mark. The mark's
    original pixel (x, y) is at origin + s * (x, y) in `image`."""
    image = image.convert("RGBA")
    w, h = image.size
    ox, oy = origin

    def at(x: float, y: float) -> tuple[float, float]:
        return ox + s * x, oy + s * y

    right = round(ox + s * MARK_RIGHT)
    found = shapes(image.getchannel("A"), right)
    if len(found) < 2:
        raise SystemExit("couldn't find the L and the cloud in the logo")
    grow = max(3, round(7 * s) | 1)
    cloud = layer(image, found[0][1], grow)
    ell = layer(image, found[1][1], grow)

    # 1. Scale the L about the stroke's top-right corner.
    cx, cy = at(*CORNER)
    box = ell.getchannel("A").getbbox()
    piece = ell.crop(box)
    piece = piece.resize(
        (round(piece.width * SCALE), round(piece.height * SCALE)), Image.LANCZOS
    )
    new_l = Image.new("RGBA", (w, h), (0, 0, 0, 0))
    new_l.alpha_composite(
        piece, (round(cx + (box[0] - cx) * SCALE), round(cy + (box[1] - cy) * SCALE))
    )

    # 2. Move the cloud with the L's inner corner.
    dx = (INNER[0] - CORNER[0]) * (SCALE - 1)
    dy = (INNER[1] - CORNER[1]) * (SCALE - 1)
    new_cloud = Image.new("RGBA", (w, h), (0, 0, 0, 0))
    new_cloud.alpha_composite(cloud, (round(s * dx), round(s * dy)))

    # 3. Trim the foot parallel to the moved cloud's lower-left edge.
    foot_top = at(0, CORNER[1] + (INNER[1] - CORNER[1]) * SCALE - 6)[1]
    keep = Image.new("L", (w, h), 255)
    k = keep.load()
    for y in range(max(0, int(foot_top)), h):
        y0 = (y - oy) / s - dy  # this row in the unmoved cloud's coordinates
        edge = ox + s * (CLOUD_EDGE[0] + dx + CLOUD_SLOPE * (y0 - CLOUD_EDGE[1]) - GAP)
        for x in range(max(0, int(edge) - 1), w):
            k[x, y] = round(255 * min(1.0, max(0.0, edge - x + 0.5)))
    new_l.putalpha(ImageChops.multiply(new_l.getchannel("A"), keep))

    # 4. Clear the old mark and draw the new one, re-centred.
    mark = Image.new("RGBA", (w, h), (0, 0, 0, 0))
    mark.alpha_composite(new_l)
    mark.alpha_composite(new_cloud)
    out = image.copy()
    clear = Image.new("L", (w, h), 255)
    clear.paste(0, (0, 0, right, h))
    out.putalpha(ImageChops.multiply(out.getchannel("A"), clear))
    out.alpha_composite(mark, (round(s * RECENTER), 0))
    return out


def on_navy(image: Image.Image) -> Image.Image:
    out = Image.new("RGBA", image.size, NAVY + (255,))
    out.alpha_composite(image)
    return out


def toolbar(white: Image.Image) -> Image.Image:
    """The toolbar logo: mark and wordmark without the tagline, on navy."""
    art = white.copy()
    art.paste((0, 0, 0, 0), TAGLINE)
    art = art.crop(art.getchannel("A").getbbox())
    pad = round(art.height * 0.02)
    framed = Image.new("RGBA", (art.width + 2 * pad, art.height + 2 * pad), NAVY + (255,))
    framed.alpha_composite(art, (pad, pad))
    width = round(framed.width * TOOLBAR_HEIGHT / framed.height)
    return framed.resize((width, TOOLBAR_HEIGHT), Image.LANCZOS).convert("RGB")


def main() -> None:
    for name in ["black", "white", "notext"]:
        src = ORIGINAL / f"logo_transparent_{name}.png"
        oversize(Image.open(src)).save(BRAND / src.name, optimize=True)
    white = Image.open(BRAND / "logo_transparent_white.png").convert("RGBA")
    on_navy(white).save(BRAND / "logo.png", optimize=True)

    (BRAND / "readme").mkdir(exist_ok=True)
    for name in ["logo-on-light.png", "logo-on-dark.png"]:
        src = Image.open(ORIGINAL / "readme" / name)
        oversize(src, README_SCALE, README_ORIGIN).save(
            BRAND / "readme" / name, optimize=True
        )

    toolbar(white).save(ROOT / "crates/lunge-gui/assets/logo-toolbar.png", optimize=True)
    make_icon.main()


if __name__ == "__main__":
    main()
