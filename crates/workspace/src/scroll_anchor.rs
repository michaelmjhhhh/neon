//! Keep the editor on the same line when its text size or font changes.
//!
//! The editor scrolls by a pixel offset. A size change re-wraps every line,
//! so the same offset lands elsewhere: three steps up from line 3,727 of a
//! 7,500-line document showed line 2,006. The editor exposes positions only
//! for the rows it laid out, so the line is found again over the next frames:
//! the first frame scales the offset by the size change, then each frame
//! after a layout moves the line back to where it was.

use gpui_kit::component::input::{EditorState, RopeExt as _};
use gpui_kit::{App, Entity, Pixels, point, px};

/// Correction frames before leaving the view where it is.
const MAX_STEPS: u8 = 4;

/// Correction frames for a jump to a far row, as to a heading picked in the
/// outline. Each frame lays out rows nearer the target, so the estimate
/// tightens; a jump across a long document of wrapped lines may need more
/// than a size change does.
const JUMP_STEPS: u8 = 8;

/// The line at the top of the viewport before a typography change.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ScrollAnchor {
    row: usize,
    /// How far the row's top sat below the viewport's top, in line heights;
    /// negative when the row was partly scrolled out.
    offset: f32,
    /// The viewport's top in content coordinates, and the text size it was
    /// measured at, for the estimate before the next layout.
    viewport_top: Pixels,
    size: f32,
    steps_left: u8,
}

impl ScrollAnchor {
    /// The first row at least partly in view, from the editor's last layout,
    /// which used text size `size`. `None` before the first layout.
    pub(crate) fn capture(editor: &EditorState, size: f32) -> Option<Self> {
        let line_height = editor.line_height()?;
        let viewport_top = editor.input_bounds().top() - editor.text_bounds()?.top();
        let (row, top, _) = laid_out_rows(editor).find(|&(_, _, bottom)| bottom > viewport_top)?;
        Some(Self {
            row,
            offset: (top - viewport_top) / line_height,
            viewport_top,
            size,
            steps_left: MAX_STEPS,
        })
    }

    /// Bring `row` to `lines` line heights below the viewport's top over the
    /// next layouts at text size `size`, as for a jump to a heading.
    pub(crate) fn to_row(row: usize, lines: f32, size: f32) -> Self {
        Self {
            row,
            offset: lines,
            // Only an estimate before a layout reads these; a size change
            // meanwhile starts from the top and corrects from there.
            viewport_top: px(0.),
            size,
            steps_left: JUMP_STEPS,
        }
    }

    /// Scroll so the next layout, at text size `size`, starts near the row,
    /// scaling its place in the last layout, at `laid_out_size`: exact when
    /// no line wraps differently. Returns the anchor with its correction
    /// frames renewed for the change.
    pub(crate) fn estimate(
        self,
        editor: &Entity<EditorState>,
        laid_out_size: f32,
        size: f32,
        cx: &mut App,
    ) -> Self {
        let state = editor.read(cx);
        let laid_out = state
            .line_height()
            .zip(laid_out_rows(state).find(|&(row, _, _)| row == self.row));
        // The row may be out of the last layout when the change follows
        // another before its correction applied.
        let viewport_top = match laid_out {
            Some((line_height, (_, top, _))) => {
                (top - line_height * self.offset) * (size / laid_out_size)
            }
            None => self.viewport_top * (size / self.size),
        };
        editor.update(cx, |editor, cx| {
            let x = editor.scroll_offset().x;
            editor.set_scroll_offset(point(x, -viewport_top.max(px(0.))), cx);
        });
        Self {
            steps_left: MAX_STEPS,
            ..self
        }
    }

    /// After a layout with the new typography, scroll the row back to where
    /// it was. Returns the anchor until a layout shows the row in place, so
    /// a change before the correction applies keeps this row, and gives up
    /// after a few frames, as at the end of the document, where the row
    /// cannot reach its old place.
    pub(crate) fn restore(self, editor: &Entity<EditorState>, cx: &mut App) -> Option<Self> {
        let state = editor.read(cx);
        // Before the editor's first layout there is nothing to place the
        // row by yet; wait for one.
        let (Some(line_height), Some(text_bounds)) = (state.line_height(), state.text_bounds())
        else {
            return Some(self);
        };
        let viewport_top = state.input_bounds().top() - text_bounds.top();
        let rows: Vec<_> = laid_out_rows(state).collect();
        let (&(first_row, first_top, _), &(last_row, last_top, _)) = (rows.first()?, rows.last()?);
        let (top, found) = match rows.iter().find(|&&(row, _, _)| row == self.row) {
            Some(&(_, top, _)) => (top, true),
            None => {
                // Step by the laid-out rows' average height per line, which
                // counts their wrapped rows.
                let per_row = if last_row > first_row {
                    (last_top - first_top) / (last_row - first_row) as f32
                } else {
                    line_height
                };
                let top = if self.row < first_row {
                    first_top - per_row * (first_row - self.row) as f32
                } else {
                    last_top + per_row * (self.row - last_row) as f32
                };
                (top, false)
            }
        };
        let target = (top - line_height * self.offset).max(px(0.));
        if found && (target - viewport_top).abs() <= px(0.5) {
            return None;
        }
        let x = state.scroll_offset().x;
        let editor = editor.clone();
        // Not while painting: the offset applies from the next layout.
        cx.defer(move |cx| {
            editor.update(cx, |editor, cx| {
                editor.set_scroll_offset(point(x, -target), cx);
            });
        });
        (self.steps_left > 1).then_some(Self {
            steps_left: self.steps_left - 1,
            ..self
        })
    }
}

/// Each laid-out buffer row with the top and bottom of its wrapped rows, in
/// content coordinates: from the top of the text, as the negated scroll
/// offset. Range bounds are in window coordinates and move with the scroll,
/// like the text's own origin.
pub(crate) fn laid_out_rows(
    editor: &EditorState,
) -> impl Iterator<Item = (usize, Pixels, Pixels)> + '_ {
    let text = editor.text();
    let origin = editor.text_bounds().map(|bounds| bounds.top());
    origin
        .zip(editor.visible_row_range())
        .into_iter()
        .flat_map(move |(origin, rows)| {
            rows.filter_map(move |row| {
                let range = text.line_start_offset(row)..text.line_end_offset(row);
                let bounds = editor.range_to_bounds(&range)?;
                Some((row, bounds.top() - origin, bounds.bottom() - origin))
            })
        })
}
