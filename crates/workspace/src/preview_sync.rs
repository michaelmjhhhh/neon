//! Keep the preview beside the editor on the text the editor shows.
//!
//! Both panes are matched by a source line at a height in the viewport: the
//! editor names the line at that height, and the preview scrolls so the
//! same line sits at the same height. The height moves with progress through
//! the document, from the viewport's top at the start to its bottom at the
//! end, so both panes show the start together and the end together, and in
//! between the line half way down the editor sits half way down the preview.
//! Lines are counted in source lines, which both panes share; pixels differ,
//! as a heading or an image is taller in the preview than in the source.

use std::iter::Peekable;

use gpui_kit::component::input::{EditorState, RopeExt as _};
use gpui_kit::{Bounds, Pixels, Point, px};
use markdown_preview::SourceAnchor;

use crate::scroll_anchor::laid_out_rows;

/// What the scroll anchor is computed from. While it stays the same, so does
/// the anchor, and a paint for a caret blink skips the work.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct LayoutKey {
    scroll: Point<Pixels>,
    viewport: Bounds<Pixels>,
    text: Option<Bounds<Pixels>>,
    len: usize,
    lines: usize,
}

impl LayoutKey {
    pub(crate) fn of(editor: &EditorState) -> Self {
        Self {
            scroll: editor.scroll_offset(),
            viewport: editor.input_bounds(),
            text: editor.text_bounds(),
            len: editor.text().len(),
            lines: editor.text().lines_len(),
        }
    }
}

/// The line to match while the editor scrolls. `None` before the editor's
/// first layout.
pub(crate) fn scroll_anchor(editor: &EditorState) -> Option<SourceAnchor> {
    let viewport = editor.input_bounds();
    let viewport_top = viewport.top() - editor.text_bounds()?.top();
    let lines = editor.text().lines_len();
    anchor_in_rows(
        laid_out_rows(editor),
        viewport_top,
        viewport.size.height,
        lines,
    )
}

/// The line holding `offset`, where the editor shows it, to bring the
/// preview's text beside it. `None` when the line is not laid out.
pub(crate) fn line_anchor(editor: &EditorState, offset: usize) -> Option<SourceAnchor> {
    let viewport_top = editor.input_bounds().top() - editor.text_bounds()?.top();
    let row = editor.text().offset_to_point(offset).row;
    let (_, top, _) = laid_out_rows(editor).find(|&(laid_out, _, _)| laid_out == row)?;
    Some(SourceAnchor {
        line: row as f32,
        y: top - viewport_top,
    })
}

/// The anchor among laid-out `rows`, each a line with the top and bottom of
/// its wrapped rows in content coordinates, for a viewport `height` tall
/// whose top is at `viewport_top`, in a text of `lines` lines. Walks the
/// rows once: the anchor is never above the viewport's top.
fn anchor_in_rows(
    rows: impl Iterator<Item = (usize, Pixels, Pixels)>,
    viewport_top: Pixels,
    height: Pixels,
    lines: usize,
) -> Option<SourceAnchor> {
    let mut rows = RowCursor {
        rows: rows.peekable(),
        last: None,
    };
    let progress = (rows.line_at(viewport_top)? / lines.max(1) as f32).clamp(0., 1.);
    let y = height * progress;
    Some(SourceAnchor {
        line: rows.line_at(viewport_top + y)?,
        y,
    })
}

/// Laid-out rows read top to bottom, for lookups at increasing heights.
struct RowCursor<I: Iterator<Item = (usize, Pixels, Pixels)>> {
    rows: Peekable<I>,
    /// The last row passed.
    last: Option<usize>,
}

impl<I: Iterator<Item = (usize, Pixels, Pixels)>> RowCursor<I> {
    /// The line at content height `y`, with the fraction of the way through
    /// it; above the first row, its start; below the last, the line after
    /// it. `None` with no rows.
    fn line_at(&mut self, y: Pixels) -> Option<f32> {
        while let Some(&(row, top, bottom)) = self.rows.peek() {
            if y < bottom {
                let fraction = (y - top) / (bottom - top).max(px(1.));
                return Some(row as f32 + fraction.clamp(0., 1.));
            }
            self.last = Some(row);
            self.rows.next();
        }
        self.last.map(|row| (row + 1) as f32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `count` unwrapped lines of 20px from the text's top, with lines
    /// `first..` laid out.
    fn rows(first: usize, count: usize) -> impl Iterator<Item = (usize, Pixels, Pixels)> {
        (first..first + count).map(|row| (row, px(20. * row as f32), px(20. * (row + 1) as f32)))
    }

    #[test]
    fn at_the_top_the_anchor_is_the_first_line_at_the_viewport_top() {
        let anchor = anchor_in_rows(rows(0, 30), px(0.), px(400.), 100).unwrap();
        assert_eq!(
            anchor,
            SourceAnchor {
                line: 0.,
                y: px(0.)
            }
        );
    }

    #[test]
    fn padding_above_the_first_line_counts_as_its_start() {
        let anchor = anchor_in_rows(rows(0, 30), px(-8.), px(400.), 100).unwrap();
        assert_eq!(
            anchor,
            SourceAnchor {
                line: 0.,
                y: px(0.)
            }
        );
    }

    #[test]
    fn the_anchor_moves_down_the_viewport_with_progress() {
        // Line 50 of 100 at the top: half way, so the anchor sits half way
        // down, 10 lines further on.
        let anchor = anchor_in_rows(rows(48, 30), px(1000.), px(400.), 100).unwrap();
        assert_eq!(
            anchor,
            SourceAnchor {
                line: 60.,
                y: px(200.)
            }
        );
    }

    #[test]
    fn fractions_follow_the_pixels_through_a_line() {
        let anchor = anchor_in_rows(rows(10, 30), px(205.), px(400.), 1000).unwrap();
        let expected = 10. + 0.25 + anchor.y / px(20.);
        assert!((anchor.line - expected).abs() < 1e-4, "{anchor:?}");
    }

    #[test]
    fn below_the_last_line_the_anchor_is_the_text_end() {
        // Scrolled past the last of 40 lines, into the space after it.
        let anchor = anchor_in_rows(rows(30, 10), px(780.), px(400.), 40).unwrap();
        assert_eq!(anchor.line, 40.);
    }

    #[test]
    fn no_rows_no_anchor() {
        assert_eq!(anchor_in_rows(rows(0, 0), px(0.), px(400.), 1), None);
    }
}
