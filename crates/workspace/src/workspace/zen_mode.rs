//! Zen mode in the window: entering and leaving it, and its one view.

use std::time::Instant;

use gpui_kit::assets::IconName as AssetIcon;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::RopeExt as _;
use gpui_kit::component::tab::{Tab, TabBar};
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, TitleBar};
use gpui_kit::{
    AnyElement, App, Context, EntityInputHandler as _, InteractiveElement as _, IntoElement,
    MouseButton, MouseDownEvent, MouseUpEvent, ParentElement as _, Pixels, ScrollWheelEvent,
    StyleRefinement, Styled as _, Window, div, point, px,
};
use markdown_preview::MarkdownPreview;

use super::{ExitZen, LayoutMode, SHEET_HEADER_REMS, ToggleZen, Workspace};
use crate::KEY_CONTEXT;
use crate::zen::{self, Zen, ZenFocus, ZenView};

impl Workspace {
    pub(super) fn toggle_zen(
        &mut self,
        _: &ToggleZen,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.zen.is_some() {
            self.exit_zen(window, cx);
        } else {
            self.enter_zen(window, cx);
        }
    }

    /// Escape leaves Zen; elsewhere it is left to whatever else wants it.
    pub(super) fn exit_zen_action(
        &mut self,
        _: &ExitZen,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.zen.is_some() {
            self.exit_zen(window, cx);
        } else {
            cx.propagate();
        }
    }

    /// Show the current document alone, full screen: as source, or as the
    /// preview when that is the layout Zen starts from.
    fn enter_zen(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.zen.is_some() {
            return;
        }
        let exits_fullscreen = !window.is_fullscreen();
        if exits_fullscreen {
            window.toggle_fullscreen();
        }
        let view = if self.layout == LayoutMode::Preview {
            ZenView::Preview
        } else {
            ZenView::Editor
        };
        self.zen = Some(Zen {
            exits_fullscreen,
            view,
            pointer_down: false,
            gliding: false,
            last_glide: None,
        });
        // Zen covers Settings rather than waiting behind them.
        self.set_settings_open(false, window, cx);
        self.show_zen_view(window, cx);
        self.show_zen_in_editors(window, cx);
    }

    /// Switch Zen between the source and the preview.
    pub(super) fn set_zen_view(
        &mut self,
        view: ZenView,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(zen) = &mut self.zen else {
            return;
        };
        if zen.view == view {
            return;
        }
        zen.view = view;
        self.show_zen_view(window, cx);
    }

    /// Turn the preview on or off for Zen's view, keep it on the caret's
    /// block, and give the view focus.
    fn show_zen_view(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let preview_shown = self.preview_shown();
        let cursor = self.tab().editor.read(cx).cursor();
        self.tab().preview.update(cx, |preview, cx| {
            preview.set_active(preview_shown, cx);
            if preview_shown {
                preview.reveal_offset(cursor, cx);
            }
        });
        self.focus_content(window, cx);
        if !preview_shown {
            self.glide_zen_caret(window, cx);
        }
        cx.notify();
    }

    /// Glide the caret line to the middle of the screen, one step a frame.
    /// Each step aims from the latest layout, so a click, a keystroke, a
    /// rewrap or the full screen transition all end with it centered.
    pub(super) fn glide_zen_caret(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(zen) = &mut self.zen else {
            return;
        };
        if zen.view != ZenView::Editor || zen.pointer_down || zen.gliding {
            return;
        }
        zen.gliding = true;
        zen.last_glide = None;
        cx.on_next_frame(window, |this, window, cx| {
            this.glide_zen_caret_step(window, cx)
        });
    }

    fn glide_zen_caret_step(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(zen) = &mut self.zen else {
            return;
        };
        if zen.view != ZenView::Editor || zen.pointer_down || !zen.gliding {
            zen.gliding = false;
            return;
        }
        let now = Instant::now();
        let last_glide = zen.last_glide;
        // Without motion, the line lands in one step.
        let share = if cx.reduce_motion() {
            1.
        } else {
            let elapsed = last_glide.map_or(1. / 60., |(at, _)| (now - at).as_secs_f32());
            1. - (-elapsed / zen::GLIDE_SECONDS).exp()
        };
        let stepped = self.tab().editor.update(cx, |editor, cx| {
            let content = editor.text_bounds()?;
            let cursor = editor.text().offset_to_offset_utf16(editor.cursor());
            let caret = editor.bounds_for_range(cursor..cursor, content, window, cx)?;
            let offset = editor.scroll_offset();
            // The editor stops scrolling at the top and bottom of the text,
            // where the line cannot reach the middle.
            let stuck = last_glide.is_some_and(|(_, last)| (offset.y - last).abs() < px(0.01));
            let shift = editor.input_bounds().center().y - caret.center().y;
            // Near the top the line cannot reach the middle. An offset past
            // the top would draw for a frame before the editor clamps it.
            let remaining = (offset.y + shift).min(px(0.)) - offset.y;
            if remaining.abs() < px(1.) || stuck {
                return None;
            }
            editor.set_scroll_offset(point(offset.x, offset.y + remaining * share), cx);
            Some(offset.y)
        });
        let Some(zen) = &mut self.zen else {
            return;
        };
        match stepped {
            Some(offset) => {
                zen.last_glide = Some((now, offset));
                cx.on_next_frame(window, |this, window, cx| {
                    this.glide_zen_caret_step(window, cx)
                });
            }
            None => zen.gliding = false,
        }
    }

    /// Stop gliding, for a scroll the user makes.
    fn stop_zen_glide(&mut self) {
        if let Some(zen) = &mut self.zen {
            zen.gliding = false;
        }
    }

    fn set_zen_pointer_down(&mut self, down: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(zen) = &mut self.zen else {
            return;
        };
        zen.pointer_down = down;
        if down {
            zen.gliding = false;
        } else {
            self.glide_zen_caret(window, cx);
        }
    }

    /// Return to the layout and window size Zen started from.
    pub(super) fn exit_zen(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(zen) = self.zen.take() else {
            return;
        };
        // Only if Zen made it full screen and it still is.
        if zen.exits_fullscreen && window.is_fullscreen() {
            window.toggle_fullscreen();
        }
        let preview_shown = self.preview_shown();
        let cursor = self.tab().editor.read(cx).cursor();
        self.tab().preview.update(cx, |preview, cx| {
            preview.set_active(preview_shown, cx);
            if preview_shown {
                preview.reveal_offset(cursor, cx);
            }
        });
        self.show_zen_in_editors(window, cx);
        self.focus_content(window, cx);
        cx.notify();
    }

    /// What Zen shows, or `None` outside Zen.
    pub(super) fn zen_view(&self) -> Option<ZenView> {
        self.zen.as_ref().map(|zen| zen.view)
    }

    /// Whether the active tab's preview is on screen.
    pub(super) fn preview_shown(&self) -> bool {
        !self.settings_open
            && match self.zen_view() {
                Some(view) => view == ZenView::Preview,
                None => self.layout != LayoutMode::Editor,
            }
    }

    /// Bring every tab in line with Zen: no line numbers, only the caret's
    /// paragraph at full ink, and the preview's text between the same edges
    /// as the editor's. Out of Zen, undo each.
    pub(super) fn show_zen_in_editors(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let in_zen = self.zen.is_some();
        let line_numbers = self.line_numbers && !in_zen;
        let preview_insets = in_zen.then_some((zen::TEXT_LEADING, zen::TEXT_TRAILING));
        for tab in &mut self.tabs {
            tab.editor.update(cx, |editor, cx| {
                editor.set_line_number(line_numbers, window, cx);
            });
            tab.preview.update(cx, |preview, cx| {
                preview.set_column_insets(preview_insets, cx)
            });
            if in_zen {
                let editor = &tab.editor;
                let focus = tab
                    .zen_focus
                    .get_or_insert_with(|| ZenFocus::new(editor, cx));
                focus.follow(editor, cx);
            } else if let Some(focus) = &tab.zen_focus {
                focus.clear(cx);
            }
        }
    }

    /// Light the paragraph the caret moved into. An edit can also join or
    /// split paragraphs without moving the caret.
    pub(super) fn zen_follow_cursor(&self, cx: &mut App) {
        if self.zen.is_none() {
            return;
        }
        if let Some(focus) = &self.tab().zen_focus {
            focus.follow(&self.tab().editor, cx);
        }
    }

    /// The column's width: a share of the window, but never narrower than
    /// the measure in the editor's font plus the editor's insets beside the
    /// text.
    fn zen_column_width(&self, window: &Window, cx: &App) -> Pixels {
        let typography = self.typography.read(cx);
        let font = gpui_kit::font(typography.editor_font(cx));
        let size = window.rem_size() * (typography.editor_size(cx) / 16.);
        let text_system = window.text_system();
        let ch = text_system
            .ch_width(text_system.resolve_font(&font), size)
            .unwrap_or(size * 0.6);
        let measure = ch * zen::MEASURE_CHARS + zen::TEXT_LEADING + zen::TEXT_TRAILING;
        measure.max(window.viewport_size().width * zen::COLUMN_SHARE)
    }

    /// Scroll the shown view from the margins beside it, so the pointer can
    /// rest anywhere on the screen.
    fn scroll_zen_view(
        &mut self,
        event: &ScrollWheelEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.stop_zen_glide();
        match self.zen_view() {
            Some(ZenView::Editor) => self.tab().editor.update(cx, |editor, cx| {
                let Some(line_height) = editor.line_height() else {
                    return;
                };
                let delta = event.delta.pixel_delta(line_height);
                let offset = editor.scroll_offset();
                editor.set_scroll_offset(point(offset.x, offset.y + delta.y), cx);
            }),
            Some(ZenView::Preview) => {
                let delta = event.delta.pixel_delta(window.line_height());
                self.tab()
                    .preview
                    .update(cx, |preview, cx| preview.scroll_by(-delta.y, cx));
            }
            None => {}
        }
    }

    /// The document on the sheet's surface, edge to edge: a strip to drag
    /// the window by, the column with the source or the preview, and one
    /// quiet line beneath it that shares the column's edges.
    pub(super) fn render_zen(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let rem = window.rem_size();
        let band = rem * SHEET_HEADER_REMS;
        let column = self.zen_column_width(window, cx);
        let view = self.zen_view().unwrap_or(ZenView::Editor);
        let theme = cx.theme();

        let margin = |id: &'static str, cx: &mut Context<Self>| {
            div()
                .id(id)
                .flex_1()
                .min_w_8()
                .h_full()
                .on_scroll_wheel(cx.listener(Self::scroll_zen_view))
        };

        let body: AnyElement = match view {
            ZenView::Editor => self
                .tab()
                .editor_pane
                .clone()
                .cached(StyleRefinement::default().size_full())
                .into_any_element(),
            ZenView::Preview => MarkdownPreview::pane(&self.tab().preview, cx).into_any_element(),
        };

        // Zen's two views, named and drawn as in the header's layout control.
        let views = [
            (ZenView::Editor, LayoutMode::Editor),
            (ZenView::Preview, LayoutMode::Preview),
        ];
        let view_switch = TabBar::new("zen-view")
            .segmented()
            .small()
            .children(views.map(|(_, mode)| {
                Tab::new().aria_label(mode.label()).child(
                    div()
                        .flex()
                        .items_center()
                        .gap_1()
                        .text_xs()
                        .child(Icon::new(mode.icon()).xsmall())
                        .child(mode.label()),
                )
            }))
            .selected_index(views.iter().position(|(v, _)| *v == view).unwrap_or(0))
            .on_click(cx.listener(move |this, ix: &usize, window, cx| {
                if let Some((view, _)) = views.get(*ix) {
                    this.set_zen_view(*view, window, cx);
                }
            }));

        let footer = div()
            .flex_none()
            .h(band)
            .flex()
            .items_center()
            .justify_between()
            // The controls line up with the text above them.
            .pl(zen::TEXT_LEADING)
            .pr(zen::TEXT_TRAILING)
            .child(view_switch)
            .child(
                Button::new("exit-zen")
                    .xsmall()
                    .ghost()
                    .label("Exit Zen Mode")
                    .tooltip_with_action("Exit Zen Mode", &ToggleZen, Some(KEY_CONTEXT))
                    .on_click(cx.listener(|this, _, window, cx| this.exit_zen(window, cx))),
            );

        div()
            .id("zen")
            .size_full()
            .flex()
            .flex_col()
            .bg(theme.background)
            // The glide waits for the pointer, then centers where it clicked.
            .capture_any_mouse_down(cx.listener(|this, event: &MouseDownEvent, window, cx| {
                if event.button == MouseButton::Left {
                    this.set_zen_pointer_down(true, window, cx);
                }
            }))
            .capture_any_mouse_up(cx.listener(|this, event: &MouseUpEvent, window, cx| {
                if event.button == MouseButton::Left {
                    this.set_zen_pointer_down(false, window, cx);
                }
            }))
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| this.set_zen_pointer_down(false, window, cx)),
            )
            .child(
                TitleBar::new()
                    .h(band)
                    .bg(gpui_kit::transparent_black())
                    .border_color(gpui_kit::transparent_black()),
            )
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .child(margin("zen-leading", cx))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .w(column)
                            .min_w_0()
                            .h_full()
                            .child(
                                div()
                                    .id("zen-body")
                                    .flex_1()
                                    .min_h_0()
                                    // A scroll of its own ends the glide.
                                    .on_scroll_wheel(
                                        cx.listener(|this, _, _, _| this.stop_zen_glide()),
                                    )
                                    .child(body),
                            )
                            .child(footer),
                    )
                    .child(margin("zen-trailing", cx)),
            )
    }
}

/// The header's Zen control.
pub(super) fn enter_zen_button(cx: &Context<Workspace>) -> Button {
    Button::new("enter-zen")
        .small()
        .ghost()
        .icon(Icon::new(AssetIcon::Focus))
        .accessibility_label("Enter Zen Mode")
        .tooltip_with_action("Enter Zen Mode", &ToggleZen, Some(KEY_CONTEXT))
        .on_click(cx.listener(|this, _, window, cx| this.enter_zen(window, cx)))
}
