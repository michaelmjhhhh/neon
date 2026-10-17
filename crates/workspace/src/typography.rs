use gpui_kit::component::{
    ActiveTheme as _,
    select::{SearchableVec, SelectEvent, SelectItem, SelectState},
};
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Entity, EventEmitter, Global, IntoElement,
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
    editor_font: SharedString,
    preview_font: SharedString,
    editor_size: SharedString,
    preview_size: SharedString,
    /// The theme's monospace family when the window opened.
    editor_default_font: SharedString,
    pickers: Option<TypographyPickers>,
}

/// Retained only while the typography settings are visible.
struct TypographyPickers {
    editor_font: Entity<FontSelect>,
    preview_font: Entity<FontSelect>,
    editor_size: Entity<SizeSelect>,
    preview_size: Entity<SizeSelect>,
    _subscriptions: Vec<Subscription>,
}

/// Enumerate once on first use; the picker lists share these font-name strings.
struct InstalledFonts(Vec<SharedString>);

impl Global for InstalledFonts {}

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
    pub(super) fn new(_: &mut Window, cx: &mut Context<Self>) -> Self {
        let editor_default = cx.theme().mono_font_family.clone();
        Self {
            editor_font: editor_default.clone(),
            preview_font: SYSTEM_DEFAULT.into(),
            editor_size: EDITOR_DEFAULT_SIZE.into(),
            preview_size: PREVIEW_DEFAULT_SIZE.into(),
            editor_default_font: editor_default,
            pickers: None,
        }
    }

    pub(crate) fn show_pickers(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.pickers.is_some() {
            return;
        }
        if cx.try_global::<InstalledFonts>().is_none() {
            let mut names: Vec<SharedString> = cx
                .text_system()
                .all_font_names()
                .into_iter()
                .filter(|name| !name.starts_with('.'))
                .map(Into::into)
                .collect();
            names.sort();
            names.dedup();
            cx.set_global(InstalledFonts(names));
        }
        let mut names = cx.global::<InstalledFonts>().0.clone();
        names.extend([
            self.editor_default_font.clone(),
            SYSTEM_DEFAULT.into(),
            self.editor_font.clone(),
            self.preview_font.clone(),
        ]);
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
            state.set_selected_value(&self.editor_font, window, cx)
        });
        preview_font.update(cx, |state, cx| {
            state.set_selected_value(&self.preview_font, window, cx)
        });
        let sizes: Vec<SharedString> = ["12", "14", "16", "18", "20", "24"]
            .into_iter()
            .map(Into::into)
            .collect();
        let editor_size = cx.new(|cx| SelectState::new(sizes.clone(), None, window, cx));
        let preview_size = cx.new(|cx| SelectState::new(sizes, None, window, cx));
        editor_size.update(cx, |state, cx| {
            state.set_selected_value(&self.editor_size, window, cx)
        });
        preview_size.update(cx, |state, cx| {
            state.set_selected_value(&self.preview_size, window, cx)
        });
        let mut subscriptions = Vec::new();
        for (pane, font) in [(Pane::Editor, &editor_font), (Pane::Preview, &preview_font)] {
            subscriptions.push(cx.subscribe(font, move |this, _, event, cx| {
                if let SelectEvent::Confirm(Some(value)) = event {
                    *this.font_value_mut(pane) = value.clone();
                    cx.emit(TypographyChanged);
                    cx.notify();
                }
            }));
        }
        for (pane, size) in [(Pane::Editor, &editor_size), (Pane::Preview, &preview_size)] {
            subscriptions.push(cx.subscribe(size, move |this, _, event, cx| {
                if let SelectEvent::Confirm(Some(value)) = event {
                    *this.size_value_mut(pane) = value.clone();
                    cx.emit(TypographyChanged);
                    cx.notify();
                }
            }));
        }
        self.pickers = Some(TypographyPickers {
            editor_font,
            preview_font,
            editor_size,
            preview_size,
            _subscriptions: subscriptions,
        });
    }

    pub(crate) fn hide_pickers(&mut self) {
        self.pickers = None;
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
        let value = self.font_value(pane);
        if value.as_ref() == SYSTEM_DEFAULT {
            cx.theme().font_family.clone()
        } else {
            value.clone()
        }
    }
    /// The pane's text size in pixels at the default rem size.
    pub(crate) fn size(&self, pane: Pane, _: &App) -> f32 {
        self.size_value(pane).parse().unwrap_or(16.)
    }

    /// The preview's typography, so an export reads like the preview: its
    /// font and size for text, the theme's monospace font for code.
    pub(crate) fn export_style(&self, cx: &App) -> export::ExportStyle {
        // Families named with a leading dot are the system's own, which
        // other apps can only reach by a generic name.
        let named = |family: &SharedString| {
            (!family.is_empty() && !family.starts_with('.')).then(|| family.to_string())
        };
        export::ExportStyle {
            text_font: (self.preview_font.as_ref() != SYSTEM_DEFAULT)
                .then(|| named(&self.preview_font))
                .flatten(),
            text_size_px: self.size(Pane::Preview, cx),
            code_font: named(&cx.theme().mono_font_family),
            ..export::ExportStyle::default()
        }
    }

    pub(crate) fn font_state(&self, pane: Pane) -> Option<&Entity<FontSelect>> {
        let pickers = self.pickers.as_ref()?;
        Some(match pane {
            Pane::Editor => &pickers.editor_font,
            Pane::Preview => &pickers.preview_font,
        })
    }
    pub(crate) fn size_state(&self, pane: Pane) -> Option<&Entity<SizeSelect>> {
        let pickers = self.pickers.as_ref()?;
        Some(match pane {
            Pane::Editor => &pickers.editor_size,
            Pane::Preview => &pickers.preview_size,
        })
    }

    fn font_value(&self, pane: Pane) -> &SharedString {
        match pane {
            Pane::Editor => &self.editor_font,
            Pane::Preview => &self.preview_font,
        }
    }
    fn font_value_mut(&mut self, pane: Pane) -> &mut SharedString {
        match pane {
            Pane::Editor => &mut self.editor_font,
            Pane::Preview => &mut self.preview_font,
        }
    }
    fn size_value(&self, pane: Pane) -> &SharedString {
        match pane {
            Pane::Editor => &self.editor_size,
            Pane::Preview => &self.preview_size,
        }
    }
    fn size_value_mut(&mut self, pane: Pane) -> &mut SharedString {
        match pane {
            Pane::Editor => &mut self.editor_size,
            Pane::Preview => &mut self.preview_size,
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
    pub(crate) fn is_default(&self, pane: Pane, _: &App) -> bool {
        self.font_value(pane) == &self.default_font(pane)
            && self.size_value(pane) == &Self::default_size(pane)
    }

    /// Return the pane to its default font and size.
    pub(crate) fn reset(&mut self, pane: Pane, window: &mut Window, cx: &mut Context<Self>) {
        let font = self.default_font(pane);
        let size = Self::default_size(pane);
        *self.font_value_mut(pane) = font.clone();
        *self.size_value_mut(pane) = size.clone();
        if let Some(state) = self.font_state(pane) {
            state.update(cx, |state, cx| state.set_selected_value(&font, window, cx));
        }
        if let Some(state) = self.size_state(pane) {
            state.update(cx, |state, cx| state.set_selected_value(&size, window, cx));
        }
        cx.emit(TypographyChanged);
        cx.notify();
    }
}
