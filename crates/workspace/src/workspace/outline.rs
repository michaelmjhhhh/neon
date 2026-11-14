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

use super::{Workspace, sidebar_empty_state};

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

    /// Show `outline`. Typing above a heading moves it in the source but
    /// not in the list, so the list redraws only when a heading's text or
    /// level changes; the lit row moves, and the list scrolls to it, only
    /// when the section changes, so a reader who scrolled the list keeps
    /// their place while typing.
    fn apply(&mut self, outline: Outline, cx: &mut Context<Self>) {
        let first_read = !std::mem::replace(&mut self.read, true);
        let shown = |heading: &markdown::OutlineHeading| (heading.level(), heading.text().clone());
        let changed = !self
            .outline
            .headings()
            .iter()
            .map(shown)
            .eq(outline.headings().iter().map(shown));
        if changed {
            self.depths =
                markdown::indent_levels(outline.headings().iter().map(|h| h.level())).into();
        }
        self.outline = Arc::new(outline);
        if changed || first_read {
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
        // Ancestor hairlines and the row radius match the Files tree, so the
        // sidebar's two lists read as one.
        let guide = theme.sidebar_border;
        // The hairlines of the sections around the lit row are drawn
        // stronger, so the reader sees which section they are in.
        let lit_guide = theme.muted_foreground.opacity(0.5);
        let radius = theme.radius;
        let count = self.outline.headings().len();
        let rem = window.rem_size();
        let outline = self.outline.clone();
        let depths = self.depths.clone();
        let active = self.active;
        let lit_spans = active.map_or_else(Vec::new, |ix| section_spans(&self.depths, ix));
        let workspace = self.workspace.clone();
        // Virtualized: a long manual can have thousands of headings.
        let list = uniform_list("outline-headings", count, move |range, _, cx| {
            let muted = cx.theme().muted_foreground;
            range
                .map(|ix| {
                    let heading = &outline.headings()[ix];
                    let text = SharedString::from(heading.text().clone());
                    let depth = depths[ix];
                    let selected = active == Some(ix);
                    let workspace = workspace.clone();
                    // One slot per ancestor, its hairline under the
                    // ancestor's first letter, so a section's subheadings
                    // hang from it.
                    let guides = (0..depth).map(|level| {
                        let lit = lit_spans.get(level).is_some_and(|span| span.contains(&ix));
                        div()
                            .flex_none()
                            .w(rem * 0.875)
                            .h_7()
                            .pl(rem * 0.125)
                            .child(div().w(gpui_kit::px(1.)).h_full().bg(if lit {
                                lit_guide
                            } else {
                                guide
                            }))
                    });
                    let item = ListItem::new(("outline-heading", ix))
                        .h_7()
                        .px_0()
                        .py_0()
                        .pl(rem * 0.5)
                        .pr(rem * 0.5)
                        .rounded(radius)
                        .text_sm()
                        .selected(selected)
                        .accessibility_label(text.clone())
                        .on_click(move |_, window, cx| {
                            workspace
                                .update(cx, |this, cx| this.go_to_outline_heading(ix, window, cx))
                                .ok();
                        })
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .w_full()
                                .min_w_0()
                                .children(guides)
                                .child(
                                    // Weight and contrast step down with
                                    // depth: sections lead, details recede
                                    // until their row is lit.
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .truncate()
                                        .when(depth == 0, |this| {
                                            this.font_weight(FontWeight::MEDIUM)
                                        })
                                        .when(depth >= 2 && !selected, |this| {
                                            this.text_color(muted)
                                        })
                                        .child(text),
                                ),
                        );
                    // Each row is laid out at the list's width, so the inset
                    // is padding around the row, not its margin.
                    div().px_2().child(item).into_any_element()
                })
                .collect::<Vec<AnyElement>>()
        })
        .track_scroll(&self.scroll)
        .flex_1()
        .min_h_0();
        container.child(list)
    }
}

/// The rows inside each section that holds row `active`, by depth: the
/// rows below the section's heading, up to the next heading at or above its
/// depth. Depths step by one from parent to child, see
/// [`markdown::indent_levels`].
fn section_spans(depths: &[usize], active: usize) -> Vec<std::ops::Range<usize>> {
    let mut spans = vec![0..0; depths[active] + 1];
    let mut row = active;
    for depth in (0..=depths[active]).rev() {
        while depths[row] != depth {
            row -= 1;
        }
        let end = depths[row + 1..]
            .iter()
            .position(|&below| below <= depth)
            .map_or(depths.len(), |len| row + 1 + len);
        spans[depth] = row + 1..end;
    }
    spans
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
        let rows = editor_rows_in_view(editor)?;
        Some(
            self.caret_in_view(cx)
                .unwrap_or_else(|| editor.text().line_start_offset(rows.start)),
        )
    }

    /// The caret, if it shows in the editor's last layout. Cheap enough for
    /// every caret move: it reads the layout's rows rather than measuring
    /// any. A caret off screen, as right after a jump before the editor
    /// scrolls, leaves the lit row to the next sample.
    pub(super) fn caret_in_view(&self, cx: &App) -> Option<usize> {
        if !self.shows_editor_now() {
            return None;
        }
        let editor = self.tab().editor.read(cx);
        let caret = editor.cursor();
        let row = editor.text().offset_to_point(caret).row;
        editor_rows_in_view(editor)?.contains(&row).then_some(caret)
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
                let heading = markdown::percent_decoded(fragment);
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

/// The buffer rows the editor's last layout shows, at least partly. The
/// layout lays out one row past the bottom edge; that row is left out.
fn editor_rows_in_view(
    editor: &gpui_kit::component::input::EditorState,
) -> Option<std::ops::Range<usize>> {
    let rows = editor.visible_row_range()?;
    let end = rows.end.saturating_sub(1).max(rows.start + 1);
    (!rows.is_empty()).then_some(rows.start..end)
}
