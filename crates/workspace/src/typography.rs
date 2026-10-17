use gpui_kit::component::{
    ActiveTheme as _, Sizable as _,
    select::{SearchableVec, Select, SelectEvent, SelectState},
};
use gpui_kit::{
    AppContext as _, Context, Entity, EventEmitter, IntoElement, ParentElement as _, Render,
    SharedString, Styled as _, Subscription, Window, div,
};

type FontSelect = SelectState<SearchableVec<SharedString>>;
type SizeSelect = SelectState<Vec<SharedString>>;

/// Independent document typography; application chrome keeps the system font.
pub(super) struct Typography {
    editor_font: Entity<FontSelect>,
    preview_font: Entity<FontSelect>,
    editor_size: Entity<SizeSelect>,
    preview_size: Entity<SizeSelect>,
    _subscriptions: Vec<Subscription>,
}

pub(super) struct TypographyChanged;

#[derive(Clone, Copy)]
enum Pane {
    Editor,
    Preview,
}
impl EventEmitter<TypographyChanged> for Typography {}

impl Typography {
    pub(super) fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut fonts: Vec<SharedString> = cx
            .text_system()
            .all_font_names()
            .into_iter()
            .filter(|name| !name.starts_with('.'))
            .map(Into::into)
            .collect();
        let editor_default = cx.theme().mono_font_family.clone();
        let preview_default: SharedString = "System default".into();
        fonts.extend([editor_default.clone(), preview_default.clone()]);
        fonts.sort();
        fonts.dedup();
        let editor_font = cx.new(|cx| {
            SelectState::new(SearchableVec::new(fonts.clone()), None, window, cx).searchable(true)
        });
        let preview_font = cx.new(|cx| {
            SelectState::new(SearchableVec::new(fonts), None, window, cx).searchable(true)
        });
        editor_font.update(cx, |state, cx| {
            state.set_selected_value(&editor_default, window, cx)
        });
        preview_font.update(cx, |state, cx| {
            state.set_selected_value(&preview_default, window, cx)
        });
        let sizes: Vec<SharedString> = ["12", "14", "16", "18", "20", "24"]
            .into_iter()
            .map(Into::into)
            .collect();
        let editor_size = cx.new(|cx| SelectState::new(sizes.clone(), None, window, cx));
        let preview_size = cx.new(|cx| SelectState::new(sizes, None, window, cx));
        editor_size.update(cx, |state, cx| {
            state.set_selected_value(&"14".into(), window, cx)
        });
        preview_size.update(cx, |state, cx| {
            state.set_selected_value(&"16".into(), window, cx)
        });
        let mut subscriptions = Vec::new();
        for font in [&editor_font, &preview_font] {
            subscriptions.push(cx.subscribe(font, |_, _, event, cx| {
                if matches!(event, SelectEvent::Confirm(Some(_))) {
                    cx.emit(TypographyChanged);
                    cx.notify();
                }
            }));
        }
        for size in [&editor_size, &preview_size] {
            subscriptions.push(cx.subscribe(size, |_, _, event, cx| {
                if matches!(event, SelectEvent::Confirm(Some(_))) {
                    cx.emit(TypographyChanged);
                    cx.notify();
                }
            }));
        }
        Self {
            editor_font,
            preview_font,
            editor_size,
            preview_size,
            _subscriptions: subscriptions,
        }
    }

    pub(super) fn editor_font(&self, cx: &gpui_kit::App) -> SharedString {
        match self.editor_font.read(cx).selected_value() {
            Some(value) if value.as_ref() != "System default" => value.clone(),
            _ => cx.theme().font_family.clone(),
        }
    }
    pub(super) fn preview_font(&self, cx: &gpui_kit::App) -> SharedString {
        match self.preview_font.read(cx).selected_value() {
            Some(value) if value.as_ref() != "System default" => value.clone(),
            _ => cx.theme().font_family.clone(),
        }
    }
    pub(super) fn editor_size(&self, cx: &gpui_kit::App) -> f32 {
        Self::size(&self.editor_size, cx)
    }
    pub(super) fn preview_size(&self, cx: &gpui_kit::App) -> f32 {
        Self::size(&self.preview_size, cx)
    }
    fn size(state: &Entity<SizeSelect>, cx: &gpui_kit::App) -> f32 {
        state
            .read(cx)
            .selected_value()
            .and_then(|value| value.parse().ok())
            .unwrap_or(16.)
    }
}

impl Typography {
    /// One pane's font and size, with a sample drawn on the document surface
    /// so it previews the change where it will land.
    fn render_section(&self, pane: Pane, cx: &Context<Self>) -> impl IntoElement {
        let (title, font, size, sample_font, sample_size, sample) = match pane {
            Pane::Editor => (
                "Editor",
                &self.editor_font,
                &self.editor_size,
                self.editor_font(cx),
                self.editor_size(cx),
                "# A place for your ideas",
            ),
            Pane::Preview => (
                "Preview",
                &self.preview_font,
                &self.preview_size,
                self.preview_font(cx),
                self.preview_size(cx),
                "A place for your ideas",
            ),
        };
        let theme = cx.theme();
        div()
            .flex()
            .w_full()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .text_sm()
                    .font_weight(gpui_kit::FontWeight::MEDIUM)
                    .child(title),
            )
            .child(
                div()
                    .flex()
                    .w_full()
                    .items_center()
                    .gap_2()
                    .child(
                        // A preferred width that may shrink: a dialog lays out
                        // at content size, so a zero-basis flex child collapses.
                        div().w_80().flex_shrink_1().min_w_0().child(
                            Select::new(font)
                                .small()
                                .accessibility_label(format!("{title} font"))
                                .w_full(),
                        ),
                    )
                    .child(
                        Select::new(size)
                            .small()
                            .accessibility_label(format!("{title} size"))
                            .w_20(),
                    ),
            )
            .child(
                div()
                    .mt_1()
                    .flex()
                    .items_center()
                    .min_h_16()
                    .px_4()
                    .py_3()
                    .rounded(theme.radius_lg)
                    .border_1()
                    .border_color(theme.border)
                    .bg(theme.background)
                    .font_family(sample_font)
                    .text_size(gpui_kit::rems(sample_size / 16.))
                    .child(sample),
            )
    }
}

impl Render for Typography {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        div()
            .flex()
            .w_full()
            .flex_col()
            .gap_6()
            .child(self.render_section(Pane::Editor, cx))
            .child(self.render_section(Pane::Preview, cx))
            .child(
                div()
                    .text_xs()
                    .text_color(muted)
                    .child("Changes apply to this window. Code keeps its monospace font."),
            )
    }
}
