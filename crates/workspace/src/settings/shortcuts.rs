//! The Shortcuts section's list: each command the app binds a key to, in
//! groups that follow the menus. The keys come from the keymap itself, so
//! the list shows what `crate::init` and the views actually bind.

use gpui_kit::component::input::{
    Copy, Cut, Indent, IndentInline, Outdent, OutdentInline, Paste, Redo, Replace, Search,
    SelectAll, Undo,
};
use gpui_kit::{Action, App, AsKeystroke as _, Keystroke};
use markdown_preview::{
    ScrollDown, ScrollPageDown, ScrollPageUp, ScrollToBottom, ScrollToTop, ScrollUp,
};

use crate::workspace::{
    CloseSettings, CloseTab, CloseWindow, CopyEntry, CopyEntryPath, ExitZen, FocusLeftPane,
    FocusRightPane, NewFile, NewFolder, NewTab, NextTab, Open, OpenFolder, OpenSettings,
    PasteEntry, PreviousTab, Quit, ResetZoom, RevealEntry, Save, SaveAs, ShowChanges, ShowEditor,
    ShowFiles, ShowPreview, ShowSplit, SplitLeft, SplitRight, ToggleBold, ToggleItalic,
    ToggleLineNumbers, ToggleSidebar, ToggleZen, ZoomIn, ZoomOut,
};

pub(super) struct Group {
    pub(super) title: &'static str,
    /// Where the keys work, for groups that need focus somewhere.
    pub(super) description: Option<&'static str>,
    pub(super) shortcuts: Vec<Shortcut>,
}

pub(super) struct Shortcut {
    pub(super) label: &'static str,
    /// Every key bound to the command, in the order they were bound: the
    /// menus show the first.
    pub(super) keys: Vec<Keystroke>,
}

/// One command per row, named as the menus name it. A row may join several
/// actions that do one thing, such as Tab and Cmd-] for Indent.
type Row = (&'static str, Vec<Box<dyn Action>>);

fn rows(cx: &App) -> [(&'static str, Option<&'static str>, Vec<Row>); 7] {
    let a = |actions: &[&dyn Action]| actions.iter().map(|action| action.boxed_clone()).collect();
    // GPUI Kit binds these but does not export their types.
    let named = |name: &str| cx.build_action(name, None).into_iter().collect();
    [
        (
            "File",
            None,
            vec![
                ("New tab", a(&[&NewTab])),
                ("New file", a(&[&NewFile])),
                ("New folder", a(&[&NewFolder])),
                ("Open file", a(&[&Open])),
                ("Open folder", a(&[&OpenFolder])),
                ("Save", a(&[&Save])),
                ("Save as", a(&[&SaveAs])),
                ("Close tab", a(&[&CloseTab])),
                ("Close window", a(&[&CloseWindow])),
                ("Settings", a(&[&OpenSettings])),
                ("Close settings", a(&[&CloseSettings])),
                ("Quit Neon", a(&[&Quit])),
            ],
        ),
        (
            "Edit",
            None,
            vec![
                ("Undo", a(&[&Undo])),
                ("Redo", a(&[&Redo])),
                ("Cut", a(&[&Cut])),
                ("Copy", a(&[&Copy])),
                ("Paste", a(&[&Paste])),
                ("Select all", a(&[&SelectAll])),
                ("Bold", a(&[&ToggleBold])),
                ("Italic", a(&[&ToggleItalic])),
                ("Find", a(&[&Search])),
                ("Find and replace", a(&[&Replace])),
                ("Indent", a(&[&IndentInline, &Indent])),
                ("Outdent", a(&[&OutdentInline, &Outdent])),
                ("Add cursor above", named("input::AddCursorAbove")),
                ("Add cursor below", named("input::AddCursorBelow")),
            ],
        ),
        (
            "View",
            None,
            vec![
                ("Editor", a(&[&ShowEditor])),
                ("Split", a(&[&ShowSplit])),
                ("Preview", a(&[&ShowPreview])),
                ("Toggle line numbers", a(&[&ToggleLineNumbers])),
                ("Toggle Zen mode", a(&[&ToggleZen])),
                ("Exit Zen mode", a(&[&ExitZen])),
                ("Zoom in", a(&[&ZoomIn])),
                ("Zoom out", a(&[&ZoomOut])),
                ("Actual size", a(&[&ResetZoom])),
            ],
        ),
        (
            "Tabs and panes",
            None,
            vec![
                ("Show next tab", a(&[&NextTab])),
                ("Show previous tab", a(&[&PreviousTab])),
                ("New pane on right", a(&[&SplitRight])),
                ("New pane on left", a(&[&SplitLeft])),
                ("Focus left pane", a(&[&FocusLeftPane])),
                ("Focus right pane", a(&[&FocusRightPane])),
            ],
        ),
        (
            "Sidebar",
            None,
            vec![
                ("Toggle sidebar", a(&[&ToggleSidebar])),
                ("Files", a(&[&ShowFiles])),
                ("Changes", a(&[&ShowChanges])),
            ],
        ),
        (
            "File list",
            Some("While the file list has focus."),
            vec![
                ("Copy", a(&[&CopyEntry])),
                ("Paste", a(&[&PasteEntry])),
                ("Copy path", a(&[&CopyEntryPath])),
                ("Reveal in Finder", a(&[&RevealEntry])),
            ],
        ),
        (
            "Preview",
            Some("While the preview has focus."),
            vec![
                ("Scroll up", a(&[&ScrollUp])),
                ("Scroll down", a(&[&ScrollDown])),
                ("Page up", a(&[&ScrollPageUp])),
                ("Page down", a(&[&ScrollPageDown])),
                ("Go to top", a(&[&ScrollToTop])),
                ("Go to bottom", a(&[&ScrollToBottom])),
            ],
        ),
    ]
}

/// The groups whose title or command names contain `query`, ignoring case,
/// with their keys read from the keymap. A command without a key is left out.
pub(super) fn groups(query: &str, cx: &App) -> Vec<Group> {
    let query = query.trim().to_lowercase();
    let keymap = cx.key_bindings();
    let keymap = keymap.borrow();
    rows(cx)
        .into_iter()
        .filter_map(|(title, description, rows)| {
            let title_matches = title.to_lowercase().contains(&query);
            let shortcuts: Vec<_> = rows
                .iter()
                .filter(|(label, _)| title_matches || label.to_lowercase().contains(&query))
                .filter_map(|(label, actions)| {
                    let mut keys: Vec<Keystroke> = Vec::new();
                    for action in actions {
                        for binding in keymap.bindings_for_action(action.as_ref()) {
                            // No binding here is a chord, so its first
                            // keystroke is the whole shortcut.
                            let Some(key) = binding.keystrokes().first() else {
                                continue;
                            };
                            let key = key.as_keystroke();
                            if !keys.contains(key) {
                                keys.push(key.clone());
                            }
                        }
                    }
                    (!keys.is_empty()).then_some(Shortcut { label, keys })
                })
                .collect();
            (!shortcuts.is_empty()).then_some(Group {
                title,
                description,
                shortcuts,
            })
        })
        .collect()
}
