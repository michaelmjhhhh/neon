//! The sidebar's Outline view: the shown document's headings, to see its
//! shape and go to a section.
//!
//! Headings are read off the main thread, only while the view shows, once
//! typing pauses. The row of the section being worked on is lit: the
//! caret's section while the caret is on screen, else the section at the top
//! of the pane being read. A heading picked in the list moves the caret to
//! it and brings it to the top of the editor, or of the preview when the
//! preview shows alone.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use gpui_kit::assets::IconName as AssetIcon;
use gpui_kit::component::input::{Rope, RopeExt as _};
use gpui_kit::component::list::ListItem;
use gpui_kit::component::notification::Notification;
use gpui_kit::component::{ActiveTheme as _, WindowExt as _};
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, FontWeight, IntoElement, ParentElement as _, Render,
    ScrollStrategy, SharedString, Styled as _, Task, UniformListScrollHandle, WeakEntity, Window,
    div, prelude::FluentBuilder as _, uniform_list,
};
use markdown::Outline;

use super::{Workspace, format_count, sidebar_empty_state};
use crate::scroll_anchor::laid_out_rows;
use crate::zen::ZenView;

/// How long typing pauses before the headings are read again. Shorter than
/// the preview's wait: the list is small and cheap to redraw.
const READ_DELAY: Duration = Duration::from_millis(150);

/// How often the lit row follows scrolling while the view shows. Caret
/// moves follow at once.
const FOLLOW_INTERVAL: Duration = Duration::from_millis(120);

/// Line heights left above a heading brought to the top of the editor, so
/// it does not sit against the edge.
const HEADING_TOP_MARGIN_LINES: f32 = 1.;

/// The list of headings, as its own view: lighting another row as the caret
/// moves redraws this list alone, not the sidebar around it.
pub(super) struct OutlineView {
    outline: Arc<Outline>,
    /// Each heading's indent, in steps; see [`markdown::indent_levels`].
    depths: Arc<[usize]>,
    /// Source given since `outline` was read, waiting for the view to show
    /// or for typing to pause.
    pending: Option<Rope>,
    /// Read the pending source without waiting for typing to pause, as for
    /// a document just shown.
    urgent: bool,
    /// The wait and read in flight, and how to stop the read.
    task: Option<Task<()>>,
    cancel: Option<Arc<AtomicBool>>,
    shown: bool,
    /// Whether the shown document's headings have been read once, so an
    /// empty list means it has none, rather than that they are on the way.
    read: bool,
    /// Where the writer or reader is in the source, and the heading whose
    /// section holds it.
    position: Option<usize>,
    active: Option<usize>,
    follow_task: Option<Task<()>>,
    scroll: UniformListScrollHandle,
    workspace: WeakEntity<Workspace>,
}

impl OutlineView {
    pub(super) fn new(workspace: WeakEntity<Workspace>) -> Self {
        Self {
            outline: Arc::default(),
            depths: Arc::default(),
            pending: None,
            urgent: false,
            task: None,
            cancel: None,
            shown: false,
            read: false,
            position: None,
            active: None,
            follow_task: None,
            scroll: UniformListScrollHandle::new(),
            workspace,
        }
    }

    /// Follow the shown document's text. `new_document` is for a document
    /// just shown, whose headings replace the list at once rather than once
    /// typing pauses; until they are read the list is empty, so no row
    /// points into the document shown before.
    pub(super) fn set_source(&mut self, source: Rope, new_document: bool, cx: &mut Context<Self>) {
        self.pending = Some(source);
        if new_document {
            self.cancel_read();
            self.urgent = true;
            self.read = false;
            self.position = None;
            self.active = None;
            if !self.outline.is_empty() {
                self.outline = Arc::default();
                self.depths = Arc::default();
                cx.notify();
            }
        }
        if self.shown && self.task.is_none() {
            self.start_read(cx);
        }
    }

    pub(super) fn is_shown(&self) -> bool {
        self.shown
    }

    /// Read headings and follow the caret only while the list is on screen.
    pub(super) fn set_shown(&mut self, shown: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.shown == shown {
            return;
        }
        self.shown = shown;
        if !shown {
            // A read in flight finishes; the next waits for the view.
            self.follow_task = None;
            return;
        }
        if self.pending.is_some() {
            // Shown again after edits: no reason to wait for a pause.
            self.cancel_read();
            self.urgent = true;
            self.start_read(cx);
        }
        self.follow_task = Some(cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor().timer(FOLLOW_INTERVAL).await;
                let Ok(workspace) = this.read_with(cx, |this, _| this.workspace.clone()) else {
                    break;
                };
                let Ok(position) =
                    workspace.read_with(cx, |workspace, cx| workspace.outline_position(cx))
                else {
                    break;
                };
                if this
                    .update(cx, |this, cx| this.set_position(position, cx))
                    .is_err()
                {
                    break;
                }
            }
        }));
        cx.notify();
    }

    /// Light the heading whose section holds the source byte `position`,
    /// scrolling the list to keep it in view.
    pub(super) fn set_position(&mut self, position: Option<usize>, cx: &mut Context<Self>) {
        self.position = position;
        self.light_section(cx);
    }

    fn light_section(&mut self, cx: &mut Context<Self>) {
        let active = self
            .position
            .and_then(|position| self.outline.section_at(position));
        if active != self.active {
            self.active = active;
            if let Some(ix) = active {
                self.scroll.scroll_to_item(ix, ScrollStrategy::Nearest);
            }
            cx.notify();
        }
    }

    /// Where the heading in row `ix` starts in `current`, the document's
    /// text now. If the list is behind the text, as right after typing, the
    /// headings are read again first, and the same heading is found by its
    /// text and how many headings above share it, so the caret lands on the
    /// heading as it reads now.
    fn heading_start(
        &mut self,
        ix: usize,
        current: &Rope,
        cx: &mut Context<Self>,
    ) -> Option<usize> {
        let picked = self.outline.headings().get(ix)?.clone();
        if self.pending.is_none() && self.task.is_none() {
            return Some(picked.source_range().start);
        }
        self.cancel_read();
        self.pending = None;
        let outline = Outline::parse(&current.to_string());
        let same_text = |heading: &&markdown::OutlineHeading| heading.text() == picked.text();
        let occurrence = self.outline.headings()[..ix]
            .iter()
            .filter(same_text)
            .count();
        let start = outline
            .headings()
            .iter()
            .filter(same_text)
            .nth(occurrence)
            .or_else(|| outline.headings().get(ix))
            .map(|heading| heading.source_range().start);
        self.apply(outline, cx);
        start
    }

    fn start_read(&mut self, cx: &mut Context<Self>) {
        let urgent = std::mem::take(&mut self.urgent);
        let cancel = Arc::new(AtomicBool::new(false));
        self.cancel = Some(cancel.clone());
        self.task = Some(cx.spawn(async move |this, cx| {
            if !urgent {
                // Coalesce typing, keeping only the latest source.
                cx.background_executor().timer(READ_DELAY).await;
            }
            let Ok(Some(source)) = this.update(cx, |this, _| this.pending.take()) else {
                return;
            };
            let outline = cx
                .background_spawn(async move {
                    // Flattened here, off the main thread.
                    let text = source.to_string();
                    Outline::parse_with_cancellation(&text, || cancel.load(Ordering::Relaxed))
                })
                .await;
            this.update(cx, |this, cx| {
                this.task = None;
                this.cancel = None;
                if let Some(outline) = outline {
                    this.apply(outline, cx);
                }
                if this.shown && this.pending.is_some() {
                    this.start_read(cx);
                }
            })
            .ok();
        }));
    }

    fn cancel_read(&mut self) {
        if let Some(cancel) = self.cancel.take() {
            cancel.store(true, Ordering::Relaxed);
        }
        self.task = None;
    }

    fn apply(&mut self, outline: Outline, cx: &mut Context<Self>) {
        let first_read = !std::mem::replace(&mut self.read, true);
        if *self.outline != outline {
            self.depths =
                markdown::indent_levels(outline.headings().iter().map(|h| h.level())).into();
            self.outline = Arc::new(outline);
            self.active = None;
            cx.notify();
        } else if first_read {
            cx.notify();
        }
        self.light_section(cx);
    }
}

impl Drop for OutlineView {
    fn drop(&mut self) {
        self.cancel_read();
    }
}

impl Render for OutlineView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let container = div().flex().flex_col().flex_1().min_h_0().pt_1();
        if self.outline.is_empty() {
            if !self.read {
                // Read in a moment; an empty state now would only flash.
                return container;
            }
            return container.child(sidebar_empty_state(
                AssetIcon::TableOfContents,
                "No headings",
                "Start a line with # and a space to add a heading. Headings show here as you \
                 write.",
            ));
        }
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        let count = self.outline.headings().len();
        let summary = match count {
            1 => "1 heading".to_string(),
            count => format!("{} headings", format_count(count)),
        };
        let rem = window.rem_size();
        let radius = theme.radius;
        let outline = self.outline.clone();
        let depths = self.depths.clone();
        let active = self.active;
        let workspace = self.workspace.clone();
        // Virtualized: a long manual can have thousands of headings.
        let list = uniform_list("outline-headings", count, move |range, _, cx| {
            let muted = cx.theme().muted_foreground;
            range
                .map(|ix| {
                    let heading = &outline.headings()[ix];
                    let text = SharedString::from(heading.text().clone());
                    let workspace = workspace.clone();
                    let item = ListItem::new(("outline-heading", ix))
                        .h_7()
                        .px_0()
                        .py_0()
                        .pl(rem * (0.5 + 0.75 * depths[ix] as f32))
                        .pr(rem * 0.5)
                        .rounded(radius)
                        .text_sm()
                        .selected(active == Some(ix))
                        .accessibility_label(text.clone())
                        .on_click(move |_, window, cx| {
                            workspace
                                .update(cx, |this, cx| this.go_to_outline_heading(ix, window, cx))
                                .ok();
                        })
                        .child(
                            div()
                                .min_w_0()
                                .truncate()
                                .when(heading.level() == 1, |this| {
                                    this.font_weight(FontWeight::MEDIUM)
                                })
                                .when(heading.level() >= 5, |this| this.text_color(muted))
                                .child(text),
                        );
                    div().px_2().child(item).into_any_element()
                })
                .collect::<Vec<AnyElement>>()
        })
        .track_scroll(&self.scroll)
        .flex_1()
        .min_h_0();
        container
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .h_7()
                    .mx_2()
                    .pl_1()
                    .text_xs()
                    .text_color(muted)
                    .child(summary),
            )
            .child(list)
    }
}

impl Workspace {
    /// Whether the Outline view is on screen.
    pub(super) fn outline_shown(&self) -> bool {
        self.sidebar_visible
            && self.sidebar_view == super::SidebarView::Outline
            && self.zen.is_none()
            && !self.settings_open
    }

    /// Tell the Outline view whether it shows, after anything that may
    /// have changed it.
    pub(super) fn sync_outline_shown(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let shown = self.outline_shown();
        self.outline
            .update(cx, |outline, cx| outline.set_shown(shown, window, cx));
    }

    /// Give the Outline view the active document's text. `new_document`
    /// for a document just shown in place of another.
    pub(super) fn update_outline(&mut self, new_document: bool, cx: &mut Context<Self>) {
        let source = self.tab().editor.read(cx).text().clone();
        self.outline.update(cx, |outline, cx| {
            outline.set_source(source, new_document, cx)
        });
    }

    /// Where the writer or reader is in the active document: the caret
    /// while it is on screen in the editor, else the top of the pane being
    /// read. `None` before the pane is laid out.
    pub(super) fn outline_position(&self, cx: &App) -> Option<usize> {
        if !self.shows_editor_now() {
            return self.tab().preview.read(cx).reading_offset();
        }
        let editor = self.tab().editor.read(cx);
        let text = editor.text();
        let viewport_top = editor.input_bounds().top() - editor.text_bounds()?.top();
        let viewport_bottom = viewport_top + editor.input_bounds().size.height;
        let caret = editor.cursor();
        let caret_row = text.offset_to_point(caret).row;
        let mut first_visible = None;
        for (row, top, bottom) in laid_out_rows(editor) {
            if row == caret_row
                && top >= viewport_top - gpui_kit::px(1.)
                && bottom <= viewport_bottom + gpui_kit::px(1.)
            {
                return Some(caret);
            }
            if first_visible.is_none() && top >= viewport_top - gpui_kit::px(1.) {
                first_visible = Some(row);
            }
        }
        first_visible.map(|row| text.line_start_offset(row))
    }

    /// Whether the active document shows as source, rather than as its
    /// preview alone.
    fn shows_editor_now(&self) -> bool {
        self.zen_view()
            .map_or(self.layout.shows_editor(), |view| view == ZenView::Editor)
    }

    fn go_to_outline_heading(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let current = self.tab().editor.read(cx).text().clone();
        let start = self
            .outline
            .update(cx, |outline, cx| outline.heading_start(ix, &current, cx));
        if let Some(start) = start {
            self.go_to_source_offset(start, window, cx);
        }
    }

    /// Go to the heading a link's `#fragment` names in the active document,
    /// or say there is none.
    pub(super) fn go_to_heading(
        &mut self,
        fragment: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let text = self.tab().editor.read(cx).text().to_string();
        let outline = Outline::parse(&text);
        match outline.find(fragment) {
            Some(ix) => {
                let start = outline.headings()[ix].source_range().start;
                self.go_to_source_offset(start, window, cx);
            }
            None => {
                let heading = percent_decoded_lossy(fragment);
                let message = match self.tab().document.path() {
                    Some(path) => {
                        format!("“{}” has no heading “{heading}”.", super::file_name(path))
                    }
                    None => format!("This note has no heading “{heading}”."),
                };
                window.push_notification(Notification::info(message), cx);
            }
        }
    }

    /// Put the caret at the source byte `offset` and bring its line near
    /// the top of the editor; with the preview alone, bring the block there
    /// to the top of the preview instead. Then focus that pane.
    pub(super) fn go_to_source_offset(
        &mut self,
        offset: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let tab = self.tab();
        let (editor, editor_pane, preview) = (
            tab.editor.clone(),
            tab.editor_pane.clone(),
            tab.preview.clone(),
        );
        let offset = offset.min(editor.read(cx).text().len());
        if self.shows_editor_now() {
            let row = editor.read(cx).text().offset_to_point(offset).row;
            editor.update(cx, |editor, cx| {
                editor.set_selected_range(offset..offset, cx)
            });
            // Zen keeps the caret's line in the middle by itself.
            if self.zen.is_none() {
                editor_pane.update(cx, |pane, cx| {
                    pane.scroll_row_to_top(row, HEADING_TOP_MARGIN_LINES, cx)
                });
            }
        } else {
            preview.update(cx, |preview, cx| preview.scroll_to_offset(offset, cx));
        }
        self.focus_content(window, cx);
        self.outline
            .update(cx, |outline, cx| outline.set_position(Some(offset), cx));
    }

    /// Show the heading a link to another note named, now that the note
    /// shows.
    pub(super) fn reveal_pending_heading(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(fragment) = self.pending_heading.take() else {
            return;
        };
        // A document just read has no layout to scroll by yet.
        cx.on_next_frame(window, move |this, window, cx| {
            this.go_to_heading(&fragment, window, cx);
        });
    }
}

/// `%20`-style escapes decoded for a message, as written if not UTF-8.
fn percent_decoded_lossy(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut ix = 0;
    while ix < bytes.len() {
        let hex = |at: usize| bytes.get(at).and_then(|&b| (b as char).to_digit(16));
        match (bytes[ix], hex(ix + 1), hex(ix + 2)) {
            (b'%', Some(high), Some(low)) => {
                decoded.push((high * 16 + low) as u8);
                ix += 3;
            }
            (byte, _, _) => {
                decoded.push(byte);
                ix += 1;
            }
        }
    }
    String::from_utf8(decoded).unwrap_or_else(|_| text.to_string())
}
