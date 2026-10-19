use std::cell::RefCell;
use std::ops::Range;
use std::rc::Rc;

use gpui_kit::component::input::{Editor, EditorState, Rope, RopeExt as _};
use gpui_kit::{
    Bounds, Context, ElementInputHandler, Entity, EntityInputHandler, Focusable as _, IntoElement,
    ParentElement as _, Pixels, Point, Render, SharedString, Styled as _, TextRun, UTF16Selection,
    Window, canvas, div, point,
};

use crate::typography::Typography;

/// The source editor as its own view, so the workspace can cache it: a
/// keystroke re-renders this view alone, and a preview update or status
/// change reuses the editor's last frame instead of laying it out again.
///
/// It also stands in for the editor as the window's text input handler, to
/// place the input method's candidate window; see
/// [`EditorPane::bounds_for_range`].
pub(crate) struct EditorPane {
    editor: Entity<EditorState>,
    typography: Entity<Typography>,
    readonly: bool,
    /// The editor's text as of its last layout.
    laid_out_text: Rc<RefCell<Rope>>,
}

impl EditorPane {
    pub(crate) fn new(editor: Entity<EditorState>, typography: Entity<Typography>) -> Self {
        Self {
            editor,
            typography,
            readonly: false,
            laid_out_text: Rc::default(),
        }
    }

    pub(crate) fn set_readonly(&mut self, readonly: bool, cx: &mut Context<Self>) {
        if self.readonly != readonly {
            self.readonly = readonly;
            cx.notify();
        }
    }

    /// Where `offset` in the editor's current text is drawn, and the line
    /// height there, even when the last layout predates the latest edit.
    ///
    /// The last layout is still right for the offset's line up to the first
    /// character that changed, so the editor places that point; the text
    /// after it is measured by shaping it in the editor's font. Returns
    /// `None` when lines above the offset changed since the layout.
    fn current_origin(
        &self,
        offset: usize,
        editor_bounds: Bounds<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<(Point<Pixels>, Pixels)> {
        let text = self.editor.read(cx).text().clone();
        let laid_out = self.laid_out_text.borrow().clone();
        let row = text.offset_to_point(offset).row;
        let line_start = text.line_start_offset(row);
        if row >= laid_out.lines_len() || laid_out.line_start_offset(row) != line_start {
            return None;
        }
        let unchanged: usize = text
            .slice_line(row)
            .chars()
            .zip(laid_out.slice_line(row).chars())
            .take_while(|(current, laid_out)| current == laid_out)
            .map(|(ch, _)| ch.len_utf8())
            .sum();
        let anchor = line_start + unchanged.min(offset - line_start);
        let anchor_utf16 = text.offset_to_offset_utf16(anchor);
        let anchor_bounds = self.editor.update(cx, |editor, cx| {
            editor.bounds_for_range(anchor_utf16..anchor_utf16, editor_bounds, window, cx)
        })?;
        let mut origin = anchor_bounds.origin;
        if offset > anchor {
            let typography = self.typography.read(cx);
            let font = gpui_kit::font(typography.editor_font(cx));
            let size = window.rem_size() * (typography.editor_size(cx) / 16.);
            let typed = SharedString::from(text.slice(anchor..offset).to_string());
            let run = TextRun {
                len: typed.len(),
                font,
                color: gpui_kit::black(),
                background_color: None,
                underline: None,
                strikethrough: None,
            };
            origin.x += window
                .text_system()
                .shape_line(typed, size, &[run], None)
                .width;
        }
        Some((origin, anchor_bounds.size.height))
    }
}

impl Render for EditorPane {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let typography = self.typography.read(cx);
        let pane = cx.entity();
        let editor = self.editor.clone();
        let laid_out_text = self.laid_out_text.clone();
        div()
            .size_full()
            .child(
                Editor::new(&self.editor)
                    .readonly(self.readonly)
                    .font_family(typography.editor_font(cx))
                    .text_size(gpui_kit::rems(typography.editor_size(cx) / 16.))
                    // The workspace sheet draws the boundary; the editor stays flat.
                    .bordered(false)
                    .size_full(),
            )
            .child(
                // Paints after the editor, so this handler replaces the one
                // the editor registered for the frame.
                canvas(
                    |_, _, _| {},
                    move |bounds, _, window, cx| {
                        *laid_out_text.borrow_mut() = editor.read(cx).text().clone();
                        let focus = editor.read(cx).focus_handle(cx);
                        window.handle_input(&focus, ElementInputHandler::new(bounds, pane), cx);
                    },
                )
                .absolute()
                .size_full(),
            )
    }
}

/// Forwards to the editor, except for [`Self::bounds_for_range`].
impl EntityInputHandler for EditorPane {
    fn text_for_range(
        &mut self,
        range: Range<usize>,
        adjusted_range: &mut Option<Range<usize>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<String> {
        self.editor.update(cx, |editor, cx| {
            editor.text_for_range(range, adjusted_range, window, cx)
        })
    }

    fn selected_text_range(
        &mut self,
        ignore_disabled_input: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        self.editor.update(cx, |editor, cx| {
            editor.selected_text_range(ignore_disabled_input, window, cx)
        })
    }

    fn marked_text_range(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        self.editor
            .update(cx, |editor, cx| editor.marked_text_range(window, cx))
    }

    fn unmark_text(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.editor
            .update(cx, |editor, cx| editor.unmark_text(window, cx));
    }

    fn replace_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.editor.update(cx, |editor, cx| {
            editor.replace_text_in_range(range, text, window, cx)
        });
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        new_text: &str,
        new_selected_range: Option<Range<usize>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.editor.update(cx, |editor, cx| {
            editor.replace_and_mark_text_in_range(range, new_text, new_selected_range, window, cx)
        });
    }

    /// Where the input method puts its candidate window.
    ///
    /// macOS asks right after each change to the text being composed, usually
    /// before the next frame. The editor answers from its last layout, so
    /// typing faster than the frame rate placed the window a few characters
    /// behind the caret, and it jumped forward on the next keystroke. This
    /// answers from the current text instead.
    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        _: Bounds<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let editor_bounds = self.editor.read(cx).text_bounds()?;
        let text = self.editor.read(cx).text().clone();
        let start = text.offset_utf16_to_offset(range_utf16.start);
        let end = text.offset_utf16_to_offset(range_utf16.end);
        let origins = self
            .current_origin(start, editor_bounds, window, cx)
            .zip(self.current_origin(end, editor_bounds, window, cx));
        let Some(((start, line_height), (end, _))) = origins else {
            return self.editor.update(cx, |editor, cx| {
                editor.bounds_for_range(range_utf16, editor_bounds, window, cx)
            });
        };
        // Below the line, like the editor, so the window does not cover it.
        Some(Bounds::from_corners(
            start,
            point(end.x, start.y + line_height),
        ))
    }

    fn character_index_for_point(
        &mut self,
        point: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<usize> {
        self.editor.update(cx, |editor, cx| {
            editor.character_index_for_point(point, window, cx)
        })
    }
}
