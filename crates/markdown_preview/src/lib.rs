//! A virtualized, incrementally updated Markdown preview.
//!
//! [`MarkdownPreview`] parses on a background thread and renders top-level
//! blocks through GPUI's `list`, so only visible blocks are laid out. After an
//! edit, only the blocks whose content changed are re-measured; every other
//! block keeps its cached height and the scroll position stays put.

mod blocks;
mod preview;

use gpui_kit::{App, KeyBinding};

pub use preview::{
    MarkdownPreview, MarkdownPreviewEvent, ScrollDown, ScrollPageDown, ScrollPageUp,
    ScrollToBottom, ScrollToTop, ScrollUp,
};

const KEY_CONTEXT: &str = "MarkdownPreview";

/// Register the preview's keyboard scrolling. Call once at startup.
pub fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("up", ScrollUp, Some(KEY_CONTEXT)),
        KeyBinding::new("down", ScrollDown, Some(KEY_CONTEXT)),
        KeyBinding::new("pageup", ScrollPageUp, Some(KEY_CONTEXT)),
        KeyBinding::new("pagedown", ScrollPageDown, Some(KEY_CONTEXT)),
        KeyBinding::new("shift-space", ScrollPageUp, Some(KEY_CONTEXT)),
        KeyBinding::new("space", ScrollPageDown, Some(KEY_CONTEXT)),
        KeyBinding::new("home", ScrollToTop, Some(KEY_CONTEXT)),
        KeyBinding::new("end", ScrollToBottom, Some(KEY_CONTEXT)),
        #[cfg(target_os = "macos")]
        KeyBinding::new("cmd-up", ScrollToTop, Some(KEY_CONTEXT)),
        #[cfg(target_os = "macos")]
        KeyBinding::new("cmd-down", ScrollToBottom, Some(KEY_CONTEXT)),
        #[cfg(not(target_os = "macos"))]
        KeyBinding::new("ctrl-home", ScrollToTop, Some(KEY_CONTEXT)),
        #[cfg(not(target_os = "macos"))]
        KeyBinding::new("ctrl-end", ScrollToBottom, Some(KEY_CONTEXT)),
    ]);
}
