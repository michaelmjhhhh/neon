//! The document window: a Markdown editor beside its live preview.

mod change_tree;
mod commit_graph;
mod document_file;
mod editor_pane;
mod find_bar;
mod folder;
mod git;
mod highlighter;
mod inline_format;
#[cfg(feature = "diagnostics")]
mod latency_bench;
mod list_continuation;
mod list_guides;
mod present_now;
mod scroll_anchor;
mod scroll_metrics;
mod session_file;
mod settings;
mod settings_file;
mod slash_commands;
mod slash_menu;
mod theme;
mod typography;
mod word_count;
mod workspace;
mod zen;

use gpui_kit::component::TitleBar;
use gpui_kit::{App, KeyBinding, TitlebarOptions, WindowOptions, point, px};

pub use document_file::DocumentFile;
#[cfg(feature = "diagnostics")]
pub use latency_bench::LatencyBench;
pub use session_file::{Session, load as load_session, saved as saved_session};
pub use settings_file::load as load_settings;
#[cfg(feature = "memory-bench")]
pub use workspace::MemoryBench;
pub use workspace::{
    CloseSettings, CloseTab, CloseWindow, Commit, ExitZen, ExportDocx, ExportHtml, ExportPdf,
    FocusLeftPane, FocusRightPane, LayoutMode, NewFile, NewFolder, NewTab, NextTab, Open,
    OpenFolder, OpenSettings, PreviousTab, Quit, RefreshFolder, ResetZoom, Save, SaveAs,
    ShowEditor, ShowPreview, ShowSplit, SplitLeft, SplitRight, ToggleBold, ToggleItalic,
    ToggleLineNumbers, ToggleSidebar, ToggleTheme, ToggleZen, Workspace, ZoomIn, ZoomOut,
};
pub use workspace::{Push, ShowChanges, ShowFiles, ShowHistory, StageAll, UnstageAll};

const KEY_CONTEXT: &str = "Workspace";
/// The sidebar's file tree while it has focus. Its Copy and Paste act on
/// files, so they bind here alone and the editor keeps Cmd-C and Cmd-V.
const TREE_KEY_CONTEXT: &str = "Workspace > Tree";

gpui_kit::assets::icon_assets!(
    ExtraIcons,
    [
        CircleQuestionMark,
        Columns2,
        FilePlus,
        FolderPlus,
        Focus,
        GitBranch,
        GitCompare,
        GitGraph,
        ImageOff,
        Keyboard,
        Lightbulb,
        PenLine,
        Quote,
    ]
);

/// The icons the app draws: GPUI Kit's default set, plus the few beyond it
/// that the workspace and the preview's callouts use.
pub struct Assets;

impl gpui_kit::AssetSource for Assets {
    fn load(&self, path: &str) -> gpui_kit::Result<Option<std::borrow::Cow<'static, [u8]>>> {
        match ExtraIcons.load(path)? {
            Some(data) => Ok(Some(data)),
            None => gpui_kit::assets::Assets.load(path),
        }
    }

    fn list(&self, path: &str) -> gpui_kit::Result<Vec<gpui_kit::SharedString>> {
        let mut paths = gpui_kit::assets::Assets.list(path)?;
        paths.extend(ExtraIcons.list(path)?);
        Ok(paths)
    }
}
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
        KeyBinding::new("secondary-shift-n", NewFolder, Some(KEY_CONTEXT)),
        KeyBinding::new("secondary-c", workspace::CopyEntry, Some(TREE_KEY_CONTEXT)),
        KeyBinding::new("secondary-v", workspace::PasteEntry, Some(TREE_KEY_CONTEXT)),
        // Return renames, as in Finder. The name field binds Return itself
        // and wins over this while it is open.
        KeyBinding::new("enter", workspace::RenameEntry, Some(TREE_KEY_CONTEXT)),
        // As in Finder.
        KeyBinding::new(
            "secondary-backspace",
            workspace::TrashEntry,
            Some(TREE_KEY_CONTEXT),
        ),
        // As in Zed and VS Code.
        KeyBinding::new(
            "alt-secondary-c",
            workspace::CopyEntryPath,
            Some(TREE_KEY_CONTEXT),
        ),
        KeyBinding::new(
            "alt-secondary-r",
            workspace::RevealEntry,
            Some(TREE_KEY_CONTEXT),
        ),
        KeyBinding::new("secondary-shift-o", OpenFolder, Some(KEY_CONTEXT)),
        KeyBinding::new("secondary-,", OpenSettings, Some(KEY_CONTEXT)),
        KeyBinding::new("escape", CloseSettings, Some(SETTINGS_KEY_CONTEXT)),
        KeyBinding::new("secondary-shift-b", ToggleSidebar, Some(KEY_CONTEXT)),
        // Files and Changes in the sidebar, with VS Code's shortcuts on macOS.
        KeyBinding::new("secondary-shift-e", ShowFiles, Some(KEY_CONTEXT)),
        KeyBinding::new("ctrl-shift-g", ShowChanges, Some(KEY_CONTEXT)),
        KeyBinding::new("secondary-p", Push, Some(KEY_CONTEXT)),
        KeyBinding::new("secondary-shift-l", ToggleLineNumbers, Some(KEY_CONTEXT)),
        // Z for Zen. macOS menus cannot show Enter, so not Cmd-Shift-Enter.
        KeyBinding::new("ctrl-secondary-z", ToggleZen, Some(KEY_CONTEXT)),
        // Fields and the editor bind Escape themselves and win over this.
        KeyBinding::new("escape", ExitZen, Some(KEY_CONTEXT)),
        // The menu shows the first binding; Cmd-+ also works, as in Safari.
        KeyBinding::new("secondary-=", ZoomIn, Some(KEY_CONTEXT)),
        KeyBinding::new("secondary-+", ZoomIn, Some(KEY_CONTEXT)),
        KeyBinding::new("secondary--", ZoomOut, Some(KEY_CONTEXT)),
        KeyBinding::new("secondary-0", ResetZoom, Some(KEY_CONTEXT)),
        KeyBinding::new("secondary-b", ToggleBold, Some(KEY_CONTEXT)),
        KeyBinding::new("secondary-i", ToggleItalic, Some(KEY_CONTEXT)),
        KeyBinding::new("secondary-o", Open, Some(KEY_CONTEXT)),
        KeyBinding::new("secondary-s", Save, Some(KEY_CONTEXT)),
        KeyBinding::new("secondary-shift-s", SaveAs, Some(KEY_CONTEXT)),
        KeyBinding::new("secondary-1", ShowEditor, Some(KEY_CONTEXT)),
        KeyBinding::new("secondary-2", ShowSplit, Some(KEY_CONTEXT)),
        KeyBinding::new("secondary-3", ShowPreview, Some(KEY_CONTEXT)),
        KeyBinding::new("secondary-t", NewTab, Some(KEY_CONTEXT)),
        KeyBinding::new("secondary-w", CloseTab, Some(KEY_CONTEXT)),
        KeyBinding::new("secondary-shift-w", CloseWindow, Some(KEY_CONTEXT)),
        // The menu shows the first binding: Control-Tab, as in Safari.
        KeyBinding::new("ctrl-tab", NextTab, Some(KEY_CONTEXT)),
        KeyBinding::new("ctrl-shift-tab", PreviousTab, Some(KEY_CONTEXT)),
        KeyBinding::new("secondary-}", NextTab, Some(KEY_CONTEXT)),
        // As in iTerm. Not Duplicate in the sidebar, as in Finder: a click on
        // a file leaves the tree focused, so Cmd-D right after it would
        // copy the file instead of splitting.
        KeyBinding::new("secondary-d", SplitRight, Some(KEY_CONTEXT)),
        KeyBinding::new("secondary-shift-d", SplitLeft, Some(KEY_CONTEXT)),
        // The editor leaves Option-Command arrows unbound.
        KeyBinding::new("alt-secondary-left", FocusLeftPane, Some(KEY_CONTEXT)),
        KeyBinding::new("alt-secondary-right", FocusRightPane, Some(KEY_CONTEXT)),
        KeyBinding::new("secondary-{", PreviousTab, Some(KEY_CONTEXT)),
        KeyBinding::new("secondary-q", Quit, None),
    ]);
}
