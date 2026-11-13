use std::cell::{Cell, RefCell};
use std::ops::Range;
use std::rc::Rc;

use gpui_kit::component::input::{
    Editor, EditorState, Escape, Replace, Rope, RopeExt as _, Search,
};
use gpui_kit::{
    AppContext as _, Bounds, Context, DispatchPhase, ElementInputHandler, Entity,
    EntityInputHandler, Focusable as _, InteractiveElement as _, IntoElement, MouseButton,
    MouseDownEvent, ParentElement as _, Pixels, Point, Render, SharedString, Styled as _,
    Subscription, TextRun, UTF16Selection, Window, canvas, div, point,
};
use markdown_preview::{MarkdownPreview, SourceAnchor};

use crate::find_bar::FindBar;
use crate::list_guides;
use crate::preview_sync::{self, LayoutKey};
use crate::scroll_anchor::ScrollAnchor;
use crate::slash_commands::SlashMenu;
use crate::slash_menu;
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
    find_bar: Entity<FindBar>,
    slash_menu: Entity<SlashMenu>,
    readonly: bool,
    /// The editor's text as of its last layout.
    laid_out_text: Rc<RefCell<Rope>>,
    /// The text size of the editor's last layout; zero before the first.
    laid_out_size: Rc<Cell<f32>>,
    /// The line to keep in place while a typography change takes effect.
    scroll_anchor: Rc<Cell<Option<ScrollAnchor>>>,
    /// The preview of the same document, which follows the editor's scroll
    /// while `sync_preview` is on, as in Split.
    preview: Entity<MarkdownPreview>,
    sync_preview: bool,
    /// The layout the anchor was last computed for and the anchor sent to
    /// the preview, so a paint that did not move the editor neither
    /// recomputes it nor moves a preview the reader scrolled alone.
    synced: Rc<Cell<Option<(LayoutKey, SourceAnchor)>>>,
    /// A double-click asked to bring the preview beside the caret's line.
    align_caret: Rc<Cell<bool>>,
    _slash_menu_changed: Subscription,
}

impl EditorPane {
    pub(crate) fn new(
        editor: Entity<EditorState>,
        preview: Entity<MarkdownPreview>,
        typography: Entity<Typography>,
        slash_menu: Entity<SlashMenu>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let find_bar = cx.new(|cx| FindBar::new(editor.clone(), window, cx));
        let slash_menu_changed = cx.observe(&slash_menu, |_, _, cx| cx.notify());
        Self {
            editor,
            typography,
            find_bar,
            slash_menu,
            readonly: false,
            laid_out_text: Rc::default(),
            laid_out_size: Rc::default(),
            scroll_anchor: Rc::default(),
            preview,
            sync_preview: false,
            synced: Rc::default(),
            align_caret: Rc::default(),
            _slash_menu_changed: slash_menu_changed,
        }
    }

    /// Have the preview follow the editor's scroll, and a double-click
    /// bring the preview's text beside the clicked line. Turning it on
    /// matches the preview to the editor on the next paint.
    pub(crate) fn set_sync_preview(&mut self, on: bool, cx: &mut Context<Self>) {
        if self.sync_preview != on {
            self.sync_preview = on;
            self.synced.set(None);
            cx.notify();
        }
    }

    /// Match the preview to the editor again on the next paint, even if the
    /// editor did not move, as after the caret moved: editing brings back a
    /// preview the reader scrolled elsewhere.
    pub(crate) fn resync(&self) {
        self.synced.set(None);
    }

    /// Keep the line at the top of the editor in place after a typography
    /// change. Call before the editor lays out with the new typography.
    pub(crate) fn anchor_scroll(&mut self, cx: &mut Context<Self>) {
        let laid_out_size = self.laid_out_size.get();
        if laid_out_size == 0. {
            return;
        }
        // Changes since the last layout keep the line found before them.
        let Some(anchor) = self
            .scroll_anchor
            .get()
            .or_else(|| ScrollAnchor::capture(self.editor.read(cx), laid_out_size))
        else {
            return;
        };
        let size = self.typography.read(cx).editor_size(cx);
        let anchor = anchor.estimate(&self.editor, laid_out_size, size, cx);
        self.scroll_anchor.set(Some(anchor));
    }

    pub(crate) fn set_readonly(&mut self, readonly: bool, cx: &mut Context<Self>) {
        if self.readonly != readonly {
            self.readonly = readonly;
            self.find_bar
                .update(cx, |find_bar, cx| find_bar.set_readonly(readonly, cx));
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
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let typography = self.typography.read(cx);
        let pane = cx.entity();
        let editor = self.editor.clone();
        let typography_entity = self.typography.clone();
        let laid_out_text = self.laid_out_text.clone();
        let laid_out_size = self.laid_out_size.clone();
        let scroll_anchor = self.scroll_anchor.clone();
        let sync = self.sync_preview.then(|| PreviewSync {
            preview: self.preview.clone(),
            synced: self.synced.clone(),
            align_caret: self.align_caret.clone(),
        });
        let size = typography.editor_size(cx);
        let slash_menu = slash_menu::render(
            &self.slash_menu,
            &self.editor,
            typography.editor_font(cx),
            window.rem_size() * (size / 16.),
            cx,
        );
        div()
            .size_full()
            .relative()
            // From the editor or the find bar itself.
            .on_action(cx.listener(|this, _: &Search, window, cx| {
                this.find_bar
                    .update(cx, |find_bar, cx| find_bar.show(false, window, cx));
            }))
            .on_action(cx.listener(|this, _: &Replace, window, cx| {
                this.find_bar
                    .update(cx, |find_bar, cx| find_bar.show(true, window, cx));
            }))
            // Escape the editor had no use for, such as with no menu open,
            // closes the find bar too.
            .on_action(cx.listener(|this, _: &Escape, window, cx| {
                if !this
                    .find_bar
                    .update(cx, |find_bar, cx| find_bar.dismiss(window, cx))
                {
                    cx.propagate();
                }
            }))
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
                // the editor registered for the frame, and the list guides
                // use the editor's layout for this frame.
                canvas(
                    |_, _, _| {},
                    move |bounds, _, window, cx| {
                        list_guides::paint(&editor, &typography_entity, bounds, window, cx);
                        *laid_out_text.borrow_mut() = editor.read(cx).text().clone();
                        laid_out_size.set(size);
                        if let Some(anchor) = scroll_anchor.take() {
                            scroll_anchor.set(anchor.restore(&editor, cx));
                        }
                        if let Some(sync) = &sync {
                            sync.paint(&editor, window, cx);
                        }
                        let focus = editor.read(cx).focus_handle(cx);
                        window.handle_input(&focus, ElementInputHandler::new(bounds, pane), cx);
                    },
                )
                .absolute()
                .size_full(),
            )
            .child(self.find_bar.clone())
            .children(slash_menu)
    }
}

/// The pane's state for matching the preview to the editor, for its paint.
struct PreviewSync {
    preview: Entity<MarkdownPreview>,
    synced: Rc<Cell<Option<(LayoutKey, SourceAnchor)>>>,
    align_caret: Rc<Cell<bool>>,
}

impl PreviewSync {
    /// Match the preview to the editor's layout for this frame: beside the
    /// caret's line after a double-click, or else by scroll when the editor
    /// moved. Then listen for the next double-click.
    fn paint(&self, editor: &Entity<EditorState>, window: &mut Window, cx: &mut gpui_kit::App) {
        let state = editor.read(cx);
        let key = LayoutKey::of(state);
        let last = self.synced.get();
        let scroll_anchor = || {
            if last.map(|(key, _)| key) == Some(key) {
                return last.map(|(_, anchor)| anchor);
            }
            preview_sync::scroll_anchor(state)
        };
        let anchor = if self.align_caret.take() {
            // Recorded as sent, so the line stays beside the caret until the
            // editor scrolls.
            self.synced.set(scroll_anchor().map(|anchor| (key, anchor)));
            preview_sync::line_anchor(state, state.cursor())
        } else if last.map(|(key, _)| key) != Some(key) {
            let anchor = scroll_anchor();
            self.synced.set(anchor.map(|anchor| (key, anchor)));
            anchor.filter(|&anchor| last.map(|(_, sent)| sent) != Some(anchor))
        } else {
            None
        };
        if let Some(anchor) = anchor {
            let preview = self.preview.clone();
            // Not while painting: the preview moves on the next frame.
            cx.defer(move |cx| {
                preview.update(cx, |preview, cx| preview.scroll_to_source_line(anchor, cx));
            });
        }

        let editor = editor.clone();
        let align_caret = self.align_caret.clone();
        window.on_mouse_event(move |event: &MouseDownEvent, phase, window, cx| {
            let state = editor.read(cx);
            // Focus excludes the find bar and menus over the editor: the
            // first click of the pair moved focus there.
            if phase == DispatchPhase::Bubble
                && event.button == MouseButton::Left
                && event.click_count == 2
                && state.input_bounds().contains(&event.position)
                && state.focus_handle(cx).is_focused(window)
            {
                // The next paint aligns, once the click has placed the caret.
                align_caret.set(true);
            }
        });
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
