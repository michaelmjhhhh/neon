use std::cell::Cell;
use std::path::Path;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use gpui_kit::component::empty::{
    Empty, EmptyDescription, EmptyHeader, EmptyMedia, EmptyMediaVariant, EmptyTitle,
};
use gpui_kit::component::input::Rope;
use gpui_kit::component::scroll::{Scrollbar, ScrollbarHandle};
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, Sizable as _};
use gpui_kit::{
    App, AppContext as _, Bounds, Context, Entity, EventEmitter, FocusHandle, Focusable,
    InteractiveElement as _, IntoElement, ListAlignment, ListOffset, ListState, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement as _, Pixels, Point, Render, Size,
    StyleRefinement, Styled as _, Task, WeakEntity, Window, canvas, div, list,
    prelude::FluentBuilder as _, px,
};
use markdown::MarkdownDocument;

use crate::KEY_CONTEXT;
use crate::blocks::{BlockLayout, render_block};
use crate::image_cache::PreviewImageCache;
use crate::rows::PreviewRows;

gpui_kit::actions!(
    markdown_preview,
    [
        ScrollUp,
        ScrollDown,
        ScrollPageUp,
        ScrollPageDown,
        ScrollToTop,
        ScrollToBottom
    ]
);

/// Show a Markdown file that a link in the preview points at, such as a
/// `[[wikilink]]`, in Neon rather than in another app. The window handles it.
#[derive(Clone, Debug, PartialEq, gpui_kit::Action)]
#[action(namespace = markdown_preview, no_json)]
pub struct OpenLinkedFile {
    /// The file beside the document, or the link as written when the
    /// document has not been saved yet.
    pub path: std::path::PathBuf,
    /// The link as written, such as `Note.md` or `sub/Note.md`, so the
    /// window can look for the note elsewhere in its folder, as Obsidian
    /// resolves `[[Note]]` by name.
    pub link: std::path::PathBuf,
}

/// How far beyond the viewport blocks are laid out ahead of scrolling, so fast
/// scrolling does not reveal blocks that are still being measured. Runtime
/// geometry, not design spacing.
const OVERDRAW: Pixels = px(1200.);

/// Minimum time between the starts of two parses while typing. An isolated
/// edit parses at once; a burst parses at most this often, so preview frames
/// do not compete with the editor's frames for every keystroke. Zed's
/// Markdown preview waits 200 ms.
const PARSE_INTERVAL: Duration = Duration::from_millis(200);

/// Emitted after a parse result has been applied.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MarkdownPreviewEvent {
    DocumentChanged,
}

/// A rendered, scrollable view of Markdown source.
///
/// The owner supplies source text with [`Self::set_source`] or
/// [`Self::load_source`]; the preview owns parsing, the block list and its
/// scroll position.
pub struct MarkdownPreview {
    document: Arc<MarkdownDocument>,
    rows: Arc<PreviewRows>,
    /// Once a document needs list chunks, keep their boundaries through edits
    /// and suspension. Merging them at the threshold loses row-local scroll.
    chunk_lists: bool,
    active: bool,
    source: Rope,
    image_cache: Option<Entity<PreviewImageCache>>,
    suspended_scroll: Option<(usize, Pixels)>,
    pending_reveal_offset: Option<usize>,
    font_family: Option<gpui_kit::SharedString>,
    font_scale: f32,
    /// Leading and trailing insets that replace the centered reading column.
    column_insets: Option<(Pixels, Pixels)>,
    list_state: ListState,
    scrollbar: Entity<PreviewScrollbar>,
    focus_handle: FocusHandle,
    /// Directory that relative image paths and links resolve against.
    base_dir: Option<Arc<Path>>,
    /// The parse in flight, or the wait before the next one. At most one
    /// runs; newer source waits in `pending_source` and only the latest is
    /// kept.
    parse_task: Option<Task<()>>,
    parse_cancel: Option<Arc<AtomicBool>>,
    pending_source: Option<Rope>,
    last_parse_at: Option<Instant>,
    /// The next applied parse replaces the document instead of patching it.
    replace_on_next_parse: bool,
    /// Height hint for blocks that have not been measured yet.
    estimated_block_height: Pixels,
}

impl EventEmitter<MarkdownPreviewEvent> for MarkdownPreview {}

impl MarkdownPreview {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let list_state = ListState::new(0, ListAlignment::Top, OVERDRAW);
        let preview = cx.weak_entity();
        let scrollbar = cx.new(|_| PreviewScrollbar {
            handle: PreviewScrollHandle {
                list_state: list_state.clone(),
                moved: Rc::default(),
            },
            preview,
        });
        Self {
            document: Arc::default(),
            rows: Arc::default(),
            chunk_lists: false,
            active: true,
            source: Rope::new(),
            image_cache: Some(PreviewImageCache::new(cx)),
            suspended_scroll: None,
            pending_reveal_offset: None,
            font_family: None,
            font_scale: 1.,
            column_insets: None,
            list_state,
            scrollbar,
            focus_handle: cx.focus_handle(),
            base_dir: None,
            parse_task: None,
            parse_cancel: None,
            pending_source: None,
            last_parse_at: None,
            replace_on_next_parse: false,
            estimated_block_height: px(48.),
        }
    }

    /// Suspend hidden previews without retaining their parsed text or images.
    /// Source updates still replace the latest shared rope; showing the preview
    /// parses that source once and restores its source-relative scroll anchor.
    pub fn set_active(&mut self, active: bool, cx: &mut Context<Self>) {
        if self.active == active {
            return;
        }
        self.active = active;
        self.cancel_parse();
        self.last_parse_at = None;
        if active {
            self.reset_image_cache(cx);
            self.replace_on_next_parse = true;
            self.pending_source = Some(self.source.clone());
            self.schedule_parse(cx);
        } else {
            let top = self.list_state.logical_scroll_top();
            if top.item_ix < self.rows.len(&self.document) {
                self.suspended_scroll = Some((
                    self.rows.source_range(top.item_ix, &self.document).start,
                    top.offset_in_item,
                ));
            }
            self.pending_source = None;
            self.document = Arc::default();
            self.rows = Arc::default();
            self.list_state.reset(0);
            self.reset_image_cache(cx);
        }
        cx.notify();
    }

    /// Change document typography and invalidate cached block heights.
    pub fn set_typography(
        &mut self,
        family: gpui_kit::SharedString,
        scale: f32,
        cx: &mut Context<Self>,
    ) {
        self.font_family = Some(family);
        // Only guards against nonsense; the host decides the sizes it offers.
        self.font_scale = scale.clamp(0.5, 4.);
        self.list_state
            .remeasure_items(0..self.rows.len(&self.document));
        cx.notify();
    }

    /// Lay blocks across the preview's full width, between these leading and
    /// trailing insets, instead of in the centered reading column; `None`
    /// returns to the column. For a host that sets the width itself.
    pub fn set_column_insets(&mut self, insets: Option<(Pixels, Pixels)>, cx: &mut Context<Self>) {
        if self.column_insets == insets {
            return;
        }
        self.column_insets = insets;
        self.list_state
            .remeasure_items(0..self.rows.len(&self.document));
        cx.notify();
    }

    /// Update relative resources after Save As without moving the reader.
    pub fn set_base_dir(&mut self, base_dir: Option<Arc<Path>>, cx: &mut Context<Self>) {
        if self.base_dir != base_dir && self.active {
            self.reset_image_cache(cx);
        }
        self.base_dir = base_dir;
        self.list_state
            .remeasure_items(0..self.rows.len(&self.document));
        cx.notify();
    }

    /// The preview with its scrollbar, both as cached views.
    ///
    /// The scrollbar notifies its view for hover, fades and idle timers.
    /// Inside the preview, each of those would lay out the visible blocks
    /// again; as a sibling view it redraws alone, and the blocks are laid
    /// out only when the preview itself changes.
    pub fn pane(preview: &Entity<Self>, cx: &App) -> impl IntoElement + use<> {
        let style = || StyleRefinement::default().size_full();
        let preview_ref = preview.read(cx);
        let show_scrollbar = !preview_ref.document.blocks().is_empty();
        let scrollbar = preview_ref.scrollbar.clone();
        div()
            .relative()
            .size_full()
            .child(preview.clone().cached(style()))
            .when(show_scrollbar, |this| {
                this.child(div().absolute().inset_0().child(scrollbar.cached(style())))
            })
    }

    /// Scroll by `distance`; positive moves toward the end.
    pub fn scroll_by(&mut self, distance: Pixels, cx: &mut Context<Self>) {
        self.list_state.scroll_by(distance);
        cx.notify();
    }

    /// The most recently applied parse.
    pub fn document(&self) -> &Arc<MarkdownDocument> {
        &self.document
    }

    /// Number of variable-height scroll rows, including chunks of large lists.
    pub fn row_count(&self) -> usize {
        self.rows.len(&self.document)
    }

    /// Vertical scroll distance from the top, using the scrollbar's measured offset.
    pub fn scroll_offset(&self) -> Pixels {
        (-self.list_state.scroll_px_offset_for_scrollbar().y).max(px(0.))
    }

    /// Fraction of the source reached by the bottom of the viewport.
    /// Source ranges keep progress independent of unmeasured block heights.
    pub fn viewed_fraction(&self) -> f32 {
        let count = self.row_count();
        if count == 0 {
            return 1.;
        }
        let total = self.rows.source_range(count - 1, &self.document).end.max(1) as f32;
        let viewport_bottom = self.list_state.viewport_bounds().bottom();
        let first = self.list_state.logical_scroll_top().item_ix;
        let mut viewed = if first < count {
            self.rows.source_range(first, &self.document).start as f32
        } else {
            0.
        };
        for ix in first..count {
            let Some(bounds) = self.list_state.bounds_for_item(ix) else {
                break;
            };
            if bounds.top() >= viewport_bottom {
                break;
            }
            let fraction = if bounds.size.height > px(0.) {
                ((viewport_bottom - bounds.top()) / bounds.size.height).clamp(0., 1.)
            } else {
                1.
            };
            let range = self.rows.source_range(ix, &self.document);
            viewed = range.start as f32 + range.len() as f32 * fraction;
            if fraction < 1. {
                break;
            }
        }
        (viewed / total).clamp(0., 1.)
    }

    /// Whether source is still waiting to be parsed.
    pub fn is_parsing(&self) -> bool {
        self.parse_task.is_some() || self.pending_source.is_some()
    }

    /// Update the preview after an edit. Unchanged blocks keep their layout
    /// and the scroll position is preserved. During a burst of edits the
    /// preview catches up at most every [`PARSE_INTERVAL`].
    pub fn set_source(&mut self, source: Rope, cx: &mut Context<Self>) {
        self.source = source.clone();
        if !self.active {
            return;
        }
        self.pending_source = Some(source);
        if self.parse_task.is_none() {
            self.schedule_parse(cx);
        }
    }

    /// Show a different document, scrolled to its start.
    pub fn load_source(
        &mut self,
        source: Rope,
        base_dir: Option<Arc<Path>>,
        cx: &mut Context<Self>,
    ) {
        self.base_dir = base_dir;
        self.cancel_parse();
        self.suspended_scroll = None;
        self.pending_reveal_offset = None;
        self.document = Arc::default();
        self.rows = Arc::default();
        self.chunk_lists = false;
        self.list_state.reset(0);
        self.reset_image_cache(cx);
        self.replace_on_next_parse = true;
        // A new document shows at once; only typing is throttled.
        self.last_parse_at = None;
        self.pending_source = None;
        self.set_source(source, cx);
        cx.notify();
    }

    /// Scroll so the block rendered from the source byte `offset` is visible.
    pub fn reveal_offset(&mut self, offset: usize, cx: &mut Context<Self>) {
        let Some(ix) = self.rows.row_for_offset(offset, &self.document) else {
            self.pending_reveal_offset = Some(offset);
            return;
        };
        let before = self.list_state.logical_scroll_top();
        self.list_state.scroll_to_reveal_item(ix);
        let after = self.list_state.logical_scroll_top();
        // Typing inside a visible block moves nothing; skip the re-render.
        if (before.item_ix, before.offset_in_item) != (after.item_ix, after.offset_in_item) {
            cx.notify();
        }
    }

    /// Parse `pending_source` now, or once [`PARSE_INTERVAL`] has passed
    /// since the previous parse started.
    fn schedule_parse(&mut self, cx: &mut Context<Self>) {
        // The executor's clock, so tests can advance it.
        let now = cx.background_executor().now();
        let wait = self.last_parse_at.map_or(Duration::ZERO, |at| {
            PARSE_INTERVAL.saturating_sub(now.saturating_duration_since(at))
        });
        if wait.is_zero() {
            if let Some(source) = self.pending_source.take() {
                self.start_parse(source, cx);
            }
            return;
        }
        self.parse_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(wait).await;
            this.update(cx, |this, cx| {
                this.parse_task = None;
                this.schedule_parse(cx);
            })
            .ok();
        }));
    }

    fn start_parse(&mut self, source: Rope, cx: &mut Context<Self>) {
        self.last_parse_at = Some(cx.background_executor().now());
        let cancel = Arc::new(AtomicBool::new(false));
        self.parse_cancel = Some(cancel.clone());
        let chunk_lists = self.chunk_lists;
        let parse = cx.background_spawn(async move {
            if cancel.load(Ordering::Relaxed) {
                return None;
            }
            let text = source.to_string();
            let document = MarkdownDocument::parse_with_cancellation(&text, || {
                cancel.load(Ordering::Relaxed)
            })?;
            let rows = PreviewRows::new(&document, chunk_lists);
            (!cancel.load(Ordering::Relaxed)).then_some((document, rows))
        });
        self.parse_task = Some(cx.spawn(async move |this, cx| {
            let document = parse.await;
            this.update(cx, |this, cx| {
                this.parse_task = None;
                this.parse_cancel = None;
                if let Some((document, rows)) = document {
                    this.apply_document(document, rows, cx);
                }
                if this.pending_source.is_some() {
                    this.schedule_parse(cx);
                }
            })
            .ok();
        }));
    }

    fn cancel_parse(&mut self) {
        if let Some(cancel) = self.parse_cancel.take() {
            cancel.store(true, Ordering::Relaxed);
        }
        self.parse_task = None;
    }

    fn reset_image_cache(&mut self, cx: &mut Context<Self>) {
        if let Some(cache) = self.image_cache.take() {
            cache.update(cx, |cache, cx| cache.clear(cx));
        }
        self.image_cache = self.active.then(|| PreviewImageCache::new(cx));
    }

    fn apply_document(
        &mut self,
        document: MarkdownDocument,
        rows: PreviewRows,
        cx: &mut Context<Self>,
    ) {
        let row_count = rows.len(&document);
        self.chunk_lists |= rows.is_expanded();
        let mut splice_scroll = None;
        if std::mem::take(&mut self.replace_on_next_parse) {
            self.list_state
                .reset_with_uniform_height(row_count, self.estimated_block_height);
        } else if let Some((old_range, new_range)) =
            self.rows.changed_range(&self.document, &rows, &document)
        {
            if old_range.len() == new_range.len() {
                // Same shape: keep the old heights as hints until remeasured,
                // so the scrollbar does not jump for the frame in between.
                self.list_state.remeasure_items(old_range);
            } else {
                let top = self.list_state.logical_scroll_top();
                if old_range.contains(&top.item_ix) {
                    // Splice normally resets an affected scroll row to the
                    // replacement's beginning. Keep its identity or source
                    // anchor when list chunk boundaries change instead.
                    let id = self.rows.id(top.item_ix, &self.document);
                    let offset = self.rows.source_range(top.item_ix, &self.document).start;
                    splice_scroll = rows
                        .row_for_id(id, &document)
                        .or_else(|| rows.row_for_offset(offset, &document))
                        .map(|item_ix| ListOffset {
                            item_ix,
                            offset_in_item: top.offset_in_item,
                        });
                }
                self.list_state.splice(old_range, new_range.len());
            }
        }
        self.document = Arc::new(document);
        self.rows = Arc::new(rows);
        if let Some(scroll) = splice_scroll {
            self.list_state.scroll_to(scroll);
        }
        if let Some((offset, offset_in_item)) = self.suspended_scroll.take()
            && let Some(item_ix) = self.rows.row_for_offset(offset, &self.document)
        {
            self.list_state.scroll_to(ListOffset {
                item_ix,
                offset_in_item,
            });
        }
        if let Some(offset) = self.pending_reveal_offset.take() {
            if let Some(item_ix) = self.rows.row_for_offset(offset, &self.document) {
                // Fresh rows only have height hints. Anchor the target directly:
                // reveal-item can mistake tall unmeasured rows for visible ones.
                self.list_state.scroll_to(ListOffset {
                    item_ix,
                    offset_in_item: px(0.),
                });
            } else {
                // An empty parse can precede a newer queued source update.
                self.pending_reveal_offset = Some(offset);
            }
        }
        cx.emit(MarkdownPreviewEvent::DocumentChanged);
        cx.notify();
    }

    fn scroll_up(&mut self, _: &ScrollUp, window: &mut Window, cx: &mut Context<Self>) {
        self.list_state.scroll_by(-window.line_height() * 3.);
        cx.notify();
    }

    fn scroll_down(&mut self, _: &ScrollDown, window: &mut Window, cx: &mut Context<Self>) {
        self.list_state.scroll_by(window.line_height() * 3.);
        cx.notify();
    }

    fn scroll_page_up(&mut self, _: &ScrollPageUp, window: &mut Window, cx: &mut Context<Self>) {
        self.list_state.scroll_by(-self.page_height(window));
        cx.notify();
    }

    fn scroll_page_down(
        &mut self,
        _: &ScrollPageDown,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.list_state.scroll_by(self.page_height(window));
        cx.notify();
    }

    fn scroll_to_top(&mut self, _: &ScrollToTop, _: &mut Window, cx: &mut Context<Self>) {
        self.list_state.scroll_to(ListOffset {
            item_ix: 0,
            offset_in_item: px(0.),
        });
        cx.notify();
    }

    fn scroll_to_bottom(&mut self, _: &ScrollToBottom, _: &mut Window, cx: &mut Context<Self>) {
        self.list_state.scroll_to_end();
        cx.notify();
    }

    /// One viewport, less a line of overlap to keep the reader's place.
    fn page_height(&self, window: &Window) -> Pixels {
        let viewport = self.list_state.viewport_bounds().size.height;
        (viewport - window.line_height()).max(window.line_height())
    }
}

impl Drop for MarkdownPreview {
    fn drop(&mut self) {
        self.cancel_parse();
    }
}

impl Focusable for MarkdownPreview {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for MarkdownPreview {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Track the rem size so unmeasured blocks get a proportional hint.
        self.estimated_block_height = window.rem_size() * 3.;

        let document = self.document.clone();
        let rows = self.rows.clone();
        let layout = BlockLayout {
            font_family: self
                .font_family
                .clone()
                .unwrap_or_else(|| cx.theme().font_family.clone()),
            font_scale: self.font_scale,
            column_insets: self.column_insets,
        };
        let base_dir = self.base_dir.clone();
        let is_empty = document.blocks().is_empty() && !self.is_parsing();
        let cache = self.image_cache.clone();
        if let Some(cache) = &cache {
            cache.update(cx, |cache, _| cache.begin_frame());
        }
        let finish_cache = cache.clone();
        let theme = cx.theme();
        let content = div()
            .id("markdown-preview")
            .key_context(KEY_CONTEXT)
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::scroll_up))
            .on_action(cx.listener(Self::scroll_down))
            .on_action(cx.listener(Self::scroll_page_up))
            .on_action(cx.listener(Self::scroll_page_down))
            .on_action(cx.listener(Self::scroll_to_top))
            .on_action(cx.listener(Self::scroll_to_bottom))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| this.focus_handle.focus(window, cx)),
            )
            .relative()
            .size_full()
            .min_h_0()
            .overflow_hidden()
            .bg(theme.background)
            .text_color(theme.foreground)
            .when(is_empty, |this| {
                this.flex().child(
                    Empty::new().header(
                        EmptyHeader::new()
                            .media(
                                EmptyMedia::new()
                                    .with_variant(EmptyMediaVariant::Icon)
                                    .child(Icon::new(IconName::BookOpen).small()),
                            )
                            .title(EmptyTitle::new().child("Nothing to preview"))
                            .description(
                                EmptyDescription::new()
                                    .child("Markdown you write appears here as you type."),
                            ),
                    ),
                )
            })
            .when(!is_empty, |this| {
                this.child(
                    list(self.list_state.clone(), move |ix, _, cx| {
                        let Some(cache) = &cache else {
                            return div().into_any_element();
                        };
                        render_block(
                            &document.blocks()[rows.block_ix(ix)],
                            rows.id(ix, &document),
                            rows.list_items(ix),
                            base_dir.as_ref(),
                            cache,
                            &layout,
                            cx,
                        )
                    })
                    .size_full()
                    .pt_10()
                    .pb_16(),
                )
            });
        content.when_some(finish_cache, |this, cache| {
            this.child(
                canvas(
                    |_, _, _| {},
                    move |_, _, window, cx| {
                        cache.update(cx, |cache, cx| cache.finish_frame(window, cx));
                    },
                )
                .absolute()
                .size_0(),
            )
        })
    }
}

/// The preview's scrollbar; see [`MarkdownPreview::pane`].
pub struct PreviewScrollbar {
    handle: PreviewScrollHandle,
    preview: WeakEntity<MarkdownPreview>,
}

impl Render for PreviewScrollbar {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let moved = self.handle.moved.clone();
        let preview = self.preview.clone();
        div()
            .size_full()
            // The scrollbar notifies only this view when it scrolls the list.
            // Redraw the blocks in the same event. Painted before the
            // scrollbar, so these run after its own listeners.
            .child(
                canvas(
                    |_, _, _| {},
                    move |_, _, window, _| {
                        let notify_if_moved = {
                            let moved = moved.clone();
                            let preview = preview.clone();
                            move |cx: &mut App| {
                                if moved.take() {
                                    preview.update(cx, |_, cx| cx.notify()).ok();
                                }
                            }
                        };
                        let on_move = notify_if_moved.clone();
                        window.on_mouse_event(move |_: &MouseMoveEvent, _, _, cx| on_move(cx));
                        let on_down = notify_if_moved.clone();
                        window.on_mouse_event(move |_: &MouseDownEvent, _, _, cx| on_down(cx));
                        window
                            .on_mouse_event(move |_: &MouseUpEvent, _, _, cx| notify_if_moved(cx));
                    },
                )
                .absolute()
                .size_0(),
            )
            .child(
                Scrollbar::vertical(&self.handle)
                    .id("markdown-preview-scrollbar")
                    .viewport_from_layout(),
            )
    }
}

/// The list's scroll handle, noting when the scrollbar moves it.
#[derive(Clone)]
struct PreviewScrollHandle {
    list_state: ListState,
    moved: Rc<Cell<bool>>,
}

impl ScrollbarHandle for PreviewScrollHandle {
    fn viewport_bounds(&self) -> Bounds<Pixels> {
        ScrollbarHandle::viewport_bounds(&self.list_state)
    }

    fn offset(&self) -> Point<Pixels> {
        ScrollbarHandle::offset(&self.list_state)
    }

    fn set_offset(&self, offset: Point<Pixels>) {
        ScrollbarHandle::set_offset(&self.list_state, offset);
        self.moved.set(true);
    }

    fn content_size(&self) -> Size<Pixels> {
        ScrollbarHandle::content_size(&self.list_state)
    }

    fn start_drag(&self) {
        ScrollbarHandle::start_drag(&self.list_state);
    }

    fn end_drag(&self) {
        ScrollbarHandle::end_drag(&self.list_state);
    }
}

#[cfg(test)]
mod tests {
    use gpui_kit::{TestAppContext, VisualTestContext};

    use super::*;

    fn rope(text: &str) -> Rope {
        Rope::from(text)
    }

    #[gpui_kit::test]
    async fn applies_the_latest_source_after_a_burst_of_edits(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let window = cx.add_window(|_, cx| MarkdownPreview::new(cx));
        let preview = window.root(cx).unwrap();
        let cx = &mut VisualTestContext::from_window(*window, cx);

        preview.update(cx, |preview, cx| {
            preview.load_source(rope("# One\n\nbody"), None, cx);
            preview.set_source(rope("# One\n\nbody\n\nmore"), cx);
            preview.set_source(rope("# One\n\nbody\n\nmore\n\nlast"), cx);
        });
        cx.run_until_parked();
        // Edits right after a parse wait out the throttle.
        assert!(preview.read_with(cx, |preview, _| preview.is_parsing()));
        cx.executor().advance_clock(PARSE_INTERVAL);
        cx.run_until_parked();

        preview.read_with(cx, |preview, _| {
            assert!(!preview.is_parsing());
            assert_eq!(preview.document().blocks().len(), 4);
            assert_eq!(preview.list_state.item_count(), 4);
        });
    }

    #[gpui_kit::test]
    async fn an_edit_keeps_the_list_in_step_with_the_document(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let window = cx.add_window(|_, cx| MarkdownPreview::new(cx));
        let preview = window.root(cx).unwrap();
        let cx = &mut VisualTestContext::from_window(*window, cx);

        preview.update(cx, |preview, cx| {
            preview.load_source(rope("a\n\nb\n\nc"), None, cx);
        });
        cx.run_until_parked();
        preview.update(cx, |preview, cx| preview.set_source(rope("a\n\nc"), cx));
        cx.executor().advance_clock(PARSE_INTERVAL);
        cx.run_until_parked();

        preview.read_with(cx, |preview, _| {
            assert_eq!(preview.document().blocks().len(), 2);
            assert_eq!(preview.list_state.item_count(), 2);
        });
    }
}
