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

use gpui_kit::Pixels;
use gpui_kit::component::input::{EditorState, RopeExt as _};

use crate::scroll_anchor::laid_out_rows;

/// A 0-based source line, with a fraction for the way through it, and its
/// height below the viewport's top. `None` before the editor's first layout.
pub(crate) type Anchor = Option<(f32, Pixels)>;

/// The line to match while the editor scrolls.
pub(crate) fn scroll_anchor(editor: &EditorState) -> Anchor {
    let viewport = editor.input_bounds();
    let viewport_top = viewport.top() - editor.text_bounds()?.top();
    let rows: Vec<_> = laid_out_rows(editor).collect();
    let line_at = |y: Pixels| {
        let &(last, _, last_bottom) = rows.last()?;
        if y >= last_bottom {
            return Some((last + 1) as f32);
        }
        let &(row, top, bottom) = rows.iter().find(|&&(_, _, bottom)| y < bottom)?;
        let fraction = (y - top) / (bottom - top).max(gpui_kit::px(1.));
        Some(row as f32 + fraction.clamp(0., 1.))
    };
    let lines = editor.text().lines_len().max(1) as f32;
    let progress = (line_at(viewport_top)? / lines).clamp(0., 1.);
    let y = viewport.size.height * progress;
    Some((line_at(viewport_top + y)?, y))
}

/// The line holding `offset`, where the editor shows it, to bring the
/// preview's text beside it. `None` when the line is not laid out.
pub(crate) fn line_anchor(editor: &EditorState, offset: usize) -> Anchor {
    let viewport_top = editor.input_bounds().top() - editor.text_bounds()?.top();
    let row = editor.text().offset_to_point(offset).row;
    let (_, top, _) = laid_out_rows(editor).find(|&(laid_out, _, _)| laid_out == row)?;
    Some((row as f32, top - viewport_top))
}
