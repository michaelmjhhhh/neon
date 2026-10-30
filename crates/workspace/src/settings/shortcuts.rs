//! The Shortcuts section's list: each command the app binds a key to,
//! grouped by menu, or by the area that must have focus for the keys to work.
//! The keys come from the keymap itself, so a row shows what `crate::init`
//! and the views actually bind; the commands are listed here by hand.

use gpui_kit::component::input::{
    Copy, Cut, Indent, IndentInline, Outdent, OutdentInline, Paste, Redo, Replace, Search,
    SelectAll, Undo,
};
use gpui_kit::{Action, App, AsKeystroke as _, KeyBinding, Keystroke};
use markdown_preview::{
    ScrollDown, ScrollPageDown, ScrollPageUp, ScrollToBottom, ScrollToTop, ScrollUp,
};

use crate::workspace::{
    CloseSettings, CloseTab, CloseWindow, CopyEntry, CopyEntryPath, ExitZen, FocusLeftPane,
    FocusRightPane, NewFile, NewFolder, NewTab, NextTab, Open, OpenFolder, OpenSettings,
    PasteEntry, PreviousTab, Push, Quit, RenameEntry, ResetZoom, RevealEntry, Save, SaveAs,
    SelectNextMatch, SelectPreviousMatch, ShowChanges, ShowEditor, ShowFiles, ShowPreview,
    ShowSearch, ShowSplit, SplitLeft, SplitRight, ToggleBold, ToggleItalic, ToggleLineNumbers,
    ToggleSidebar, ToggleZen, TrashEntry, ZoomIn, ZoomOut,
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
    /// menus show the first. Each is one keystroke, or several for a chord.
    pub(super) keys: Vec<Vec<Keystroke>>,
}

/// One command per row, named as the menus name it. A row may join several
/// actions that do one thing, such as Tab and Cmd-] for Indent.
type Row = (&'static str, Vec<Box<dyn Action>>);

fn rows(cx: &App) -> [(&'static str, Option<&'static str>, Vec<Row>); 9] {
    let a = |actions: &[&dyn Action]| actions.iter().map(|action| action.boxed_clone()).collect();
    // GPUI Kit binds these but does not export their types.
    let named = |name: &str| {
        let action = cx.build_action(name, None);
        debug_assert!(action.is_ok(), "GPUI Kit no longer registers {name}");
        action.into_iter().collect()
    };
    [
        (
            "Neon",
            None,
            vec![
                ("Settings", a(&[&OpenSettings])),
                ("Close settings", a(&[&CloseSettings])),
                ("Quit Neon", a(&[&Quit])),
            ],
        ),
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
                ("Search", a(&[&ShowSearch])),
                ("Changes", a(&[&ShowChanges])),
                ("Push", a(&[&Push])),
            ],
        ),
        (
            "Search",
            Some("While the search field has focus."),
            vec![
                ("Next match", a(&[&SelectNextMatch])),
                ("Previous match", a(&[&SelectPreviousMatch])),
            ],
        ),
        (
            "File list",
            Some("While the file list has focus."),
            vec![
                ("Copy", a(&[&CopyEntry])),
                ("Paste", a(&[&PasteEntry])),
                ("Rename", a(&[&RenameEntry])),
                ("Copy path", a(&[&CopyEntryPath])),
                ("Reveal in Finder", a(&[&RevealEntry])),
                ("Move to Trash", a(&[&TrashEntry])),
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

/// Every group with its keys read from the keymap. A command without a key
/// is left out. The keymap is complete once `crate::init` has run, so this
/// is read once rather than on each frame.
pub(super) fn all(cx: &App) -> Vec<Group> {
    let keymap = cx.key_bindings();
    let keymap = keymap.borrow();
    let bindings: Vec<&KeyBinding> = keymap.bindings().collect();
    rows(cx)
        .into_iter()
        .filter_map(|(title, description, rows)| {
            let shortcuts: Vec<_> = rows
                .into_iter()
                .filter_map(|(label, actions)| {
                    let mut keys: Vec<Vec<Keystroke>> = Vec::new();
                    for action in &actions {
                        for binding in keymap.bindings_for_action(action.as_ref()) {
                            if is_overridden(binding, &bindings) {
                                continue;
                            }
                            let key: Vec<_> = binding
                                .keystrokes()
                                .iter()
                                .map(|key| key.as_keystroke().clone())
                                .collect();
                            if !keys.contains(&key) {
                                keys.push(key);
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

/// Whether a binding made after `binding` gives its keys to another command
/// in the same context, so they no longer reach this one: ⌘⇧F, which GPUI
/// Kit binds to Replace and Neon to Search.
fn is_overridden(binding: &KeyBinding, bindings: &[&KeyBinding]) -> bool {
    let Some(ix) = bindings
        .iter()
        .position(|other| std::ptr::eq(*other, binding))
    else {
        return false;
    };
    bindings[ix + 1..].iter().any(|later| {
        later.keystrokes() == binding.keystrokes()
            && later.predicate() == binding.predicate()
            && !later.action().partial_eq(binding.action())
    })
}

/// The groups whose title or command names contain `query`, ignoring case:
/// a matching title keeps all its commands.
pub(super) fn filter<'a>(groups: &'a [Group], query: &str) -> Vec<(&'a Group, Vec<&'a Shortcut>)> {
    let query = query.trim().to_lowercase();
    groups
        .iter()
        .filter_map(|group| {
            let title_matches = group.title.to_lowercase().contains(&query);
            let shortcuts: Vec<_> = group
                .shortcuts
                .iter()
                .filter(|shortcut| title_matches || shortcut.label.to_lowercase().contains(&query))
                .collect();
            (!shortcuts.is_empty()).then_some((group, shortcuts))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn leaves_out_keys_a_later_binding_takes_in_the_same_context() {
        let replace = KeyBinding::new("cmd-shift-f", Replace, Some("Input"));
        let elsewhere = KeyBinding::new("cmd-shift-f", Replace, Some("Editor"));
        let search = KeyBinding::new("cmd-shift-f", ShowSearch, Some("Input"));
        let moved = KeyBinding::new("alt-cmd-f", Replace, Some("Input"));
        let bindings = [&replace, &elsewhere, &search, &moved];

        assert!(is_overridden(&replace, &bindings));
        // Another context keeps its keys, and so does the last binding.
        assert!(!is_overridden(&elsewhere, &bindings));
        assert!(!is_overridden(&search, &bindings));
        assert!(!is_overridden(&moved, &bindings));
    }
}
