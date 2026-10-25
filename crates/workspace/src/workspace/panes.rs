//! Split panes: a tab in the strip can hold two documents side by side, as
//! one split tab. The pane with focus shows the active document, so every
//! command that acts on the document, such as Save, acts on that one.
//!
//! Each document keeps its own entry in [`Workspace::tabs`]; a split pairs
//! two of them, kept next to each other there, and the strip draws the pair
//! as one tab.

use std::path::PathBuf;

use gpui_kit::component::ActiveTheme as _;
use gpui_kit::{
    Context, InteractiveElement as _, IntoElement, ParentElement as _, Pixels, Point, Render,
    SharedString, Styled as _, Window, div, px,
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

/// Two documents shown as one split tab, by tab id. The left one comes
/// right before the right one in the tabs.
#[derive(Clone, Copy, Debug)]
pub(super) struct PaneSplit {
    pub(super) left: u64,
    pub(super) right: u64,
    /// The pane that had focus last, which a click on the tab returns to.
    pub(super) focused: Side,
}

impl PaneSplit {
    pub(super) fn side_of(&self, id: u64) -> Option<Side> {
        if id == self.left {
            Some(Side::Left)
        } else if id == self.right {
            Some(Side::Right)
        } else {
            None
        }
    }

    pub(super) fn id(&self, side: Side) -> u64 {
        match side {
            Side::Left => self.left,
            Side::Right => self.right,
        }
    }
}

/// A sidebar file on its way to a pane. It draws as its name beside the
/// pointer.
#[derive(Clone)]
pub(super) struct DraggedFile {
    pub(super) path: PathBuf,
    pub(super) name: SharedString,
    /// Where the row was pressed, from its corner. A drag draws from the
    /// pressed row's corner, so the name is pushed this far to sit by the
    /// pointer, as Zed's file drags do.
    pub(super) press_offset: Point<Pixels>,
}

impl Render for DraggedFile {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let chip = div()
            .px_2()
            .py_1()
            .rounded(theme.radius)
            .border_1()
            .border_color(theme.border)
            .bg(theme.popover)
            .shadow_md()
            .text_sm()
            .text_color(theme.popover_foreground)
            .child(self.name.clone());
        div()
            .pl(self.press_offset.x + px(12.))
            .pt(self.press_offset.y + px(8.))
            .child(chip)
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
    /// so the next file opened lands there. A tab holds two panes at most:
    /// the pane already beside the document gives way, to a tab of its own.
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
        let ix = self.insert_tab(self.tabs.len(), window, cx);
        let new = self.tabs[ix].id;
        self.pair(new, side, current);
        self.show_tab_id(new, window, cx);
    }

    /// The split the tab with `id` is in, by index.
    pub(super) fn split_index_of(&self, id: u64) -> Option<usize> {
        self.splits
            .iter()
            .position(|split| split.side_of(id).is_some())
    }

    /// The split the active document is in, and its side there.
    pub(super) fn active_split(&self) -> Option<(PaneSplit, Side)> {
        let id = self.tab().id;
        let split = self.splits[self.split_index_of(id)?];
        Some((split, split.side_of(id)?))
    }

    /// The document the other pane shows, while it is on screen. Zen shows
    /// the focused document alone.
    pub(super) fn other_pane_tab(&self) -> Option<&DocumentTab> {
        let (split, side) = self.active_split().filter(|_| self.zen.is_none())?;
        let other = split.id(side.opposite());
        self.tabs.iter().find(|tab| tab.id == other)
    }

    /// Show the document with tab id `focused` on `side` and `other` beside
    /// it, as one split tab where the earlier of the two was in the strip.
    /// Either one's split, if any, ends: its other document keeps a tab of
    /// its own.
    fn pair(&mut self, focused: u64, side: Side, other: u64) {
        self.unpair(focused);
        self.unpair(other);
        let (left, right) = match side {
            Side::Left => (focused, other),
            Side::Right => (other, focused),
        };
        let active = self.tab().id;
        let position = |tabs: &[DocumentTab], id| tabs.iter().position(|tab| tab.id == id);
        let (Some(left_ix), Some(right_ix)) =
            (position(&self.tabs, left), position(&self.tabs, right))
        else {
            return;
        };
        let at = left_ix.min(right_ix);
        let (first, second) = if left_ix < right_ix {
            (right_ix, left_ix)
        } else {
            (left_ix, right_ix)
        };
        let mut removed = [self.tabs.remove(first), self.tabs.remove(second)];
        if removed[0].id != right {
            removed.swap(0, 1);
        }
        let [right_tab, left_tab] = removed;
        self.tabs.insert(at, right_tab);
        self.tabs.insert(at, left_tab);
        self.active_tab = position(&self.tabs, active).unwrap_or_default();
        self.splits.push(PaneSplit {
            left,
            right,
            focused: side,
        });
    }

    /// End the split the tab with `id` is in, if any. Both documents stay
    /// open, each in a tab of its own.
    pub(super) fn unpair(&mut self, id: u64) {
        if let Some(ix) = self.split_index_of(id) {
            self.splits.remove(ix);
        }
    }

    /// Show the document with tab id `id`, with its split partner if any.
    fn show_tab_id(&mut self, id: u64, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(ix) = self.tabs.iter().position(|tab| tab.id == id) {
            self.active_tab = ix;
            self.show_active_tab(window, cx);
        }
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

    /// Move focus to the pane on `side` of a split tab.
    pub(super) fn focus_pane(&mut self, side: Side, window: &mut Window, cx: &mut Context<Self>) {
        if self
            .active_split()
            .is_some_and(|(_, active_side)| active_side != side)
        {
            self.focus_other_pane(window, cx);
        }
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

    /// Open a file dropped from the sidebar on `side` of the sheet. Without
    /// a split it joins the document as a split tab, on that side; with one,
    /// it takes the place of the pane it was dropped on, whose document keeps
    /// a tab of its own. A document already open moves rather than opening
    /// twice, and a blank document, as in a pane just opened with Cmd-D,
    /// takes the file in place.
    pub(super) fn drop_file(
        &mut self,
        path: PathBuf,
        side: Side,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.dragging_file.set(false);
        if self.is_busy() {
            return;
        }
        let open = self.tab_id_of_path(&path);
        // The document the dropped one goes beside.
        let beside = match self.active_split() {
            Some((split, _)) => {
                let (here, there) = (split.id(side), split.id(side.opposite()));
                if open == Some(here) {
                    self.show_tab_id(here, window, cx);
                    return;
                }
                // The other pane's document: the two trade places.
                if open == Some(there) {
                    self.pair(there, side, here);
                    self.show_tab_id(there, window, cx);
                    return;
                }
                if open.is_none() && self.is_blank(here, cx) {
                    self.show_tab_id(here, window, cx);
                    self.open_path(path, window, cx);
                    return;
                }
                there
            }
            None if open == Some(self.tab().id) => return,
            // Nothing to keep beside the file.
            None if open.is_none() && self.is_blank(self.tab().id, cx) => {
                self.open_path(path, window, cx);
                return;
            }
            None => self.tab().id,
        };
        let target = match open {
            Some(id) => id,
            None => {
                let ix = self.insert_tab(self.tabs.len(), window, cx);
                self.tabs[ix].id
            }
        };
        self.pair(target, side, beside);
        self.show_tab_id(target, window, cx);
        if self.tab().document.path().is_none() {
            self.open_path(path, window, cx);
        }
    }

    /// Whether the document with tab id `id` is untitled and empty, with
    /// nothing a file opened in its place would lose.
    fn is_blank(&self, id: u64, cx: &gpui_kit::App) -> bool {
        self.tabs.iter().any(|tab| {
            tab.id == id
                && tab.document.path().is_none()
                && !tab.document.is_dirty()
                && tab.editor.read(cx).text().len() == 0
        })
    }

    fn tab_id_of_path(&self, path: &std::path::Path) -> Option<u64> {
        self.tab_index_of_path(path).map(|ix| self.tabs[ix].id)
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
