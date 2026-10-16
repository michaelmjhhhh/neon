"""Render the Neon app icon.

The mark is one "N" path drawn three times as parallel offsets, brightest on
the outside, like a folded neon tube seen as line art. It sits on a dark plate
in Apple's macOS icon grid (an 824 px plate on a 1024 px canvas). Sizes of
32 px and below use a single thick stroke, because three thin lines merge
into a smudge at that scale.

Usage: python3 assets/icon/generate.py
Writes assets/icon/neon.png and assets/icon/Neon.icns (needs macOS iconutil).
"""

import math
import subprocess
import tempfile
from pathlib import Path

from PIL import Image, ImageChops, ImageDraw, ImageFilter

HERE = Path(__file__).resolve().parent
SUPERSAMPLE = 4
CANVAS = 1024
PLATE = 824

# The mark is defined in its own design space, where the three lines are
# 16 units wide at a 30-unit pitch, then scaled to fit the plate.
DESIGN_SCALE = PLATE / 905
DESIGN_CENTER = (512.0, 509.0)
CENTERLINE = [(283.0, 771.0), (283.0, 309.6), (741.0, 707.8), (741.0, 247.0)]
STROKE = 16.0
PITCH = 30.0
# Outermost (left of travel) to innermost.
LINE_COLORS = [(254, 88, 30), (209, 68, 25), (170, 52, 21)]
SMALL_STROKE = 2 * PITCH + STROKE + 10

PLATE_TOP = (29, 30, 35)
PLATE_BOTTOM = (14, 14, 17)
GRID_COLOR = (36, 38, 45)
GRID_BOX = (232.0, 222.0, 792.0, 798.0)
GRID_CELLS = 12


def to_canvas(point, size):
    """Map a design-space point to pixel space for a canvas of `size`."""
    k = DESIGN_SCALE * size / CANVAS
    x, y = point
    return (
        size / 2 + (x - DESIGN_CENTER[0]) * k,
        size / 2 + (y - DESIGN_CENTER[1]) * k,
    )


def offset_polyline(points, distance):
    """Offset an open polyline to the left of travel, with mitred joins."""

    def normal(a, b):
        dx, dy = b[0] - a[0], b[1] - a[1]
        length = math.hypot(dx, dy)
        # Left of travel in y-down screen space.
        return dy / length, -dx / length

    segments = []
    for a, b in zip(points, points[1:]):
        nx, ny = normal(a, b)
        segments.append(
            ((a[0] + nx * distance, a[1] + ny * distance),
             (b[0] + nx * distance, b[1] + ny * distance))
        )

    def intersect(s1, s2):
        (x1, y1), (x2, y2) = s1
        (x3, y3), (x4, y4) = s2
        denom = (x1 - x2) * (y3 - y4) - (y1 - y2) * (x3 - x4)
        det1 = x1 * y2 - y1 * x2
        det2 = x3 * y4 - y3 * x4
        return (
            (det1 * (x3 - x4) - (x1 - x2) * det2) / denom,
            (det1 * (y3 - y4) - (y1 - y2) * det2) / denom,
        )

    result = [segments[0][0]]
    for s1, s2 in zip(segments, segments[1:]):
        result.append(intersect(s1, s2))
    result.append(segments[-1][1])
    return result


def stroke_polygon(offset, width):
    """The outline of one stroke whose centerline is offset from CENTERLINE.

    Both ends are vertical segments, so offsetting keeps the end caps flat and
    horizontal.
    """
    left = offset_polyline(CENTERLINE, offset + width / 2)
    right = offset_polyline(CENTERLINE, offset - width / 2)
    return left + right[::-1]


def plate_outline(size, inset, steps_per_corner=64):
    """macOS icon plate: a rounded square with corners at 22.37% of its side."""
    side = size - 2 * inset
    radius = side * 0.2237
    lo, hi = inset + radius, size - inset - radius
    corners = [(hi, hi, 0.0), (lo, hi, 90.0), (lo, lo, 180.0), (hi, lo, 270.0)]
    points = []
    for cx, cy, start in corners:
        for i in range(steps_per_corner + 1):
            t = math.radians(start + 90.0 * i / steps_per_corner)
            points.append((cx + radius * math.cos(t), cy + radius * math.sin(t)))
    return points


def vertical_gradient(size, top, bottom, y0, y1):
    gradient = Image.new("RGB", (1, size))
    for y in range(size):
        t = min(max((y - y0) / (y1 - y0), 0.0), 1.0)
        gradient.putpixel(
            (0, y), tuple(round(a + (b - a) * t) for a, b in zip(top, bottom))
        )
    return gradient.resize((size, size))


def render(size, simplified):
    s = size * SUPERSAMPLE
    inset = (CANVAS - PLATE) / 2 * s / CANVAS
    plate_shape = plate_outline(s, inset)

    canvas = Image.new("RGBA", (s, s), (0, 0, 0, 0))

    # Soft shadow under the plate.
    shadow = Image.new("L", (s, s), 0)
    ImageDraw.Draw(shadow).polygon(
        [(x, y + s * 0.012) for x, y in plate_shape], fill=110
    )
    shadow = shadow.filter(ImageFilter.GaussianBlur(s * 0.018))
    canvas.paste((0, 0, 0, 255), (0, 0), shadow)

    # The plate itself.
    mask = Image.new("L", (s, s), 0)
    ImageDraw.Draw(mask).polygon(plate_shape, fill=255)
    plate = vertical_gradient(s, PLATE_TOP, PLATE_BOTTOM, inset, s - inset)
    plate = plate.convert("RGBA")
    draw = ImageDraw.Draw(plate)

    if not simplified:
        # Blueprint grid behind the mark.
        x0, y0, x1, y1 = GRID_BOX
        line = max(1, round(1.5 * s / CANVAS * DESIGN_SCALE))
        for i in range(GRID_CELLS + 1):
            x = x0 + (x1 - x0) * i / GRID_CELLS
            y = y0 + (y1 - y0) * i / GRID_CELLS
            draw.line([to_canvas((x, y0), s), to_canvas((x, y1), s)],
                      fill=GRID_COLOR, width=line)
            draw.line([to_canvas((x0, y), s), to_canvas((x1, y), s)],
                      fill=GRID_COLOR, width=line)

    if simplified:
        polygon = stroke_polygon(0.0, SMALL_STROKE)
        draw.polygon([to_canvas(p, s) for p in polygon], fill=LINE_COLORS[0])
    else:
        for ix, color in enumerate(LINE_COLORS):
            polygon = stroke_polygon(PITCH * (1 - ix), STROKE)
            draw.polygon([to_canvas(p, s) for p in polygon], fill=color)

    canvas.paste(plate, (0, 0), mask)

    if not simplified:
        # A thin light rim along the top edge, fading out down the sides.
        rim = Image.new("L", (s, s), 0)
        ImageDraw.Draw(rim).polygon(plate_shape, fill=255)
        inner = Image.new("L", (s, s), 0)
        ImageDraw.Draw(inner).polygon(
            [(x, y + s * 0.0025) for x, y in plate_shape], fill=255
        )
        rim = ImageChops.subtract(rim, inner)
        fade = vertical_gradient(s, (90,), (0,), inset, inset + s * 0.25)
        rim = ImageChops.multiply(rim, fade.convert("L"))
        canvas.paste((255, 255, 255, 255), (0, 0), rim)

    # Hairline edge so the plate holds its shape on dark backgrounds.
    edge = Image.new("RGBA", (s, s), (0, 0, 0, 0))
    ImageDraw.Draw(edge).polygon(
        plate_shape, outline=(0, 0, 0, 140), width=max(1, round(s / 1024))
    )
    canvas.alpha_composite(edge)

    return canvas.resize((size, size), Image.LANCZOS)


def main():
    render(CANVAS, simplified=False).save(HERE / "neon.png")

    with tempfile.TemporaryDirectory() as tmp:
        iconset = Path(tmp) / "Neon.iconset"
        iconset.mkdir()
        for points in (16, 32, 128, 256, 512):
            for scale in (1, 2):
                pixels = points * scale
                name = f"icon_{points}x{points}{'@2x' if scale == 2 else ''}.png"
                render(pixels, simplified=pixels <= 32).save(iconset / name)
        subprocess.run(
            ["iconutil", "-c", "icns", str(iconset), "-o", str(HERE / "Neon.icns")],
            check=True,
        )


if __name__ == "__main__":
    main()
