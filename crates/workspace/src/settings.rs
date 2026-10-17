//! Settings: sections in a sidebar on the window chrome, the chosen section
//! on the document sheet, in one reading-width column.

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::select::Select;
use gpui_kit::component::{
    ActiveTheme as _, Icon, IconName, Selectable as _, Sizable as _, ThemeConfig, TitleBar,
    try_parse_color,
};
use gpui_kit::{
    AnyElement, App, Context, Entity, FocusHandle, Focusable, FontWeight, Hsla,
    InteractiveElement as _, IntoElement, ParentElement as _, Render, SharedString, Styled as _,
    Window, div, prelude::FluentBuilder as _, relative, rems,
};

use crate::SETTINGS_KEY_CONTEXT;
use crate::theme::{self, Appearance};
use crate::typography::{Pane, Typography};
use crate::workspace::{CloseSettings, SHEET_HEADER_REMS, SHEET_INSET_REMS, SIDEBAR_WIDTH_REMS};

/// The reading column's width: wide enough for a label and a font picker.
const COLUMN_REMS: f32 = 36.;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Section {
    Appearance,
    Typography,
}

impl Section {
    const ALL: [Self; 2] = [Self::Appearance, Self::Typography];

    fn title(self) -> &'static str {
        match self {
            Self::Appearance => "Appearance",
            Self::Typography => "Typography",
        }
    }

    fn icon(self) -> IconName {
        match self {
            Self::Appearance => IconName::Palette,
            Self::Typography => IconName::ALargeSmall,
        }
    }
}

pub(crate) struct SettingsView {
    section: Section,
    typography: Entity<Typography>,
    focus_handle: FocusHandle,
}

impl SettingsView {
    pub(crate) fn new(typography: Entity<Typography>, cx: &mut Context<Self>) -> Self {
        Self {
            section: Section::Appearance,
            typography,
            focus_handle: cx.focus_handle(),
        }
    }

    /// The window controls' row on the chrome, then one row per section.
    fn render_sidebar(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        div()
            .flex()
            .flex_col()
            .flex_none()
            .w(rems(SIDEBAR_WIDTH_REMS))
            .h_full()
            .bg(theme.sidebar)
            .text_color(theme.sidebar_foreground)
            .child(
                TitleBar::new()
                    .h(window.rem_size() * (2. * SHEET_INSET_REMS + SHEET_HEADER_REMS))
                    .bg(theme.sidebar)
                    .border_color(gpui_kit::transparent_black()),
            )
            .child(
                // Lines up with the folder name the sidebar shows otherwise.
                div()
                    .flex()
                    .items_center()
                    .h_8()
                    .mx_2()
                    .pl_1()
                    .text_sm()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("Settings"),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_0p5()
                    .px_2()
                    .children(Section::ALL.map(|section| {
                        let selected = section == self.section;
                        let muted = cx.theme().muted_foreground;
                        let accent = cx.theme().primary;
                        Button::new(section.title())
                            .ghost()
                            .small()
                            .w_full()
                            .h_7()
                            .px_1p5()
                            .selected(selected)
                            .accessibility_label(section.title())
                            .child(
                                div()
                                    .flex()
                                    .w_full()
                                    .items_center()
                                    .gap_2()
                                    .text_sm()
                                    .child(
                                        Icon::new(section.icon()).small().text_color(if selected {
                                            accent
                                        } else {
                                            muted
                                        }),
                                    )
                                    .child(
                                        div()
                                            .when(selected, |this| {
                                                this.font_weight(FontWeight::MEDIUM)
                                            })
                                            .child(section.title()),
                                    ),
                            )
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.section = section;
                                cx.notify();
                            }))
                    })),
            )
    }

    /// The section on the document sheet, under a header with its title.
    fn render_sheet(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        let body = match self.section {
            Section::Appearance => self.render_appearance(cx).into_any_element(),
            Section::Typography => self.render_typography(cx).into_any_element(),
        };
        let theme = cx.theme();
        let inset = window.rem_size() * SHEET_INSET_REMS;
        div().flex_1().min_w_0().h_full().p(inset).pl_0().child(
            div()
                .flex()
                .flex_col()
                .size_full()
                .rounded(theme.radius_lg)
                .border_1()
                .border_color(theme.border)
                .bg(theme.background)
                .overflow_hidden()
                .child(
                    TitleBar::new()
                        .h(window.rem_size() * SHEET_HEADER_REMS)
                        .pl_4()
                        .pr_2()
                        .bg(gpui_kit::transparent_black())
                        .border_color(theme.border)
                        .child(
                            div()
                                .flex()
                                .flex_1()
                                .items_center()
                                .justify_between()
                                .child(
                                    div()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .child(self.section.title()),
                                )
                                .child(
                                    Button::new("close-settings")
                                        .small()
                                        .label("Done")
                                        .tooltip_with_action(
                                            "Close settings",
                                            &CloseSettings,
                                            Some(SETTINGS_KEY_CONTEXT),
                                        )
                                        .on_click(|_, window, cx| {
                                            window.dispatch_action(Box::new(CloseSettings), cx)
                                        }),
                                ),
                        ),
                )
                .child(
                    div().flex_1().min_h_0().child(
                        div()
                            .id("settings-scroll")
                            .size_full()
                            .overflow_y_scrollbar()
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap_8()
                                    .w_full()
                                    .max_w(rems(COLUMN_REMS))
                                    .mx_auto()
                                    .px_6()
                                    .py_8()
                                    .child(body),
                            ),
                    ),
                ),
        )
    }

    fn render_appearance(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let current = theme::appearance(cx);
        let light = Palette::from_config(&cx.theme().light_theme, cx);
        let dark = Palette::from_config(&cx.theme().dark_theme, cx);
        let theme = cx.theme();
        section(
            "Theme",
            "System follows the appearance set in macOS.",
            None,
            div()
                .flex()
                .gap_3()
                .children(Appearance::ALL.map(|appearance| {
                    let selected = appearance == current;
                    let thumbnail = match appearance {
                        Appearance::System => split_thumbnail(&light, &dark),
                        Appearance::Light => light.thumbnail(),
                        Appearance::Dark => dark.thumbnail(),
                    };
                    Button::new(appearance.label())
                        .ghost()
                        .flex_1()
                        .h_auto()
                        .p_1p5()
                        .rounded(theme.radius_lg)
                        .accessibility_label(format!("{} appearance", appearance.label()))
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .w_full()
                                .gap_2()
                                .child(
                                    div()
                                        .w_full()
                                        .h(rems(5.5))
                                        .rounded(theme.radius)
                                        .overflow_hidden()
                                        .map(|this| {
                                            if selected {
                                                this.border_2().border_color(theme.primary)
                                            } else {
                                                this.border_1().border_color(theme.border)
                                            }
                                        })
                                        .child(thumbnail),
                                )
                                .child(
                                    div()
                                        .w_full()
                                        .text_center()
                                        .text_sm()
                                        .when(selected, |this| this.font_weight(FontWeight::MEDIUM))
                                        .when(!selected, |this| {
                                            this.text_color(theme.muted_foreground)
                                        })
                                        .child(appearance.label()),
                                ),
                        )
                        .on_click(move |_, window, cx| {
                            theme::set_appearance(appearance, window, cx);
                        })
                })),
        )
    }

    fn render_typography(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .gap_8()
            .children(Pane::ALL.map(|pane| self.render_pane(pane, cx)))
    }

    /// One pane's sample, font and size, as a card of rows.
    fn render_pane(&self, pane: Pane, cx: &mut Context<Self>) -> AnyElement {
        let typography = self.typography.read(cx);
        let font = typography.font(pane, cx);
        let size = typography.size(pane, cx);
        let font_state = typography.font_state(pane).clone();
        let size_state = typography.size_state(pane).clone();
        let is_default = typography.is_default(pane, cx);
        let title = pane.title();
        let (description, sample) = match pane {
            Pane::Editor => ("The Markdown source.", "# A place for your ideas"),
            Pane::Preview => (
                "The formatted document. Code keeps its monospace font.",
                "A place for your ideas",
            ),
        };
        let reset = (!is_default).then(|| {
            Button::new(SharedString::from(format!("reset-{title}")))
                .xsmall()
                .ghost()
                .label("Reset")
                .accessibility_label(format!("Reset {title} typography"))
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.typography
                        .update(cx, |typography, cx| typography.reset(pane, window, cx));
                }))
                .into_any_element()
        });
        let theme = cx.theme();
        let row = |label: &'static str, control: AnyElement| {
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap_4()
                .h_12()
                .px_4()
                .child(div().text_sm().child(label))
                // A Select sizes against its wrapper, so it gets a definite one.
                .child(div().flex_none().w(rems(14.)).child(control))
        };

        section(
            title,
            description,
            reset,
            div()
                .flex()
                .flex_col()
                .rounded(theme.radius_lg)
                .border_1()
                .border_color(theme.border)
                .overflow_hidden()
                .child(
                    // Drawn on the document surface so it previews the
                    // change where it will land.
                    div()
                        .flex()
                        .items_center()
                        .min_h_20()
                        .px_5()
                        .py_4()
                        .bg(theme.background)
                        .border_b_1()
                        .border_color(theme.border)
                        .font_family(font)
                        .text_size(rems(size / 16.))
                        .child(sample),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .bg(theme.sidebar.opacity(0.4))
                        .child(row(
                            "Font",
                            Select::new(&font_state)
                                .small()
                                .menu_width(rems(16.))
                                .accessibility_label(format!("{title} font"))
                                .w_full()
                                .into_any_element(),
                        ))
                        .child(div().mx_4().h_px().bg(theme.border))
                        .child(row(
                            "Size",
                            Select::new(&size_state)
                                .small()
                                .accessibility_label(format!("{title} size"))
                                .w_full()
                                .into_any_element(),
                        )),
                ),
        )
        .into_any_element()
    }
}

/// A titled block of the column: a heading with a short description, an
/// optional action at the trailing edge, then its content.
fn section(
    title: &'static str,
    description: &'static str,
    action: Option<AnyElement>,
    content: impl IntoElement,
) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .gap_3()
        .child(
            div()
                .flex()
                .items_end()
                .justify_between()
                .gap_4()
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_0p5()
                        .child(
                            div()
                                .text_sm()
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(title),
                        )
                        .child(SectionDescription(description)),
                )
                .children(action),
        )
        .child(content)
}

/// Muted supporting text, colored at render time from the active theme.
#[derive(IntoElement)]
struct SectionDescription(&'static str);

impl gpui_kit::RenderOnce for SectionDescription {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        div()
            .text_xs()
            .text_color(cx.theme().muted_foreground)
            .child(self.0)
    }
}

/// The colors of one Neon theme, for drawing a miniature of its window.
struct Palette {
    chrome: Hsla,
    sheet: Hsla,
    border: Hsla,
    text: Hsla,
    faint: Hsla,
    accent: Hsla,
}

impl Palette {
    /// Read a theme's own colors, falling back to the active theme's for any
    /// the theme leaves out.
    fn from_config(config: &ThemeConfig, cx: &App) -> Self {
        let theme = cx.theme();
        let colors = &config.colors;
        let parse = |value: &Option<SharedString>, fallback: Hsla| {
            value
                .as_deref()
                .and_then(|value| try_parse_color(value).ok())
                .unwrap_or(fallback)
        };
        let text = parse(&colors.foreground, theme.foreground);
        Self {
            chrome: parse(&colors.sidebar, theme.sidebar),
            sheet: parse(&colors.background, theme.background),
            border: parse(&colors.border, theme.border),
            text,
            faint: parse(&colors.muted_foreground, theme.muted_foreground).opacity(0.5),
            accent: parse(&colors.primary, theme.primary),
        }
    }

    /// A window in miniature: a sidebar on the chrome and a sheet with a
    /// heading, lines of text and the accent caret.
    fn thumbnail(&self) -> AnyElement {
        let line = |width: f32, color: Hsla| {
            div()
                .h(rems(0.25))
                .w(relative(width))
                .rounded_full()
                .bg(color)
        };
        div()
            .flex()
            .size_full()
            .bg(self.chrome)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_none()
                    .w(relative(0.28))
                    .gap(rems(0.3))
                    .pt(rems(0.9))
                    .px(rems(0.4))
                    .child(line(0.8, self.faint))
                    .child(line(0.6, self.faint))
                    .child(line(0.7, self.faint)),
            )
            .child(
                div().flex_1().py(rems(0.3)).pr(rems(0.3)).child(
                    div()
                        .flex()
                        .flex_col()
                        .size_full()
                        .gap(rems(0.3))
                        .p(rems(0.5))
                        .rounded(rems(0.25))
                        .border_1()
                        .border_color(self.border)
                        .bg(self.sheet)
                        .child(line(0.5, self.text))
                        .child(line(0.9, self.faint))
                        .child(line(0.75, self.faint))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(rems(0.15))
                                .child(line(0.4, self.faint).flex_none())
                                .child(div().w(rems(0.1)).h(rems(0.5)).bg(self.accent)),
                        ),
                ),
            )
            .into_any_element()
    }
}

/// The light miniature with its right half drawn in dark.
fn split_thumbnail(light: &Palette, dark: &Palette) -> AnyElement {
    div()
        .relative()
        .size_full()
        .child(light.thumbnail())
        .child(
            div()
                .absolute()
                .top_0()
                .right_0()
                .h_full()
                .w(relative(0.5))
                .overflow_hidden()
                .child(
                    div()
                        .absolute()
                        .top_0()
                        .right_0()
                        .h_full()
                        .w(relative(2.))
                        .child(dark.thumbnail()),
                ),
        )
        .into_any_element()
}

impl Focusable for SettingsView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for SettingsView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("settings")
            .key_context(SETTINGS_KEY_CONTEXT)
            .track_focus(&self.focus_handle)
            .flex()
            .size_full()
            .child(self.render_sidebar(window, cx))
            .child(self.render_sheet(window, cx))
    }
}
