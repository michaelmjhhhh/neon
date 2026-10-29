use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Once;
use std::time::Duration;

use gpui_kit::component::WindowExt as _;
use gpui_kit::test::{TestAppContextExt as _, TestWindowExt as _};
use gpui_kit::{
    AnyWindowHandle, AppContext as _, Bounds, Entity, TestAppContext, WindowBounds, WindowOptions,
    point, px, size,
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

/// Keep Git, here and in the window, off the machine's own configuration:
/// a signing key, hooks or credential helpers there would run in tests.
fn isolate_git() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        // SAFETY: the values never change, and std serializes environment
        // access with its own reads and the processes it spawns.
        unsafe {
            std::env::set_var("GIT_CONFIG_GLOBAL", "/dev/null");
            std::env::set_var("GIT_CONFIG_NOSYSTEM", "1");
            std::env::set_var("GIT_AUTHOR_NAME", "Test");
            std::env::set_var("GIT_AUTHOR_EMAIL", "test@example.com");
            std::env::set_var("GIT_COMMITTER_NAME", "Test");
            std::env::set_var("GIT_COMMITTER_EMAIL", "test@example.com");
        }
    });
}

fn git(dir: &Path, args: &[&str]) -> String {
    isolate_git();
    let output = Command::new("git")
        .current_dir(dir)
        .args(args)
        .output()
        .expect("run git");
    assert!(output.status.success(), "git {args:?} failed");
    String::from_utf8_lossy(&output.stdout).into_owned()
}

/// A clone of an empty remote with one pushed commit, open in a window.
fn open_pushed_repository(dir: &Path, cx: &mut TestAppContext) -> (PathBuf, AnyWindowHandle) {
    let remote = dir.join("remote.git");
    let work = dir.join("work");
    git(
        dir,
        &[
            "init",
            "--quiet",
            "--bare",
            "-b",
            "main",
            remote.to_str().unwrap(),
        ],
    );
    git(
        dir,
        &[
            "clone",
            "--quiet",
            remote.to_str().unwrap(),
            work.to_str().unwrap(),
        ],
    );
    std::fs::write(work.join("notes.md"), "# Notes\n").unwrap();
    git(&work, &["add", "."]);
    git(&work, &["commit", "--quiet", "-m", "One"]);
    git(&work, &["push", "--quiet", "-u", "origin", "main"]);

    let (handle, workspace) = open_workspace(cx);
    cx.update_window(handle, |_, window, cx| {
        workspace.update(cx, |workspace, cx| {
            workspace.open_path(work.clone(), window, cx)
        });
    })
    .unwrap();
    cx.run_until_parked();
    focus_content(handle, &workspace, cx);
    (work, handle)
}

#[gpui_kit::test]
async fn push_shortcut_reads_the_status_again(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let (work, handle) = open_pushed_repository(dir.path(), cx);

    // A commit made in Terminal, which the window's status has not seen.
    std::fs::write(work.join("notes.md"), "# Notes\n\nMore.\n").unwrap();
    git(&work, &["commit", "--quiet", "-am", "Two"]);
    assert!(git(&work, &["status", "-sb"]).contains("ahead 1"));

    cx.update_window(handle, |_, window, cx| window.press("secondary-p", cx))
        .unwrap();
    cx.run_until_parked();

    assert!(!git(&work, &["status", "-sb"]).contains("ahead"));
}

#[gpui_kit::test]
async fn push_shortcut_works_after_committing_every_change(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let (work, handle) = open_pushed_repository(dir.path(), cx);
    std::fs::write(work.join("notes.md"), "# Notes\n\nMore.\n").unwrap();
    git(&work, &["add", "."]);

    cx.update_window(handle, |_, window, cx| window.press("ctrl-shift-g", cx))
        .unwrap();
    cx.run_until_parked();
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        // The message box spans the panel just above the Commit button. A
        // drag that does not move is a click at a point, not an element.
        let button = window.find("commit").bounds();
        let message = point(button.center().x, button.top() - px(12.));
        window.drag(message, message, cx);
        window.input("Two", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(handle, |_, window, cx| window.press("secondary-enter", cx))
        .unwrap();
    cx.run_until_parked();
    assert!(
        git(&work, &["status", "-sb"]).contains("ahead 1"),
        "the message box did not commit"
    );
    // The commit took every change, so the message box left the panel
    // with the focus in it.
    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("commit").is_none());
    })
    .unwrap();

    cx.update_window(handle, |_, window, cx| {
        window.render_frame(cx);
        window.press("secondary-p", cx);
    })
    .unwrap();
    cx.run_until_parked();

    assert!(!git(&work, &["status", "-sb"]).contains("ahead"));
}
