//! Neon: a fast Markdown viewer and editor.

use std::path::PathBuf;

use gpui_kit::component::{Theme, TitleBar};
use gpui_kit::{
    App, AppContext as _, Bounds, Menu, MenuItem, OsAction, SystemMenuType, WindowBounds,
    WindowOptions, size,
};
use workspace::{
    CloseWindow, NewFile, Open, Quit, Save, SaveAs, ShowEditor, ShowPreview, ShowSplit,
    ToggleTheme, Workspace,
};

fn main() {
    let path = std::env::args_os().nth(1).map(PathBuf::from);

    gpui_kit::application()
        .with_assets(gpui_kit::assets::Assets)
        .run(move |cx| {
            gpui_kit::init(cx);
            workspace::init(cx);
            // Bindings are registered above, so the menus show their shortcuts.
            set_menus(cx);
            cx.on_action(|_: &Quit, cx| cx.quit());
            cx.activate(true);
            open_window(path, cx);
        });
}

fn open_window(path: Option<PathBuf>, cx: &mut App) {
    let bounds = Bounds::centered(None, size(gpui_kit::px(1200.), gpui_kit::px(800.)), cx);
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        window_min_size: Some(size(gpui_kit::px(480.), gpui_kit::px(320.))),
        ..TitleBar::window_options()
    };
    gpui_kit::open_window(options, cx, |window, cx| {
        Theme::sync_system_appearance(Some(window), cx);
        cx.new(|cx| {
            let mut workspace = Workspace::new(window, cx);
            if let Some(path) = path {
                workspace.open_path(path, window, cx);
            }
            workspace
        })
    })
    .expect("failed to open window");
}

fn set_menus(cx: &mut App) {
    cx.set_menus([
        Menu::new("Neon").items([
            MenuItem::os_submenu("Services", SystemMenuType::Services),
            MenuItem::separator(),
            MenuItem::action("Quit Neon", Quit),
        ]),
        Menu::new("File").items([
            MenuItem::action("New", NewFile),
            MenuItem::action("Open…", Open),
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
            MenuItem::action("Toggle Dark Mode", ToggleTheme),
        ]),
    ]);
}
