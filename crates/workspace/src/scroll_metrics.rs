use std::time::Duration;
#[cfg(feature = "diagnostics")]
use std::time::Instant;

use gpui_kit::component::input::{EditorState, RopeExt as _};
use gpui_kit::{
    Context, Entity, IntoElement, ParentElement as _, Render, Styled as _, Task, Window, div, px,
};
use markdown_preview::MarkdownPreview;

use crate::LayoutMode;

const SCROLL_SAMPLE_INTERVAL: Duration = Duration::from_millis(100);
#[cfg(feature = "diagnostics")]
const FPS_SAMPLE_INTERVAL: Duration = Duration::from_millis(500);

/// A separate view so sampling the counters does not invalidate the editor.
pub(crate) struct ScrollMetrics {
    editor: Entity<EditorState>,
    preview: Entity<MarkdownPreview>,
    layout: LayoutMode,
    preview_active: bool,
    offsets: [f32; 2],
    viewed_percent: u8,
    fps: Option<u64>,
    #[cfg(feature = "diagnostics")]
    last_frame_count: u64,
    #[cfg(feature = "diagnostics")]
    last_frame_sample: Instant,
    _sample_task: Task<()>,
}

impl ScrollMetrics {
    pub(crate) fn new(
        editor: Entity<EditorState>,
        preview: Entity<MarkdownPreview>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let task = cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor().timer(SCROLL_SAMPLE_INTERVAL).await;
                if this
                    .update_in(cx, |this, window, cx| this.sample(window, cx))
                    .is_err()
                {
                    break;
                }
            }
        });
        Self {
            editor,
            preview,
            layout: LayoutMode::default(),
            preview_active: false,
            offsets: [0.; 2],
            viewed_percent: 100,
            fps: None,
            #[cfg(feature = "diagnostics")]
            last_frame_count: presented_frames(window),
            #[cfg(feature = "diagnostics")]
            last_frame_sample: Instant::now(),
            _sample_task: task,
        }
    }

    pub(crate) fn set_layout(&mut self, layout: LayoutMode, cx: &mut Context<Self>) {
        self.layout = layout;
        self.preview_active = layout == LayoutMode::Preview;
        self.update_viewed_percent(cx);
        cx.notify();
    }

    fn sample(&mut self, _window: &Window, cx: &mut Context<Self>) {
        let offsets = [
            f32::from((-self.editor.read(cx).scroll_offset().y).max(px(0.))),
            f32::from(self.preview.read(cx).scroll_offset()),
        ];
        let previous = (self.preview_active, self.viewed_percent, self.fps);
        self.preview_active = match self.layout {
            LayoutMode::Editor => false,
            LayoutMode::Preview => true,
            // Editor cursor movement can scroll both panes; prefer the editor
            // in that case. Independent preview scrolling selects the preview.
            LayoutMode::Split if offsets[0] != self.offsets[0] => false,
            LayoutMode::Split if offsets[1] != self.offsets[1] => true,
            LayoutMode::Split => self.preview_active,
        };
        self.offsets = offsets;
        self.update_viewed_percent(cx);

        #[cfg(feature = "diagnostics")]
        {
            let now = Instant::now();
            let elapsed = now.duration_since(self.last_frame_sample);
            if elapsed >= FPS_SAMPLE_INTERVAL {
                let count = presented_frames(_window);
                self.fps = Some(
                    (count.saturating_sub(self.last_frame_count) as f64 / elapsed.as_secs_f64())
                        .round() as u64,
                );
                self.last_frame_count = count;
                self.last_frame_sample = now;
            }
        }
        if previous != (self.preview_active, self.viewed_percent, self.fps) {
            cx.notify();
        }
    }

    fn update_viewed_percent(&mut self, cx: &Context<Self>) {
        let fraction = if self.preview_active {
            self.preview.read(cx).viewed_fraction()
        } else {
            editor_viewed_fraction(self.editor.read(cx))
        };
        // Do not round a partially visible final line up to 100%.
        self.viewed_percent = (fraction.clamp(0., 1.) * 100.).floor() as u8;
    }
}

impl Render for ScrollMetrics {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let panel = if self.preview_active {
            "Preview"
        } else {
            "Editor"
        };
        #[cfg(feature = "diagnostics")]
        let fps = self
            .fps
            .map_or_else(|| "— FPS".to_string(), |fps| format!("{fps} FPS"));
        let metrics = div().flex().items_center().gap_3().child(
            div()
                .min_w_24()
                .child(format!("{panel} {}%", self.viewed_percent)),
        );
        #[cfg(feature = "diagnostics")]
        let metrics = metrics.child(div().min_w_16().child(fps));
        metrics
    }
}

/// Use source ranges, checking their laid-out bounds to exclude the editor's
/// overscan rows and account for a partially visible wrapped line.
fn editor_viewed_fraction(editor: &EditorState) -> f32 {
    let text = editor.text();
    if text.len() == 0 {
        return 1.;
    }
    let Some(visible) = editor.visible_row_range() else {
        return 0.;
    };
    // Range bounds are in unscrolled content coordinates.
    let viewport_bottom = editor.input_bounds().bottom() - editor.scroll_offset().y;
    for row in visible.rev() {
        let range = text.line_start_offset(row)..text.line_end_offset(row);
        let Some(bounds) = editor.range_to_bounds(&range) else {
            continue;
        };
        if bounds.top() >= viewport_bottom || bounds.size.height <= px(0.) {
            continue;
        }
        let fraction = ((viewport_bottom - bounds.top()) / bounds.size.height).clamp(0., 1.);
        let viewed = range.start as f32 + range.len() as f32 * fraction;
        return (viewed / text.len() as f32).clamp(0., 1.);
    }
    0.
}

/// Count presentations, not render calls or requested animation callbacks.
/// The profiler's per-window counters work without enabling trace collection.
#[cfg(feature = "diagnostics")]
fn presented_frames(window: &Window) -> u64 {
    window
        .frame_duration_snapshot()
        .by_refresh_interval
        .iter()
        .map(|frames| frames.frames_on_time + frames.frames_late + frames.frames_unmeasured)
        .sum()
}
