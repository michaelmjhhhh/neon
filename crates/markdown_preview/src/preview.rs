use std::path::Path;
use std::sync::Arc;

use gpui_kit::component::ActiveTheme as _;
use gpui_kit::component::input::Rope;
use gpui_kit::component::scroll::Scrollbar;
use gpui_kit::{
    App, AppContext as _, Context, EventEmitter, FocusHandle, Focusable, InteractiveElement as _,
    IntoElement, ListAlignment, ListOffset, ListState, MouseButton, ParentElement as _, Pixels,
    Render, Styled as _, Task, Window, div, list, prelude::FluentBuilder as _, px,
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
    focus_handle: FocusHandle,
    /// Directory that relative image paths and links resolve against.
    base_dir: Option<Arc<Path>>,
    /// The parse in flight. At most one runs; newer source waits in
    /// `pending_source` and only the latest is kept.
    parse_task: Option<Task<()>>,
    pending_source: Option<Rope>,
    /// The next applied parse replaces the document instead of patching it.
    replace_on_next_parse: bool,
    /// Height hint for blocks that have not been measured yet.
    estimated_block_height: Pixels,
}

impl EventEmitter<MarkdownPreviewEvent> for MarkdownPreview {}

impl MarkdownPreview {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            document: Arc::default(),
            font_family: None,
            font_scale: 1.,
            list_state: ListState::new(0, ListAlignment::Top, OVERDRAW),
            focus_handle: cx.focus_handle(),
            base_dir: None,
            parse_task: None,
            pending_source: None,
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
        self.parse_task.is_some()
    }

    /// Update the preview after an edit. Unchanged blocks keep their layout
    /// and the scroll position is preserved.
    pub fn set_source(&mut self, source: Rope, cx: &mut Context<Self>) {
        if self.parse_task.is_some() {
            self.pending_source = Some(source);
        } else {
            self.start_parse(source, cx);
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
        self.set_source(source, cx);
    }

    /// Scroll so the block rendered from the source byte `offset` is visible.
    pub fn reveal_offset(&mut self, offset: usize, cx: &mut Context<Self>) {
        if let Some(ix) = self.document.block_ix_for_offset(offset) {
            self.list_state.scroll_to_reveal_item(ix);
            cx.notify();
        }
    }

    fn start_parse(&mut self, source: Rope, cx: &mut Context<Self>) {
        let parse = cx.background_spawn(async move {
            let text = source.to_string();
            MarkdownDocument::parse(&text)
        });
        self.parse_task = Some(cx.spawn(async move |this, cx| {
            let document = parse.await;
            this.update(cx, |this, cx| {
                this.parse_task = None;
                this.apply_document(document, cx);
                if let Some(source) = this.pending_source.take() {
                    this.start_parse(source, cx);
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
            .rounded(theme.radius_tokens().lg)
            .border_1()
            .border_color(theme.input)
            .overflow_hidden()
            .bg(theme.background)
            .text_color(theme.foreground)
            .when(is_empty, |this| {
                this.flex()
                    .items_center()
                    .justify_center()
                    .text_sm()
                    .text_color(theme.muted_foreground)
                    .child("Nothing to preview")
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
                    .py_6(),
                )
                .child(
                    div().absolute().inset_0().child(
                        Scrollbar::vertical(&self.list_state)
                            .id("markdown-preview-scrollbar")
                            .viewport_from_layout(),
                    ),
                )
            })
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
        cx.run_until_parked();

        preview.read_with(cx, |preview, _| {
            assert_eq!(preview.document().blocks().len(), 2);
            assert_eq!(preview.list_state.item_count(), 2);
        });
    }
}
