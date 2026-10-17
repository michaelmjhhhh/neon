use std::time::Duration;

use gpui_kit::component::WindowExt as _;
use gpui_kit::test::{TestAppContextExt as _, TestWindowExt as _};
use gpui_kit::{
    AnyWindowHandle, AppContext as _, Bounds, Entity, TestAppContext, WindowBounds, WindowOptions,
    px, size,
};
use markdown::BlockKind;
use workspace::{LayoutMode, Workspace};

fn open_workspace(cx: &mut TestAppContext) -> (AnyWindowHandle, Entity<Workspace>) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        workspace::init(cx);
        gpui_kit::open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                    Default::default(),
                    size(px(1000.), px(700.)),
                ))),
                ..Default::default()
            },
            cx,
            |window, cx| cx.new(|cx| Workspace::new(window, cx)),
        )
        .expect("open test window")
    })
}

/// Focus the editor once it has been laid out; focus requests made before
/// the first frame have no element to land on.
fn focus_content(handle: AnyWindowHandle, workspace: &Entity<Workspace>, cx: &mut TestAppContext) {
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        workspace.update(cx, |workspace, cx| workspace.focus_content(window, cx));
        window.render_frame(cx);
    })
    .unwrap();
}

fn editor_text(workspace: &Entity<Workspace>, cx: &mut TestAppContext) -> String {
    cx.read(|cx| workspace.read(cx).editor().read(cx).value().to_string())
}

#[gpui_kit::test]
async fn edits_flow_to_the_preview_and_save_to_disk(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("notes.md");
    std::fs::write(&path, "# Notes\n\nFirst paragraph.\n").unwrap();

    let (handle, workspace) = open_workspace(cx);
    cx.update_window(handle, |_, window, cx| {
        workspace.update(cx, |workspace, cx| {
            workspace.open_path(path.clone(), window, cx)
        });
    })
    .unwrap();
    cx.run_until_parked();

    cx.read(|cx| {
        let workspace = workspace.read(cx);
        assert_eq!(workspace.document().display_name(), "notes.md");
        assert!(!workspace.document().is_dirty());
        assert!(!workspace.is_loading());
        assert_eq!(workspace.preview().read(cx).document().blocks().len(), 2);
    });

    // Type at the end of the document through real key events.
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.press("secondary-down", cx);
        window.input("\nA *new* block.", cx);
    })
    .unwrap();
    // The preview catches up with typing at most every 200 ms.
    cx.executor().advance_clock(Duration::from_millis(200));
    cx.run_until_parked();

    assert_eq!(
        editor_text(&workspace, cx),
        "# Notes\n\nFirst paragraph.\n\nA *new* block."
    );
    cx.read(|cx| {
        let workspace = workspace.read(cx);
        assert!(workspace.document().is_dirty());
        let preview = workspace.preview().read(cx);
        let blocks = preview.document().blocks();
        assert_eq!(blocks.len(), 3);
        let BlockKind::Paragraph(text) = blocks[2].kind() else {
            panic!("expected the typed paragraph");
        };
        assert_eq!(&**text.text(), "A new block.");
    });

    cx.update_window(handle, |_, window, cx| window.press("secondary-s", cx))
        .unwrap();
    cx.run_until_parked();

    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "# Notes\n\nFirst paragraph.\n\nA *new* block."
    );
    cx.read(|cx| assert!(!workspace.read(cx).document().is_dirty()));
}

#[gpui_kit::test]
async fn layout_shortcuts_switch_panes(cx: &mut TestAppContext) {
    let (handle, workspace) = open_workspace(cx);
    focus_content(handle, &workspace, cx);
    cx.update_window(handle, |_, window, cx| window.press("secondary-3", cx))
        .unwrap();
    cx.read(|cx| assert_eq!(workspace.read(cx).layout(), LayoutMode::Preview));

    // The preview now has focus, so the workspace bindings still reach it.
    cx.update_window(handle, |_, window, cx| window.press("secondary-1", cx))
        .unwrap();
    cx.read(|cx| assert_eq!(workspace.read(cx).layout(), LayoutMode::Editor));
}

#[gpui_kit::test]
async fn new_file_asks_before_discarding_edits(cx: &mut TestAppContext) {
    let (handle, workspace) = open_workspace(cx);
    focus_content(handle, &workspace, cx);
    cx.update_window(handle, |_, window, cx| window.input("Draft", cx))
        .unwrap();
    assert_eq!(editor_text(&workspace, cx), "Draft");

    cx.update_window(handle, |_, window, cx| window.press("secondary-n", cx))
        .unwrap();
    cx.run_until_parked();

    // The edit is still there until the user confirms.
    assert_eq!(editor_text(&workspace, cx), "Draft");
    cx.update_window(handle, |_, window, cx| {
        assert!(window.has_active_dialog(cx));
        window.press("escape", cx);
    })
    .unwrap();
    cx.wait_for(handle, Duration::from_secs(1), |window, cx| {
        !window.has_active_dialog(cx)
    })
    .await;
    assert_eq!(editor_text(&workspace, cx), "Draft");
    cx.read(|cx| assert!(workspace.read(cx).document().is_dirty()));
}
