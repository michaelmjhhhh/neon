//! Split panes: the sheet can show two documents side by side. The pane with
//! focus shows the active tab, so every command that acts on the document,
//! such as Save or Close Tab, acts on that one. The other pane shows another
//! tab, kept by id; both are tabs in the one strip.

use std::path::PathBuf;

use gpui_kit::component::ActiveTheme as _;
use gpui_kit::{
    Context, InteractiveElement as _, IntoElement, ParentElement as _, Render, SharedString,
    Styled as _, Window, div,
};

use super::tabs::DocumentTab;
use super::{SplitLeft, SplitRight, Workspace};

/// A pane's place in the sheet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Side {
    Left,
    Right,
}

impl Side {
    pub(super) fn opposite(self) -> Self {
        match self {
            Self::Left => Self::Right,
            Self::Right => Self::Left,
        }
    }
}

/// The sheet's second pane, while it is split.
#[derive(Clone, Copy, Debug)]
pub(super) struct PaneSplit {
    /// The tab the other pane shows. Never the active tab.
    pub(super) other_tab: u64,
    /// The side of the pane with focus, which shows the active tab.
    pub(super) active_side: Side,
}

/// A sidebar file on its way to a pane. It draws as its name under the
/// pointer.
#[derive(Clone)]
pub(super) struct DraggedFile {
    pub(super) path: PathBuf,
    pub(super) name: SharedString,
}

impl Render for DraggedFile {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        div()
            .px_2()
            .py_1()
            .rounded(theme.radius)
            .border_1()
            .border_color(theme.border)
            .bg(theme.popover)
            .shadow_md()
            .text_sm()
            .text_color(theme.popover_foreground)
            .child(self.name.clone())
    }
}

impl Workspace {
    pub(super) fn split_right(
        &mut self,
        _: &SplitRight,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.split_pane(Side::Right, window, cx);
    }

    pub(super) fn split_left(
        &mut self,
        _: &SplitLeft,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.split_pane(Side::Left, window, cx);
    }

    /// Open an empty pane on `side` of the focused document and focus it,
    /// so the next file opened lands there. Only two panes show at once:
    /// a pane already beside the document gives way, and its tab stays open.
    fn split_pane(&mut self, side: Side, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_busy() {
            return;
        }
        self.exit_zen(window, cx);
        self.set_settings_open(false, window, cx);
        if self.showing_diff() {
            self.close_diff(window, cx);
        }
        let current = self.tab().id;
        let ix = self.insert_tab(self.beside_active(side), window, cx);
        self.pane_split = Some(PaneSplit {
            other_tab: current,
            active_side: side,
        });
        self.active_tab = ix;
        self.show_active_tab(window, cx);
    }

    /// Where a tab for a pane on `side` of the active tab goes in the strip,
    /// so the strip reads in the panes' order.
    fn beside_active(&self, side: Side) -> usize {
        match side {
            Side::Left => self.active_tab,
            Side::Right => self.active_tab + 1,
        }
    }

    /// The tab the other pane shows, while it is on screen. Zen shows the
    /// focused document alone.
    pub(super) fn other_pane_tab(&self) -> Option<&DocumentTab> {
        let split = self.pane_split.filter(|_| self.zen.is_none())?;
        self.tabs.iter().find(|tab| tab.id == split.other_tab)
    }

    /// Move focus to the other pane.
    pub(super) fn focus_other_pane(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ix) = self
            .other_pane_tab()
            .and_then(|other| self.tabs.iter().position(|tab| tab.id == other.id))
        else {
            return;
        };
        self.activate_tab(ix, window, cx);
    }

    /// Focus reached a tab's editor or preview, as when clicked: if the other
    /// pane shows it, that pane becomes the focused one.
    pub(super) fn on_pane_focused(
        &mut self,
        tab_id: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self
            .other_pane_tab()
            .is_some_and(|other| other.id == tab_id)
        {
            self.focus_other_pane(window, cx);
        }
    }

    /// Show the tab at `ix` in the focused pane. A tab the other pane shows
    /// trades places with the focused one.
    fn show_in_focused_pane(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        if ix == self.active_tab {
            return;
        }
        let current = self.tab().id;
        if let Some(split) = &mut self.pane_split
            && split.other_tab == self.tabs[ix].id
        {
            split.other_tab = current;
        }
        self.active_tab = ix;
        self.show_active_tab(window, cx);
    }

    /// Open a file dropped from the sidebar on `side` of the sheet. Without
    /// a split it opens in a new pane there, beside the document; with one,
    /// it opens in the pane it was dropped on, as a click in the sidebar
    /// would, except that edits there are kept by opening it in a new tab.
    pub(super) fn drop_file(
        &mut self,
        path: PathBuf,
        side: Side,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.is_busy() || self.tab().document.path() == Some(path.as_path()) {
            return;
        }
        let Some(split) = self.pane_split else {
            let current = self.tab().id;
            let ix = match self.tab_index_of_path(&path) {
                Some(ix) => ix,
                None => self.insert_tab(self.beside_active(side), window, cx),
            };
            self.pane_split = Some(PaneSplit {
                other_tab: current,
                active_side: side,
            });
            self.active_tab = ix;
            self.show_active_tab(window, cx);
            if self.tab().document.path().is_none() {
                self.open_path(path, window, cx);
            }
            return;
        };
        if side != split.active_side {
            self.focus_other_pane(window, cx);
        }
        if self.tab().document.path() == Some(path.as_path()) {
            return;
        }
        match self.tab_index_of_path(&path) {
            Some(ix) => self.show_in_focused_pane(ix, window, cx),
            None => {
                if self.tab().document.is_dirty() {
                    let ix = self.insert_tab(self.active_tab + 1, window, cx);
                    self.show_in_focused_pane(ix, window, cx);
                }
                self.open_path(path, window, cx);
            }
        }
    }

    /// Turn on the previews on screen and off the rest, so a hidden preview
    /// does no work as its text changes.
    pub(super) fn sync_previews(&self, cx: &mut Context<Self>) {
        let shown = self.preview_shown();
        let active = self.tab().id;
        let other = self.other_pane_tab().map(|tab| tab.id);
        for tab in &self.tabs {
            let on = shown && (tab.id == active || Some(tab.id) == other);
            tab.preview
                .update(cx, |preview, cx| preview.set_active(on, cx));
        }
    }

    /// Where a dragged file can land while a drag is under way: it tints
    /// under the pointer, and a file dropped there opens on `side`.
    pub(super) fn drop_zone(&self, side: Side, cx: &mut Context<Self>) -> impl IntoElement {
        let tint = cx.theme().drop_target;
        div()
            .flex_1()
            .h_full()
            .drag_over::<DraggedFile>(move |style, _, _, _| style.bg(tint))
            .on_drop(cx.listener(move |this, file: &DraggedFile, window, cx| {
                this.drop_file(file.path.clone(), side, window, cx);
            }))
    }
}
