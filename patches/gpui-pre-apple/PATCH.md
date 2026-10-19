# gpui-pre-apple 0.3.9, patched

This is the crates.io release with one change in `src/text_system.rs`: it
synthesizes bold and italic when a face lacks them.

## Problem

GPUI turns a bold or italic request into one of the faces the font family
ships. If the family has no such face, it uses the regular face and draws
nothing extra. Examples:

- `Hei`, `Kai`, `STSong` and other single-face fonts: bold and italic both
  draw as regular.
- Every CJK font, including the PingFang fallback under the system font: no
  italic face, so `*中文*` draws upright.

## Change

- When a face lacks the requested style, `font_id` returns a separate
  `FontId` that shares the face's glyphs and metrics, so shaping is
  unchanged. It does the same for a fallback face that CoreText picks for
  glyphs the requested font lacks.
- The rasterizer draws that id with a 14 degree shear for oblique (as WebKit
  does) and with fill plus a thin stroke for bold (Skia's fake-bold width).
  The glyph bounds grow to fit.
- Bold is synthesized only when the face is at most weight 500, so a real
  semibold or bold face is never thickened again.

Drop this patch when GPUI synthesizes missing faces itself.
