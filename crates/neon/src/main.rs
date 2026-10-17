//! Neon: a fast Markdown viewer and editor.

use std::path::{Path, PathBuf};

use futures::StreamExt as _;
use futures::channel::mpsc;
use gpui_kit::component::{Theme, TitleBar};
use gpui_kit::{
    AnyWindowHandle, App, AppContext as _, Bounds, Global, Menu, MenuItem, OsAction,
    SystemMenuType, WeakEntity, WindowBounds, WindowKind, WindowOptions, size,
};
use workspace::{
    CloseWindow, LatencyBench, NewFile, Open, OpenFolder, Quit, RefreshFolder, Save, SaveAs,
    ShowEditor, ShowPreview, ShowSplit, ToggleSidebar, ToggleTheme, TypographySettings, Workspace,
};

/// Every open document window, so files opened from Finder can reuse one.
#[derive(Default)]
struct OpenWorkspaces(Vec<(AnyWindowHandle, WeakEntity<Workspace>)>);

impl Global for OpenWorkspaces {}

fn main() {
    // Finder delivers "open file" requests outside the app context, possibly
    // before launch finishes; queue them and handle them once the app runs.
    let (open_requests, mut opened) = mpsc::unbounded::<Vec<PathBuf>>();
    let launch_paths: Vec<PathBuf> = std::env::args_os()
        .skip(1)
        .filter(|arg| !arg.to_string_lossy().starts_with("-psn_"))
        .map(PathBuf::from)
        .collect();

    let app = gpui_kit::application().with_assets(gpui_kit::assets::Assets);
    app.on_open_urls(move |urls| {
        let paths = urls
            .iter()
            .filter_map(|url| path_from_file_url(url))
            .collect();
        open_requests.unbounded_send(paths).ok();
    })
    .on_reopen(|cx| {
        // Clicking the Dock icon with no windows open starts a new document.
        if cx.windows().is_empty() {
            open_window(None, WindowKind::Normal, cx);
        }
    });

    app.run(move |cx| {
        gpui_kit::init(cx);
        workspace::init(cx);
        // Bindings are registered above, so the menus show their shortcuts.
        set_menus(cx);
        cx.set_global(OpenWorkspaces::default());
        cx.on_action(|_: &Quit, cx| cx.quit());
        cx.activate(true);

        if let Some(bench) = LatencyBench::from_env() {
            start_latency_bench(bench, cx);
        } else if launch_paths.is_empty() {
            open_window(None, WindowKind::Normal, cx);
        } else {
            open_paths(launch_paths, cx);
        }

        cx.spawn(async move |cx| {
            while let Some(paths) = opened.next().await {
                cx.update(|cx| open_paths(paths, cx));
            }
        })
        .detach();
    });
}

/// Show each file: focus a window that already has it, reuse the empty launch
/// window, or open a new window.
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
            .find(|(_, workspace)| workspace.read(cx).document().path() == Some(path.as_path()));
        if let Some((window, _)) = already_open {
            window
                .update(cx, |_, window, _| window.activate_window())
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
            None => open_window(Some(path), WindowKind::Normal, cx),
        }
    }
}

fn open_window(path: Option<PathBuf>, kind: WindowKind, cx: &mut App) {
    let bounds = Bounds::centered(None, size(gpui_kit::px(1200.), gpui_kit::px(800.)), cx);
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        window_min_size: Some(size(gpui_kit::px(840.), gpui_kit::px(560.))),
        kind,
        ..TitleBar::window_options()
    };
    let opened = gpui_kit::open_window(options, cx, |window, cx| {
        Theme::sync_system_appearance(Some(window), cx);
        cx.new(|cx| {
            let mut workspace = Workspace::new(window, cx);
            if let Some(path) = path {
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
    open_window(Some(path), WindowKind::PopUp, cx);
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
            MenuItem::os_submenu("Services", SystemMenuType::Services),
            MenuItem::separator(),
            MenuItem::action("Quit Neon", Quit),
        ]),
        Menu::new("File").items([
            MenuItem::action("New File…", NewFile),
            MenuItem::action("Open File…", Open),
            MenuItem::action("Open Folder…", OpenFolder),
            MenuItem::action("Refresh Folder", RefreshFolder),
            MenuItem::separator(),
            MenuItem::action("Save", Save),
            MenuItem::action("Save As…", SaveAs),
            MenuItem::separator(),
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
        ]),
        Menu::new("View").items([
            MenuItem::action("Editor", ShowEditor),
            MenuItem::action("Split", ShowSplit),
            MenuItem::action("Preview", ShowPreview),
            MenuItem::separator(),
            MenuItem::action("Toggle Sidebar", ToggleSidebar),
            MenuItem::action("Typography…", TypographySettings),
            MenuItem::action("Toggle Dark Mode", ToggleTheme),
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
