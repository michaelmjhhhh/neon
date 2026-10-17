use std::cell::Cell;
use std::path::Path;
use std::rc::Rc;
use std::sync::Arc;
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
use markdown::{BlockSplice, MarkdownDocument};

use crate::KEY_CONTEXT;
use crate::blocks::render_block;

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
    font_family: Option<gpui_kit::SharedString>,
    font_scale: f32,
    list_state: ListState,
    scrollbar: Entity<PreviewScrollbar>,
    focus_handle: FocusHandle,
    /// Directory that relative image paths and links resolve against.
    base_dir: Option<Arc<Path>>,
    /// The parse in flight, or the wait before the next one. At most one
    /// runs; newer source waits in `pending_source` and only the latest is
    /// kept.
    parse_task: Option<Task<()>>,
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
            font_family: None,
            font_scale: 1.,
            list_state,
            scrollbar,
            focus_handle: cx.focus_handle(),
            base_dir: None,
            parse_task: None,
            pending_source: None,
            last_parse_at: None,
            replace_on_next_parse: false,
            estimated_block_height: px(48.),
        }
    }

    /// Change document typography and invalidate cached block heights.
    pub fn set_typography(
        &mut self,
        family: gpui_kit::SharedString,
        scale: f32,
        cx: &mut Context<Self>,
    ) {
        self.font_family = Some(family);
        self.font_scale = scale.clamp(0.75, 1.5);
        self.list_state
            .remeasure_items(0..self.document.blocks().len());
        cx.notify();
    }

    /// Update relative resources after Save As without moving the reader.
    pub fn set_base_dir(&mut self, base_dir: Option<Arc<Path>>, cx: &mut Context<Self>) {
        self.base_dir = base_dir;
        self.list_state
            .remeasure_items(0..self.document.blocks().len());
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

    /// The most recently applied parse.
    pub fn document(&self) -> &Arc<MarkdownDocument> {
        &self.document
    }

    /// Vertical scroll distance from the top, using the scrollbar's measured offset.
    pub fn scroll_offset(&self) -> Pixels {
        (-self.list_state.scroll_px_offset_for_scrollbar().y).max(px(0.))
    }

    /// Fraction of the source reached by the bottom of the viewport.
    /// Source ranges keep progress independent of unmeasured block heights.
    pub fn viewed_fraction(&self) -> f32 {
        let blocks = self.document.blocks();
        let Some(last) = blocks.last() else {
            return 1.;
        };
        let total = last.source_range().end.max(1) as f32;
        let viewport_bottom = self.list_state.viewport_bounds().bottom();
        let first = self.list_state.logical_scroll_top().item_ix;
        let mut viewed = blocks
            .get(first)
            .map_or(0., |block| block.source_range().start as f32);
        for (ix, block) in blocks.iter().enumerate().skip(first) {
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
            let range = block.source_range();
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
        self.replace_on_next_parse = true;
        // A new document shows at once; only typing is throttled.
        self.last_parse_at = None;
        self.parse_task = None;
        self.set_source(source, cx);
    }

    /// Scroll so the block rendered from the source byte `offset` is visible.
    pub fn reveal_offset(&mut self, offset: usize, cx: &mut Context<Self>) {
        let Some(ix) = self.document.block_ix_for_offset(offset) else {
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
        let parse = cx.background_spawn(async move {
            let text = source.to_string();
            MarkdownDocument::parse(&text)
        });
        self.parse_task = Some(cx.spawn(async move |this, cx| {
            let document = parse.await;
            this.update(cx, |this, cx| {
                this.parse_task = None;
                this.apply_document(document, cx);
                if this.pending_source.is_some() {
                    this.schedule_parse(cx);
                }
            })
            .ok();
        }));
    }

    fn apply_document(&mut self, document: MarkdownDocument, cx: &mut Context<Self>) {
        let block_count = document.blocks().len();
        if std::mem::take(&mut self.replace_on_next_parse) {
            self.list_state
                .reset_with_uniform_height(block_count, self.estimated_block_height);
        } else if let Some(splice) = BlockSplice::between(self.document.blocks(), document.blocks())
        {
            let (old_range, new_range) = (splice.old_range(), splice.new_range());
            if old_range.len() == new_range.len() {
                // Same shape: keep the old heights as hints until remeasured,
                // so the scrollbar does not jump for the frame in between.
                self.list_state.remeasure_items(old_range);
            } else {
                self.list_state.splice(old_range, new_range.len());
            }
        }
        self.document = Arc::new(document);
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
        let font_family = self
            .font_family
            .clone()
            .unwrap_or_else(|| cx.theme().font_family.clone());
        let font_scale = self.font_scale;
        let base_dir = self.base_dir.clone();
        let is_empty = document.blocks().is_empty() && !self.is_parsing();
        let theme = cx.theme();

        div()
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
                    list(self.list_state.clone(), move |ix, window, cx| {
                        render_block(
                            &document.blocks()[ix],
                            base_dir.as_ref(),
                            font_family.clone(),
                            font_scale,
                            window,
                            cx,
                        )
                    })
                    .size_full()
                    .pt_10()
                    .pb_16(),
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
