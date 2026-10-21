//! Neon: a fast Markdown viewer and editor.

use std::path::{Path, PathBuf};

use futures::StreamExt as _;
use futures::channel::mpsc;
use gpui_kit::{
    AnyWindowHandle, App, AppContext as _, Bounds, Global, Menu, MenuItem, OsAction,
    SystemMenuType, WeakEntity, WindowBounds, WindowKind, WindowOptions, size,
};
#[cfg(feature = "diagnostics")]
use workspace::LatencyBench;
use workspace::{
    CloseTab, CloseWindow, ExportDocx, ExportHtml, ExportPdf, NewFile, NewFolder, NewTab, NextTab,
    Open, OpenFolder, OpenSettings, PreviousTab, Quit, RefreshFolder, Save, SaveAs, ShowEditor,
    ShowPreview, ShowSplit, ToggleBold, ToggleItalic, ToggleLineNumbers, ToggleSidebar,
    ToggleTheme, ToggleZen, Workspace,
};

/// Every open document window, so files opened from Finder can reuse one.
#[derive(Default)]
struct OpenWorkspaces(Vec<(AnyWindowHandle, WeakEntity<Workspace>)>);

impl Global for OpenWorkspaces {}

fn main() {
    // PDF export runs this executable again as a worker; see `export::pdf`.
    #[cfg(target_os = "macos")]
    if let Some(code) = export::pdf::run_worker_if_requested() {
        std::process::exit(code);
    }
    // Finder delivers "open file" requests outside the app context, possibly
    // before launch finishes; queue them and handle them once the app runs.
    let (open_requests, mut opened) = mpsc::unbounded::<Vec<PathBuf>>();
    let launch_paths: Vec<PathBuf> = std::env::args_os()
        .skip(1)
        .filter(|arg| !arg.to_string_lossy().starts_with("-psn_"))
        .map(PathBuf::from)
        .collect();

    let app = gpui_kit::application().with_assets(workspace::Assets);
    app.on_open_urls(move |urls| {
        let paths = urls
            .iter()
            .filter_map(|url| path_from_file_url(url))
            .collect();
        open_requests.unbounded_send(paths).ok();
    })
    .on_reopen(|cx| {
        // Clicking the Dock icon with no windows open reopens the last
        // folder and file, as a launch does.
        if cx.windows().is_empty() {
            let paths = workspace::saved_session(cx).existing_paths();
            open_window(paths, WindowKind::Normal, cx);
        }
    });

    app.run(move |cx| {
        gpui_kit::init(cx);
        workspace::init(cx);
        workspace::load_settings(cx);
        // Bindings are registered above, so the menus show their shortcuts.
        set_menus(cx);
        cx.set_global(OpenWorkspaces::default());
        cx.on_action(|_: &Quit, cx| cx.quit());
        #[cfg(feature = "memory-bench")]
        if let Some(bench) = workspace::MemoryBench::from_env() {
            open_window_with_focus(Vec::new(), WindowKind::Normal, false, cx);
            let (window, workspace) = cx.global::<OpenWorkspaces>().0.last().cloned().unwrap();
            window
                .update(cx, |_, window, cx| bench.start(workspace, window, cx))
                .ok();
            return;
        }

        cx.activate(true);

        #[cfg(feature = "diagnostics")]
        if let Some(bench) = LatencyBench::from_env() {
            start_latency_bench(bench, cx);
            return;
        }
        workspace::load_session(cx);
        // A file opened from Finder arrives before launch finishes; it takes
        // the place of the last session, which would otherwise open beside it.
        let mut requested = launch_paths;
        while let Ok(paths) = opened.try_recv() {
            requested.extend(paths);
        }
        if requested.is_empty() {
            let paths = workspace::saved_session(cx).existing_paths();
            open_window(paths, WindowKind::Normal, cx);
        } else {
            open_paths(requested, cx);
        }

        cx.spawn(async move |cx| {
            while let Some(paths) = opened.next().await {
                cx.update(|cx| open_paths(paths, cx));
            }
        })
        .detach();
    });
}

/// Show each file: bring forward the window and tab that already have it,
/// reuse the empty launch window, or open a new window.
fn open_paths(paths: Vec<PathBuf>, cx: &mut App) {
    for path in paths {
        let workspaces: Vec<_> = cx
            .global::<OpenWorkspaces>()
            .0
            .iter()
            .filter_map(|(window, workspace)| Some((*window, workspace.upgrade()?)))
            .collect();
        cx.global_mut::<OpenWorkspaces>().0 = workspaces
            .iter()
            .map(|(window, workspace)| (*window, workspace.downgrade()))
            .collect();

        let already_open = workspaces
            .iter()
            .find(|(_, workspace)| workspace.read(cx).has_open_file(&path));
        if let Some((window, workspace)) = already_open {
            let workspace = workspace.clone();
            window
                .update(cx, |_, window, cx| {
                    // Opening a file a tab already shows switches to that tab.
                    workspace.update(cx, |workspace, cx| workspace.open_path(path, window, cx));
                    window.activate_window();
                })
                .ok();
            continue;
        }

        let pristine = workspaces
            .iter()
            .find(|(_, workspace)| workspace.read(cx).is_pristine(cx));
        match pristine {
            Some((window, workspace)) => {
                let workspace = workspace.clone();
                window
                    .update(cx, |_, window, cx| {
                        workspace.update(cx, |workspace, cx| {
                            workspace.open_path(path, window, cx);
                        });
                        window.activate_window();
                    })
                    .ok();
            }
            None => open_window(vec![path], WindowKind::Normal, cx),
        }
    }
}

/// Open a window showing `paths`: a folder in the sidebar, a file in the
/// editor, or both.
fn open_window(paths: Vec<PathBuf>, kind: WindowKind, cx: &mut App) {
    open_window_with_focus(paths, kind, true, cx);
}

fn open_window_with_focus(paths: Vec<PathBuf>, kind: WindowKind, focus: bool, cx: &mut App) {
    let bounds = Bounds::centered(None, size(gpui_kit::px(1200.), gpui_kit::px(800.)), cx);
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        window_min_size: Some(size(gpui_kit::px(840.), gpui_kit::px(560.))),
        kind,
        focus,
        ..workspace::window_options()
    };
    let opened = gpui_kit::open_window(options, cx, |window, cx| {
        cx.new(|cx| {
            let mut workspace = Workspace::new(window, cx);
            for path in paths {
                workspace.open_path(path, window, cx);
            }
            workspace
        })
    });
    match opened {
        Ok((window, workspace)) => cx
            .global_mut::<OpenWorkspaces>()
            .0
            .push((window, workspace.downgrade())),
        Err(error) => eprintln!("Couldn’t open a window: {error}"),
    }
}

/// Open the benchmark fixture in a window and type into it.
#[cfg(feature = "diagnostics")]
fn start_latency_bench(bench: LatencyBench, cx: &mut App) {
    let path = match bench.fixture_path() {
        Ok(path) => path,
        Err(error) => {
            eprintln!("Couldn’t write the latency fixture: {error}");
            cx.quit();
            return;
        }
    };
    // Stay above other windows: an occluded window stops receiving frames.
    open_window(vec![path], WindowKind::PopUp, cx);
    let Some((window, workspace)) = cx.global::<OpenWorkspaces>().0.last().cloned() else {
        return;
    };
    window
        .update(cx, |_, window, cx| {
            if let Some(workspace) = workspace.upgrade() {
                bench.start(workspace, window, cx);
            }
        })
        .ok();
}

/// `file:///Users/me/My%20Notes.md` as a path; `None` for other schemes.
fn path_from_file_url(url: &str) -> Option<PathBuf> {
    let encoded = url.strip_prefix("file://")?;
    let bytes = encoded.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut ix = 0;
    while ix < bytes.len() {
        let hex = |b: u8| (b as char).to_digit(16);
        match (
            bytes[ix],
            bytes.get(ix + 1).copied(),
            bytes.get(ix + 2).copied(),
        ) {
            (b'%', Some(hi), Some(lo)) if hex(hi).is_some() && hex(lo).is_some() => {
                decoded.push((hex(hi)? * 16 + hex(lo)?) as u8);
                ix += 3;
            }
            (byte, _, _) => {
                decoded.push(byte);
                ix += 1;
            }
        }
    }
    let path = String::from_utf8(decoded).ok()?;
    Some(Path::new(&path).to_path_buf())
}

fn set_menus(cx: &mut App) {
    cx.set_menus([
        Menu::new("Neon").items([
            MenuItem::action("Settings…", OpenSettings),
            MenuItem::separator(),
            MenuItem::os_submenu("Services", SystemMenuType::Services),
            MenuItem::separator(),
            MenuItem::action("Quit Neon", Quit),
        ]),
        Menu::new("File").items([
            MenuItem::action("New Tab", NewTab),
            MenuItem::action("New File…", NewFile),
            MenuItem::action("New Folder…", NewFolder),
            MenuItem::action("Open File…", Open),
            MenuItem::action("Open Folder…", OpenFolder),
            MenuItem::action("Refresh Folder", RefreshFolder),
            MenuItem::separator(),
            MenuItem::action("Save", Save),
            MenuItem::action("Save As…", SaveAs),
            MenuItem::submenu(Menu::new("Export").items([
                MenuItem::action("HTML…", ExportHtml),
                MenuItem::action("PDF…", ExportPdf),
                MenuItem::action("Word Document…", ExportDocx),
            ])),
            MenuItem::separator(),
            MenuItem::action("Close Tab", CloseTab),
            MenuItem::action("Close Window", CloseWindow),
        ]),
        Menu::new("Edit").items([
            MenuItem::os_action("Undo", gpui_kit::component::input::Undo, OsAction::Undo),
            MenuItem::os_action("Redo", gpui_kit::component::input::Redo, OsAction::Redo),
            MenuItem::separator(),
            MenuItem::os_action("Cut", gpui_kit::component::input::Cut, OsAction::Cut),
            MenuItem::os_action("Copy", gpui_kit::component::input::Copy, OsAction::Copy),
            MenuItem::os_action("Paste", gpui_kit::component::input::Paste, OsAction::Paste),
            MenuItem::os_action(
                "Select All",
                gpui_kit::component::input::SelectAll,
                OsAction::SelectAll,
            ),
            MenuItem::separator(),
            MenuItem::action("Bold", ToggleBold),
            MenuItem::action("Italic", ToggleItalic),
        ]),
        Menu::new("View").items([
            MenuItem::action("Editor", ShowEditor),
            MenuItem::action("Split", ShowSplit),
            MenuItem::action("Preview", ShowPreview),
            MenuItem::separator(),
            MenuItem::action("Toggle Sidebar", ToggleSidebar),
            MenuItem::action("Toggle Line Numbers", ToggleLineNumbers),
            MenuItem::action("Toggle Dark Mode", ToggleTheme),
            MenuItem::action("Toggle Zen Mode", ToggleZen),
            MenuItem::separator(),
            MenuItem::action("Show Next Tab", NextTab),
            MenuItem::action("Show Previous Tab", PreviousTab),
        ]),
    ]);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_file_urls() {
        assert_eq!(
            path_from_file_url("file:///Users/me/My%20Notes.md"),
            Some(PathBuf::from("/Users/me/My Notes.md"))
        );
        assert_eq!(
            path_from_file_url("file:///tmp/%E4%B8%AD%E6%96%87.md"),
            Some(PathBuf::from("/tmp/中文.md"))
        );
        assert_eq!(path_from_file_url("https://example.com/a.md"), None);
    }
}
