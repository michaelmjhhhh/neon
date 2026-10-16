//! The document window: a Markdown editor beside its live preview.

mod document_file;
mod folder;
mod scroll_metrics;
mod typography;
mod workspace;

use gpui_kit::{App, KeyBinding};

pub use document_file::DocumentFile;
pub use workspace::{
    CloseWindow, LayoutMode, NewFile, Open, OpenFolder, Quit, RefreshFolder, Save, SaveAs,
    ShowEditor, ShowPreview, ShowSplit, ToggleSidebar, ToggleTheme, TypographySettings, Workspace,
};

const KEY_CONTEXT: &str = "Workspace";

/// Register the workspace's key bindings and those of its views. Call once at
/// startup, before building menus, so the menus show the shortcuts.
pub fn init(cx: &mut App) {
    markdown_preview::init(cx);
    cx.bind_keys([
        KeyBinding::new("secondary-n", NewFile, Some(KEY_CONTEXT)),
        KeyBinding::new("secondary-shift-o", OpenFolder, Some(KEY_CONTEXT)),
        KeyBinding::new("secondary-,", TypographySettings, Some(KEY_CONTEXT)),
        KeyBinding::new("secondary-b", ToggleSidebar, Some(KEY_CONTEXT)),
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
