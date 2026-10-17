use gpui_kit::component::{
    ActiveTheme as _,
    select::{SearchableVec, SelectEvent, SelectItem, SelectState},
};
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Entity, EventEmitter, IntoElement,
    ParentElement as _, SharedString, Styled as _, Subscription, Window, div,
};

pub(crate) type FontSelect = SelectState<SearchableVec<FontItem>>;
pub(crate) type SizeSelect = SelectState<Vec<SharedString>>;

const SYSTEM_DEFAULT: &str = "System default";
const EDITOR_DEFAULT_SIZE: &str = "14";
const PREVIEW_DEFAULT_SIZE: &str = "16";

/// One installed font, listed in its own typeface.
#[derive(Clone)]
pub(crate) struct FontItem {
    name: SharedString,
    family: SharedString,
}

impl SelectItem for FontItem {
    type Value = SharedString;

    fn title(&self) -> SharedString {
        self.name.clone()
    }

    fn display_title(&self) -> Option<AnyElement> {
        Some(self.render_name().into_any_element())
    }

    fn render(&self, _: &mut Window, _: &mut App) -> impl IntoElement {
        self.render_name()
    }

    fn value(&self) -> &SharedString {
        &self.name
    }
}

impl FontItem {
    fn render_name(&self) -> impl IntoElement {
        div()
            .min_w_0()
            .truncate()
            .font_family(self.family.clone())
            .child(self.name.clone())
    }
}

/// Independent document typography; application chrome keeps the system font.
pub(super) struct Typography {
    editor_font: Entity<FontSelect>,
    preview_font: Entity<FontSelect>,
    editor_size: Entity<SizeSelect>,
    preview_size: Entity<SizeSelect>,
    /// The theme's monospace family when the window opened.
    editor_default_font: SharedString,
    _subscriptions: Vec<Subscription>,
}

pub(super) struct TypographyChanged;

/// A pane whose text the typography sets.
#[derive(Clone, Copy)]
pub(crate) enum Pane {
    Editor,
    Preview,
}

impl Pane {
    pub(crate) const ALL: [Self; 2] = [Self::Editor, Self::Preview];

    pub(crate) fn title(self) -> &'static str {
        match self {
            Self::Editor => "Editor",
            Self::Preview => "Preview",
        }
    }
}

impl EventEmitter<TypographyChanged> for Typography {}

impl Typography {
    pub(super) fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut names: Vec<SharedString> = cx
            .text_system()
            .all_font_names()
            .into_iter()
            .filter(|name| !name.starts_with('.'))
            .map(Into::into)
            .collect();
        let editor_default = cx.theme().mono_font_family.clone();
        let preview_default: SharedString = SYSTEM_DEFAULT.into();
        names.extend([editor_default.clone(), preview_default.clone()]);
        names.sort();
        names.dedup();
        let system_family = cx.theme().font_family.clone();
        let fonts: Vec<FontItem> = names
            .into_iter()
            .map(|name| FontItem {
                family: if name.as_ref() == SYSTEM_DEFAULT {
                    system_family.clone()
                } else {
                    name.clone()
                },
                name,
            })
            .collect();
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
            state.set_selected_value(&EDITOR_DEFAULT_SIZE.into(), window, cx)
        });
        preview_size.update(cx, |state, cx| {
            state.set_selected_value(&PREVIEW_DEFAULT_SIZE.into(), window, cx)
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
            editor_default_font: editor_default,
            _subscriptions: subscriptions,
        }
    }

    pub(super) fn editor_font(&self, cx: &App) -> SharedString {
        self.font(Pane::Editor, cx)
    }
    pub(super) fn preview_font(&self, cx: &App) -> SharedString {
        self.font(Pane::Preview, cx)
    }
    pub(super) fn editor_size(&self, cx: &App) -> f32 {
        self.size(Pane::Editor, cx)
    }
    pub(super) fn preview_size(&self, cx: &App) -> f32 {
        self.size(Pane::Preview, cx)
    }

    /// The family the pane draws its text in.
    pub(crate) fn font(&self, pane: Pane, cx: &App) -> SharedString {
        match self.font_state(pane).read(cx).selected_value() {
            Some(value) if value.as_ref() != SYSTEM_DEFAULT => value.clone(),
            _ => cx.theme().font_family.clone(),
        }
    }
    /// The pane's text size in pixels at the default rem size.
    pub(crate) fn size(&self, pane: Pane, cx: &App) -> f32 {
        self.size_state(pane)
            .read(cx)
            .selected_value()
            .and_then(|value| value.parse().ok())
            .unwrap_or(16.)
    }

    pub(crate) fn font_state(&self, pane: Pane) -> &Entity<FontSelect> {
        match pane {
            Pane::Editor => &self.editor_font,
            Pane::Preview => &self.preview_font,
        }
    }
    pub(crate) fn size_state(&self, pane: Pane) -> &Entity<SizeSelect> {
        match pane {
            Pane::Editor => &self.editor_size,
            Pane::Preview => &self.preview_size,
        }
    }

    fn default_font(&self, pane: Pane) -> SharedString {
        match pane {
            Pane::Editor => self.editor_default_font.clone(),
            Pane::Preview => SYSTEM_DEFAULT.into(),
        }
    }
    fn default_size(pane: Pane) -> SharedString {
        match pane {
            Pane::Editor => EDITOR_DEFAULT_SIZE.into(),
            Pane::Preview => PREVIEW_DEFAULT_SIZE.into(),
        }
    }

    /// Whether the pane uses its default font and size.
    pub(crate) fn is_default(&self, pane: Pane, cx: &App) -> bool {
        self.font_state(pane).read(cx).selected_value() == Some(&self.default_font(pane))
            && self.size_state(pane).read(cx).selected_value() == Some(&Self::default_size(pane))
    }

    /// Return the pane to its default font and size.
    pub(crate) fn reset(&mut self, pane: Pane, window: &mut Window, cx: &mut Context<Self>) {
        let font = self.default_font(pane);
        let size = Self::default_size(pane);
        self.font_state(pane)
            .update(cx, |state, cx| state.set_selected_value(&font, window, cx));
        self.size_state(pane)
            .update(cx, |state, cx| state.set_selected_value(&size, window, cx));
        cx.emit(TypographyChanged);
        cx.notify();
    }
}
