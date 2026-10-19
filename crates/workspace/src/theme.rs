//! Neon's light and dark themes: graphite surfaces and one orange accent,
//! the filament from the app icon, reserved for state (caret, selection,
//! the unsaved marker, checked tasks).

use gpui_kit::component::{Theme, ThemeMode, ThemeRegistry};
use gpui_kit::{App, Global, Window};
use serde::{Deserialize, Serialize};

use crate::settings_file;

const THEMES: &str = include_str!("../themes/neon.json");
const LIGHT: &str = "Neon Light";
const DARK: &str = "Neon Dark";

/// Register the Neon themes as the ones each appearance loads. The caller
/// still picks the appearance, for example with
/// [`Theme::sync_system_appearance`].
pub(crate) fn install(cx: &mut App) {
    let registry = ThemeRegistry::global_mut(cx);
    if let Err(error) = registry.load_themes_from_str(THEMES) {
        eprintln!("Couldn’t load the Neon themes: {error}");
        return;
    }
    let themes = registry.themes();
    let (Some(light), Some(dark)) = (themes.get(LIGHT).cloned(), themes.get(DARK).cloned()) else {
        return;
    };
    Theme::update(cx, |theme| {
        theme.light_theme = light;
        theme.dark_theme = dark;
    });
    let mode = if Theme::global(cx).is_dark() {
        ThemeMode::Dark
    } else {
        ThemeMode::Light
    };
    Theme::change(mode, None, cx);
}

/// Which appearance the app shows: the system's, or a fixed one.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Appearance {
    #[default]
    System,
    Light,
    Dark,
}

impl Appearance {
    pub(crate) const ALL: [Self; 3] = [Self::System, Self::Light, Self::Dark];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::System => "System",
            Self::Light => "Light",
            Self::Dark => "Dark",
        }
    }
}

/// The app-wide appearance choice. Every window shares one theme.
#[derive(Default)]
struct AppearanceSetting(Appearance);

impl Global for AppearanceSetting {}

pub(crate) fn appearance(cx: &App) -> Appearance {
    cx.try_global::<AppearanceSetting>()
        .map(|setting| setting.0)
        .unwrap_or_default()
}

/// Choose the appearance for every window and show it now.
pub(crate) fn set_appearance(appearance: Appearance, window: &mut Window, cx: &mut App) {
    cx.set_global(AppearanceSetting(appearance));
    settings_file::update(cx, |settings| settings.appearance = appearance);
    apply_appearance(window, cx);
}

/// Start from a saved appearance; windows show it when they open.
pub(crate) fn restore_appearance(appearance: Appearance, cx: &mut App) {
    cx.set_global(AppearanceSetting(appearance));
}

/// Show the chosen appearance; with System, follow the window's current
/// system appearance. Call again when the system appearance changes.
pub(crate) fn apply_appearance(window: &mut Window, cx: &mut App) {
    match appearance(cx) {
        Appearance::System => Theme::sync_system_appearance(Some(window), cx),
        Appearance::Light => Theme::change(ThemeMode::Light, Some(window), cx),
        Appearance::Dark => Theme::change(ThemeMode::Dark, Some(window), cx),
    }
}
