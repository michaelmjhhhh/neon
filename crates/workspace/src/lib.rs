//! The document window: a Markdown editor beside its live preview.

mod document_file;
mod editor_pane;
mod folder;
mod highlighter;
mod inline_format;
#[cfg(feature = "diagnostics")]
mod latency_bench;
mod list_continuation;
mod present_now;
mod scroll_metrics;
mod settings;
mod settings_file;
mod slash_commands;
mod theme;
mod typography;
mod word_count;
mod workspace;

use gpui_kit::component::TitleBar;
use gpui_kit::{App, KeyBinding, TitlebarOptions, WindowOptions, point, px};

pub use document_file::DocumentFile;
#[cfg(feature = "diagnostics")]
pub use latency_bench::LatencyBench;
pub use settings_file::load as load_settings;
#[cfg(feature = "memory-bench")]
pub use workspace::MemoryBench;
pub use workspace::{
    CloseSettings, CloseWindow, ExportDocx, ExportHtml, ExportPdf, LayoutMode, NewFile, Open,
    OpenFolder, OpenSettings, Quit, RefreshFolder, Save, SaveAs, ShowEditor, ShowPreview,
    ShowSplit, ToggleBold, ToggleItalic, ToggleSidebar, ToggleTheme, Workspace,
};

const KEY_CONTEXT: &str = "Workspace";
const SETTINGS_KEY_CONTEXT: &str = "Settings";

/// The base options for a document window. The workspace draws its own title
/// bar, and macOS centers its window controls on the document sheet's header
/// row. The 16 px controls center at 20 + 8 = 28 px: one sheet inset plus
/// half the header height at the default rem size.
pub fn window_options() -> WindowOptions {
    WindowOptions {
        titlebar: Some(TitlebarOptions {
            traffic_light_position: Some(point(px(20.), px(20.))),
            ..TitleBar::title_bar_options()
        }),
        ..TitleBar::window_options()
    }
}

/// Register the workspace's key bindings and those of its views. Call once at
/// startup, before building menus, so the menus show the shortcuts.
pub fn init(cx: &mut App) {
    theme::install(cx);
    markdown_preview::init(cx);
    cx.bind_keys([
        KeyBinding::new("secondary-n", NewFile, Some(KEY_CONTEXT)),
        KeyBinding::new("secondary-shift-o", OpenFolder, Some(KEY_CONTEXT)),
        KeyBinding::new("secondary-,", OpenSettings, Some(KEY_CONTEXT)),
        KeyBinding::new("escape", CloseSettings, Some(SETTINGS_KEY_CONTEXT)),
        KeyBinding::new("secondary-shift-b", ToggleSidebar, Some(KEY_CONTEXT)),
        KeyBinding::new("secondary-b", ToggleBold, Some(KEY_CONTEXT)),
        KeyBinding::new("secondary-i", ToggleItalic, Some(KEY_CONTEXT)),
        KeyBinding::new("secondary-o", Open, Some(KEY_CONTEXT)),
        KeyBinding::new("secondary-s", Save, Some(KEY_CONTEXT)),
        KeyBinding::new("secondary-shift-s", SaveAs, Some(KEY_CONTEXT)),
        KeyBinding::new("secondary-1", ShowEditor, Some(KEY_CONTEXT)),
        KeyBinding::new("secondary-2", ShowSplit, Some(KEY_CONTEXT)),
        KeyBinding::new("secondary-3", ShowPreview, Some(KEY_CONTEXT)),
        KeyBinding::new("secondary-w", CloseWindow, Some(KEY_CONTEXT)),
        KeyBinding::new("secondary-q", Quit, None),
    ]);
}
