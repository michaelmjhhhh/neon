//! Zen mode in the window: entering and leaving it, and its one view.

use std::time::Instant;

use gpui_kit::assets::IconName as AssetIcon;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::RopeExt as _;
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, TitleBar};
use gpui_kit::{
    Animation, AnimationExt as _, AnyElement, App, Context, EntityInputHandler as _,
    InteractiveElement as _, IntoElement, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, ParentElement as _, Pixels, ScrollWheelEvent, StyleRefinement, Styled as _,
    Window, div, ease_in_out, point, px,
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

    /// Show the current document alone, full screen, as source to write in,
    /// whatever the layout. The preview is a shortcut away.
    fn enter_zen(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.zen.is_some() {
            return;
        }
        let exits_fullscreen = !window.is_fullscreen();
        if exits_fullscreen {
            window.toggle_fullscreen();
        }
        self.zen = Some(Zen {
            exits_fullscreen,
            view: ZenView::Editor,
            pointer_down: false,
            gliding: false,
            last_glide: None,
            lift: px(0.),
            chrome_hidden: false,
            pointer_at: None,
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
        self.sync_previews(cx);
        let preview_shown = self.preview_shown();
        let cursor = self.tab().editor.read(cx).cursor();
        self.tab().preview.update(cx, |preview, cx| {
            if preview_shown {
                preview.reveal_offset(cursor, cx);
            }
        });
        self.focus_content(window, cx);
        if !preview_shown {
            self.glide_zen_caret_once_drawn(window, cx);
        }
        cx.notify();
    }

    /// Glide now, and again once the editor is drawn: an editor that was not
    /// on screen, behind the preview or in another tab, has no layout to
    /// measure the caret in until then.
    pub(super) fn glide_zen_caret_once_drawn(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.zen.is_none() {
            return;
        }
        self.glide_zen_caret(window, cx);
        cx.on_next_frame(window, |this, window, cx| this.glide_zen_caret(window, cx));
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
        let lift = zen.lift;
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
            // The editor clamps an offset past the end of its text at its
            // next layout, where the line cannot reach the middle.
            let clamped = last_glide.is_some_and(|(_, asked)| (offset.y - asked).abs() > px(0.01));
            // The lift shortens the editor from the top, moving its center
            // half as far; the middle stays the column's.
            let middle = editor.input_bounds().center().y - lift / 2.;
            let remaining = middle - caret.center().y;
            if remaining.abs() < px(1.) || clamped {
                return None;
            }
            // Where the text sits: scrolled up while below zero, lowered by
            // the lift above it. An offset past the top would draw for a
            // frame before the editor clamps it.
            let position = offset.y + lift + remaining * share;
            let scroll = position.min(px(0.));
            editor.set_scroll_offset(point(offset.x, scroll), cx);
            Some((scroll, position.max(px(0.))))
        });
        let Some(zen) = &mut self.zen else {
            return;
        };
        match stepped {
            Some((offset, lift)) => {
                zen.last_glide = Some((now, offset));
                if zen.lift != lift {
                    zen.lift = lift;
                    cx.notify();
                }
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

    /// Raise lowered text before the editor scrolls it. While lowered, the
    /// glide keeps the editor at its top, so a scroll toward the end, from
    /// the wheel or the editor keeping the caret in view, spends the lift
    /// first and the text never moves twice. The glide moves both itself,
    /// and its scroll only lands at the editor's next layout.
    pub(super) fn settle_zen_lift(&mut self, cx: &mut Context<Self>) {
        let Some(zen) = &self.zen else {
            return;
        };
        let lift = zen.lift;
        if zen.gliding || lift <= px(0.) {
            return;
        }
        let editor = self.tab().editor.clone();
        let offset = editor.read(cx).scroll_offset();
        let taken = (-offset.y).min(lift);
        if taken <= px(0.) {
            return;
        }
        editor.update(cx, |editor, cx| {
            editor.set_scroll_offset(point(offset.x, offset.y + taken), cx);
        });
        if let Some(zen) = &mut self.zen {
            zen.lift -= taken;
        }
        cx.notify();
    }

    /// Writing hides the footer, so only the text is left on the screen.
    pub(super) fn hide_zen_chrome(&mut self, cx: &mut Context<Self>) {
        if let Some(zen) = &mut self.zen
            && !zen.chrome_hidden
        {
            zen.chrome_hidden = true;
            zen.pointer_at = None;
            cx.notify();
        }
    }

    /// Moving the pointer brings the footer back with it.
    fn show_zen_chrome(&mut self, event: &MouseMoveEvent, cx: &mut Context<Self>) {
        let Some(zen) = &mut self.zen else {
            return;
        };
        let at = event.position;
        let Some(last) = zen.pointer_at.replace(at) else {
            return;
        };
        let travel = (at.x - last.x).abs().max((at.y - last.y).abs());
        if !zen.chrome_hidden || travel < zen::POINTER_TRAVEL {
            zen.pointer_at = Some(last);
            return;
        }
        zen.chrome_hidden = false;
        cx.notify();
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
        self.sync_previews(cx);
        let preview_shown = self.preview_shown();
        let cursor = self.tab().editor.read(cx).cursor();
        self.tab().preview.update(cx, |preview, cx| {
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
        let measure = self.editor_measure(zen::MEASURE_CHARS, &self.tab().editor, window, cx);
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
            Some(ZenView::Editor) => {
                self.tab().editor.update(cx, |editor, cx| {
                    let Some(line_height) = editor.line_height() else {
                        return;
                    };
                    let delta = event.delta.pixel_delta(line_height);
                    let offset = editor.scroll_offset();
                    editor.set_scroll_offset(point(offset.x, offset.y + delta.y), cx);
                });
            }
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
    /// quiet line beneath it that shares the column's edges and fades out
    /// while writing.
    pub(super) fn render_zen(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let rem = window.rem_size();
        let band = rem * SHEET_HEADER_REMS;
        let column = self.zen_column_width(window, cx);
        let view = self.zen_view().unwrap_or(ZenView::Editor);
        let (lift, chrome_hidden) = self
            .zen
            .as_ref()
            .map_or((px(0.), false), |zen| (zen.lift, zen.chrome_hidden));
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
            // Shortened from the top by the lift, so the editor's view is
            // still exactly what shows, and it scrolls a selection dragged
            // past the bottom.
            ZenView::Editor => div()
                .pt(lift)
                .size_full()
                .child(
                    self.tab()
                        .editor_pane
                        .clone()
                        .cached(StyleRefinement::default().size_full()),
                )
                .into_any_element(),
            ZenView::Preview => MarkdownPreview::pane(&self.tab().preview, cx).into_any_element(),
        };

        // Plain words in the text's quiet ink, the shown view in full ink:
        // no box around them, at rest or under the pointer, to compete with
        // the page.
        let quiet = theme.muted_foreground;
        let view_button = |id: &'static str, item: ZenView, mode: LayoutMode| {
            let shown = item == view;
            Button::new(id)
                .xsmall()
                .text()
                .label(mode.label())
                .toggled(shown)
                .text_color(if shown { theme.foreground } else { quiet })
                .on_click(
                    cx.listener(move |this, _, window, cx| this.set_zen_view(item, window, cx)),
                )
        };

        // Text buttons have no inset, so the words line up with the text
        // above them.
        let footer = div()
            .flex_none()
            .h(band)
            .flex()
            .items_center()
            .justify_between()
            .pl(zen::TEXT_LEADING)
            .pr(zen::TEXT_TRAILING)
            .text_xs()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(view_button(
                        "zen-editor",
                        ZenView::Editor,
                        LayoutMode::Editor,
                    ))
                    .child(div().text_color(quiet).child("·"))
                    .child(view_button(
                        "zen-preview",
                        ZenView::Preview,
                        LayoutMode::Preview,
                    )),
            )
            .child(
                Button::new("exit-zen")
                    .xsmall()
                    .text()
                    .label("Exit Zen Mode")
                    .text_color(quiet)
                    .tooltip_with_action("Exit Zen Mode", &ToggleZen, Some(KEY_CONTEXT))
                    .on_click(cx.listener(|this, _, window, cx| this.exit_zen(window, cx))),
            );
        // It leaves slowly, once writing has started, and comes back quickly
        // when wanted.
        let footer = if chrome_hidden {
            footer
                .with_animation(
                    "zen-footer-out",
                    Animation::new(zen::CHROME_FADE_OUT).with_easing(ease_in_out),
                    |footer, delta| footer.opacity(1. - delta),
                )
                .into_any_element()
        } else {
            footer
                .with_animation(
                    "zen-footer-in",
                    Animation::new(zen::CHROME_FADE_IN).with_easing(ease_in_out),
                    |footer, delta| footer.opacity(delta),
                )
                .into_any_element()
        };

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
            .on_mouse_move(cx.listener(|this, event, _, cx| this.show_zen_chrome(event, cx)))
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
