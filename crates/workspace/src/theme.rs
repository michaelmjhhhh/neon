//! Neon's light and dark themes: graphite surfaces and one orange accent,
//! the filament from the app icon, reserved for state (caret, selection,
//! the unsaved marker, checked tasks).

use gpui_kit::App;
use gpui_kit::component::{Theme, ThemeMode, ThemeRegistry};

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
