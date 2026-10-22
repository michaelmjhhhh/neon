//! Zen mode: the document alone on one sheet, with the lights down. Only
//! the paragraph being written keeps its full ink; the rest of the text
//! fades back, and the caret line stays at the middle of the window.

use std::ops::Range;
use std::time::{Duration, Instant};

use gpui_kit::HighlightStyle;
use gpui_kit::component::input::{
    EditorState, Rope, RopeExt as _, TextDecoration, TextDecorationCollection,
};
use gpui_kit::{App, Entity, Pixels, Point, px};

/// How much of their ink the paragraphs away from the caret lose.
const UNLIT_FADE: f32 = 0.74;

/// The text column's narrowest measure, in characters of the editor font.
pub(crate) const MEASURE_CHARS: f32 = 72.;

/// The share of the window the text column takes when that is wider than
/// its measure.
pub(crate) const COLUMN_SHARE: f32 = 0.6;

/// How quickly the caret line glides to the middle: the time it takes to
/// close about two thirds of the distance left.
pub(crate) const GLIDE_SECONDS: f32 = 0.07;

/// Where the editor's text starts inside it with line numbers off: its 6 px
/// inset, the 6 px gutter margin and the 18 px fold-arrow lane. Fixed pixels
/// in gpui-base, so the footer under the text repeats them as pixels.
pub(crate) const TEXT_LEADING: Pixels = px(30.);
/// The editor's inset after the text, which its scrollbar overlays.
pub(crate) const TEXT_TRAILING: Pixels = px(10.);

/// What Zen shows in its column.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ZenView {
    Editor,
    Preview,
}

/// How long the footer takes to fade out once writing starts, and to come
/// back when the pointer moves.
pub(crate) const CHROME_FADE_OUT: Duration = Duration::from_millis(700);
pub(crate) const CHROME_FADE_IN: Duration = Duration::from_millis(160);

/// Zen's view, whether the window entered full screen for Zen, so leaving
/// Zen leaves full screen too, and the caret line's glide to the middle.
pub(crate) struct Zen {
    pub(crate) exits_fullscreen: bool,
    pub(crate) view: ZenView,
    /// The glide waits while the pointer is down, so a drag selection does
    /// not move under it.
    pub(crate) pointer_down: bool,
    pub(crate) gliding: bool,
    /// When the glide last stepped, and the scroll offset it asked for.
    pub(crate) last_glide: Option<(Instant, Pixels)>,
    /// How far the editor sits below the top of the column. The editor
    /// cannot scroll above its first line, so near the top of the text the
    /// glide lowers the editor instead, and the caret line still reaches the
    /// middle.
    pub(crate) lift: Pixels,
    /// Hidden from the first keystroke until the pointer moves.
    pub(crate) chrome_hidden: bool,
    /// Where the pointer last brought the footer back, so a pointer that
    /// barely moves while typing does not.
    pub(crate) pointer_at: Option<Point<Pixels>>,
}

/// How far the pointer moves before the footer comes back.
pub(crate) const POINTER_TRAVEL: Pixels = px(6.);

/// Dims one editor's text outside the paragraphs its selection touches.
pub(crate) struct ZenFocus {
    decorations: TextDecorationCollection,
}

impl ZenFocus {
    pub(crate) fn new(editor: &Entity<EditorState>, cx: &mut App) -> Self {
        let decorations = editor.update(cx, |editor, cx| {
            editor.create_decorations_collection(Vec::new(), cx)
        });
        Self { decorations }
    }

    /// Light the paragraphs under the selection and dim the rest.
    pub(crate) fn follow(&self, editor: &Entity<EditorState>, cx: &mut App) {
        let editor = editor.read(cx);
        let text = editor.text();
        let selection = editor.selected_range();
        let lit = paragraph_around(text, selection.start).start
            ..paragraph_around(text, selection.end).end;
        let style = HighlightStyle {
            fade_out: Some(UNLIT_FADE),
            ..Default::default()
        };
        let unlit = [0..lit.start, lit.end..text.len()]
            .into_iter()
            .filter(|range| !range.is_empty())
            .map(|range| TextDecoration::new(range, style))
            .collect();
        self.decorations.set(unlit, cx);
    }

    pub(crate) fn clear(&self, cx: &mut App) {
        self.decorations.clear(cx);
    }
}

/// How far [`paragraph_around`] looks for a blank line in each direction.
/// It runs on every keystroke, so a long run of text without one, such as a
/// big list or code block, lights a window of lines around the caret rather
/// than costing a walk over the whole run.
const PARAGRAPH_SCAN_LINES: usize = 200;

/// The paragraph containing `offset`: the run of non-blank lines around it,
/// at most [`PARAGRAPH_SCAN_LINES`] each way, from the start of its first
/// line to the end of its last. On a blank line, just that line.
pub(crate) fn paragraph_around(text: &Rope, offset: usize) -> Range<usize> {
    let is_blank = |row: usize| text.slice_line(row).chars().all(char::is_whitespace);
    let row = text.offset_to_point(offset.min(text.len())).row;
    let last_row = text.lines_len().saturating_sub(1);
    if is_blank(row) {
        return text.line_start_offset(row)..text.line_end_offset(row);
    }
    let mut first = row;
    while first > 0 && row - first < PARAGRAPH_SCAN_LINES && !is_blank(first - 1) {
        first -= 1;
    }
    let mut last = row;
    while last < last_row && last - row < PARAGRAPH_SCAN_LINES && !is_blank(last + 1) {
        last += 1;
    }
    text.line_start_offset(first)..text.line_end_offset(last)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lit(text: &str, at: &str) -> String {
        let rope = Rope::from(text);
        let range = paragraph_around(&rope, text.find(at).unwrap());
        text[range].to_string()
    }

    #[test]
    fn a_paragraph_runs_between_blank_lines() {
        let text = "# Title\n\nFirst line\nsecond line\n\nNext.";
        assert_eq!(lit(text, "second"), "First line\nsecond line");
        assert_eq!(lit(text, "Title"), "# Title");
        assert_eq!(lit(text, "Next"), "Next.");
    }

    #[test]
    fn whitespace_only_lines_separate_paragraphs() {
        assert_eq!(lit("One\n  \nTwo", "Two"), "Two");
    }

    #[test]
    fn a_blank_line_lights_only_itself() {
        let text = "One\n\nTwo";
        let rope = Rope::from(text);
        assert_eq!(paragraph_around(&rope, 4), 4..4);
    }

    #[test]
    fn the_end_of_the_text_belongs_to_the_last_paragraph() {
        let text = "One\n\nTwo";
        let rope = Rope::from(text);
        assert_eq!(paragraph_around(&rope, text.len()), 5..8);
    }

    #[test]
    fn a_long_run_lights_a_window_around_the_caret() {
        let text = "line\n".repeat(1_000);
        let rope = Rope::from(text.as_str());
        let row = 500;
        let range = paragraph_around(&rope, row * 5);
        let first = rope.offset_to_point(range.start).row;
        let last = rope.offset_to_point(range.end).row;
        assert_eq!(first, row - PARAGRAPH_SCAN_LINES);
        assert_eq!(last, row + PARAGRAPH_SCAN_LINES);
    }
}
