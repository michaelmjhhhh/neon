//! Document tabs: each tab keeps its own file, editor and preview, so
//! switching tabs keeps every document's edits, undo history and scroll
//! position. The window shows one tab at a time.

use std::collections::HashMap;
use std::path::Path;
use std::rc::Rc;

use gpui_kit::base::{Tab, Tabs};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::EditorState;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{ActiveTheme as _, Disableable as _, IconName, Sizable as _};
use gpui_kit::{
    AppContext as _, Context, Entity, FontWeight, InteractiveElement as _, IntoElement,
    MouseButton, ParentElement as _, SharedString, StatefulInteractiveElement as _, Styled as _,
    Subscription, Window, div, prelude::FluentBuilder as _,
};
use markdown_preview::MarkdownPreview;

use super::{CloseTab, NewTab, Workspace, file_name, home_dir, split_extension};
use crate::KEY_CONTEXT;
use crate::document_file::DocumentFile;
use crate::editor_pane::EditorPane;
use crate::slash_commands::SlashCommands;
use crate::typography::Typography;
use crate::zen::ZenFocus;

/// One open document and the views that show it.
pub(super) struct DocumentTab {
    /// Stable across reordering and closing, for element ids.
    pub(super) id: u64,
    pub(super) document: DocumentFile,
    pub(super) editor: Entity<EditorState>,
    pub(super) editor_pane: Entity<EditorPane>,
    pub(super) preview: Entity<MarkdownPreview>,
    /// Cursor offset last revealed in the preview, to react to moves only.
    pub(super) revealed_cursor: Option<usize>,
    /// Made the first time this tab shows in Zen.
    pub(super) zen_focus: Option<ZenFocus>,
    _subscriptions: [Subscription; 2],
}

impl DocumentTab {
    pub(super) fn new(
        id: u64,
        line_numbers: bool,
        slash_commands: &Rc<SlashCommands>,
        typography: &Entity<Typography>,
        window: &mut Window,
        cx: &mut Context<Workspace>,
    ) -> Self {
        let editor = cx.new(|cx| {
            let mut editor = EditorState::new(window, cx)
                .language("markdown")
                .line_number(line_numbers)
                // The editor pane draws guides for nested list items instead.
                .indent_guides(false)
                // The find bar over the editor replaces the built-in panel.
                .searchable(false)
                .placeholder("Start writing, or open a Markdown file…");
            editor.set_highlighter_factory(crate::highlighter::factory(), cx);
            editor.lsp_mut().completion_provider = Some(slash_commands.clone());
            editor
        });
        let preview = cx.new(MarkdownPreview::new);
        let state = typography.read(cx);
        let (font, scale) = (state.preview_font(cx), state.preview_size(cx) / 16.);
        preview.update(cx, |preview, cx| preview.set_typography(font, scale, cx));
        let editor_pane =
            cx.new(|cx| EditorPane::new(editor.clone(), typography.clone(), window, cx));
        let subscriptions = [
            cx.subscribe_in(&editor, window, Workspace::on_editor_event),
            cx.observe_in(&editor, window, Workspace::on_editor_notified),
        ];
        Self {
            id,
            document: DocumentFile::default(),
            editor,
            editor_pane,
            preview,
            revealed_cursor: None,
            zen_focus: None,
            _subscriptions: subscriptions,
        }
    }
}

impl Workspace {
    /// The tab the window shows.
    pub(super) fn tab(&self) -> &DocumentTab {
        &self.tabs[self.active_tab]
    }

    pub(super) fn tab_mut(&mut self) -> &mut DocumentTab {
        &mut self.tabs[self.active_tab]
    }

    pub(super) fn tab_index_of_editor(&self, editor: &Entity<EditorState>) -> Option<usize> {
        self.tabs.iter().position(|tab| &tab.editor == editor)
    }

    pub(super) fn tab_index_of_path(&self, path: &Path) -> Option<usize> {
        self.tabs
            .iter()
            .position(|tab| tab.document.path() == Some(path))
    }

    /// Whether any tab in this window shows the file at `path`.
    pub fn has_open_file(&self, path: &Path) -> bool {
        self.tab_index_of_path(path).is_some()
    }

    /// Open an empty, untitled tab after the last one and show it.
    pub fn new_tab(&mut self, _: &NewTab, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_busy() {
            return;
        }
        self.next_tab_id += 1;
        let tab = DocumentTab::new(
            self.next_tab_id,
            self.line_numbers,
            &self.slash_commands,
            &self.typography,
            window,
            cx,
        );
        self.tabs.push(tab);
        if self.zen.is_some() {
            self.show_zen_in_editors(window, cx);
        }
        // Switch first: closing Settings then turns on only the new preview.
        self.activate_tab(self.tabs.len() - 1, window, cx);
        self.set_settings_open(false, window, cx);
    }

    /// Show the tab at `ix`, keeping the one it replaces as it was.
    pub fn activate_tab(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        if ix >= self.tabs.len() || ix == self.active_tab || self.is_busy() {
            return;
        }
        self.tab()
            .preview
            .update(cx, |preview, cx| preview.set_active(false, cx));
        self.active_tab = ix;
        self.show_active_tab(window, cx);
    }

    /// Show the tab with `id` again, for a command that finishes after a
    /// file panel or an alert, behind which the user may have switched tabs.
    /// False if that tab has closed, or the window is too busy to switch.
    pub(super) fn return_to_tab(
        &mut self,
        id: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(ix) = self.tabs.iter().position(|tab| tab.id == id) else {
            return false;
        };
        self.activate_tab(ix, window, cx);
        self.active_tab == ix
    }

    /// Point the window's shared views at the active tab.
    pub(super) fn show_active_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.slash_commands.dismiss();
        let tab = self.tab();
        let (editor, preview) = (tab.editor.clone(), tab.preview.clone());
        let preview_shown = self.preview_shown();
        preview.update(cx, |preview, cx| preview.set_active(preview_shown, cx));
        let source = editor.read(cx).text().clone();
        self.word_count
            .update(cx, |count, cx| count.set_source(source, cx));
        self.scroll_metrics
            .update(cx, |metrics, cx| metrics.set_panes(editor, preview, cx));
        self.tab_scroll.scroll_to_item(self.active_tab);
        self.remember_session(cx);
        self.sync_tree_selection(cx);
        self.update_window_title(window);
        self.focus_content(window, cx);
        // In Zen, light this tab's paragraph and bring its caret line to the
        // middle; its caret did not move, so the editor would not ask.
        self.zen_follow_cursor(cx);
        self.glide_zen_caret_once_drawn(window, cx);
        cx.notify();
    }

    pub(super) fn next_tab(
        &mut self,
        _: &super::NextTab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let ix = (self.active_tab + 1) % self.tabs.len();
        self.activate_tab(ix, window, cx);
    }

    pub(super) fn previous_tab(
        &mut self,
        _: &super::PreviousTab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let ix = (self.active_tab + self.tabs.len() - 1) % self.tabs.len();
        self.activate_tab(ix, window, cx);
    }

    /// Close the active tab, or the window with its last tab.
    pub(super) fn close_tab(&mut self, _: &CloseTab, window: &mut Window, cx: &mut Context<Self>) {
        self.close_tab_at(self.active_tab, window, cx);
    }

    /// Close the tab at `ix`. A tab with unsaved edits comes to the front
    /// and asks first; a clean tab in the background closes where it is.
    pub fn close_tab_at(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        if ix >= self.tabs.len() || self.is_busy() {
            return;
        }
        if self.tabs.len() == 1 {
            self.close_window(&super::CloseWindow, window, cx);
            return;
        }
        if !self.tabs[ix].document.is_dirty() {
            self.remove_tab(ix, window, cx);
            return;
        }
        self.activate_tab(ix, window, cx);
        // The confirmation shows this tab again before it runs.
        self.confirm_discard(window, cx, |this, window, cx| {
            this.remove_tab(this.active_tab, window, cx);
        });
    }

    /// Drop the tab at `ix`. Closing the shown tab shows its right
    /// neighbor, or its left one at the end.
    pub(super) fn remove_tab(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        if self.tabs.len() < 2 || ix >= self.tabs.len() {
            return;
        }
        let was_active = ix == self.active_tab;
        self.tabs.remove(ix);
        if ix < self.active_tab || self.active_tab == self.tabs.len() {
            self.active_tab -= 1;
        }
        if was_active {
            self.show_active_tab(window, cx);
        } else {
            // A closed tab may have held the window's only edits.
            self.update_window_title(window);
            cx.notify();
        }
    }

    /// The document tabs, shown in the sheet header once a window has more
    /// than one. Each reads like the single title it replaces: the name
    /// leads and the extension recedes. The shown tab is filled; the others
    /// are names only until hovered.
    pub(super) fn render_tab_strip(&self, window: &Window, cx: &Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let rem = window.rem_size();
        let count = self.tabs.len();
        // Two files with one name say which folder each is in.
        let mut name_counts: HashMap<String, usize> = HashMap::new();
        for tab in &self.tabs {
            if tab.document.path().is_some() {
                *name_counts.entry(tab.document.display_name()).or_default() += 1;
            }
        }

        let tabs = self.tabs.iter().enumerate().map(|(ix, tab)| {
            let is_active = ix == self.active_tab;
            let is_dirty = tab.document.is_dirty();
            let name = tab.document.display_name();
            let (stem, extension) = split_extension(&name);
            let parent = tab
                .document
                .path()
                .filter(|_| name_counts.get(&name).copied().unwrap_or_default() > 1)
                .and_then(Path::parent)
                .map(file_name);
            // The tooltip says where the file lives.
            let location = tab.document.path().map(|path| {
                let home = home_dir();
                match home
                    .as_deref()
                    .and_then(|home| path.strip_prefix(home).ok())
                {
                    Some(relative) => format!("~/{}", relative.display()),
                    None => path.display().to_string(),
                }
            });
            let group = SharedString::from(format!("document-tab-{}", tab.id));
            let label = if is_dirty {
                format!("{name}, edited")
            } else {
                name.clone()
            };

            let id = tab.id;
            // The trailing slot holds the unsaved dot or the close button, so
            // marking a tab edited never moves its name. Hovering a tab trades
            // its dot for the close button.
            let close = Button::new(("close-tab", tab.id))
                .xsmall()
                .ghost()
                .icon(IconName::Close)
                .accessibility_label(format!("Close “{name}”"))
                .disabled(self.is_busy())
                .on_click(cx.listener(move |this, _, window, cx| {
                    cx.stop_propagation();
                    if let Some(ix) = this.tabs.iter().position(|tab| tab.id == id) {
                        this.close_tab_at(ix, window, cx);
                    }
                }));
            let close_always_shown = is_active && !is_dirty;
            let trailing = div()
                .relative()
                .flex_none()
                .size_5()
                .child(
                    div()
                        .absolute()
                        .inset_0()
                        .when(!close_always_shown, |this| {
                            this.opacity(0.)
                                .group_hover(group.clone(), |style| style.opacity(1.))
                        })
                        .child(close),
                )
                .when(is_dirty, |this| {
                    this.child(
                        div()
                            .absolute()
                            .inset_0()
                            .flex()
                            .items_center()
                            .justify_center()
                            .group_hover(group.clone(), |style| style.opacity(0.))
                            .child(div().size_1p5().rounded_full().bg(theme.primary)),
                    )
                });

            Tab::new(group.clone())
                .group(group.clone())
                .selected(is_active)
                .accessibility_label(label)
                .set_position(ix + 1, count)
                .justify_start()
                .gap_1()
                .h_6()
                .min_w(rem * 8.)
                .max_w(rem * 14.)
                .pl_2p5()
                .pr_0p5()
                .rounded(theme.radius)
                .text_sm()
                .text_color(theme.muted_foreground)
                .map(|this| {
                    if is_active {
                        this.bg(theme.secondary_active).text_color(theme.foreground)
                    } else {
                        this.hover(|style| style.bg(theme.muted).text_color(theme.foreground))
                    }
                })
                .when_some(location, |this, location| {
                    let location = SharedString::from(location);
                    this.tooltip(move |window, cx| Tooltip::new(location.clone()).build(window, cx))
                })
                .on_click(cx.listener(move |this, _, window, cx| {
                    if let Some(ix) = this.tabs.iter().position(|tab| tab.id == id) {
                        this.activate_tab(ix, window, cx);
                    }
                }))
                .on_mouse_up(
                    MouseButton::Middle,
                    cx.listener(move |this, _, window, cx| {
                        if let Some(ix) = this.tabs.iter().position(|tab| tab.id == id) {
                            this.close_tab_at(ix, window, cx);
                        }
                    }),
                )
                .child(
                    div()
                        .flex()
                        .flex_1()
                        .items_baseline()
                        .min_w_0()
                        .child(
                            div()
                                .min_w_0()
                                .truncate()
                                .when(is_active, |this| this.font_weight(FontWeight::MEDIUM))
                                .child(stem),
                        )
                        .when_some(extension, |this, extension| {
                            this.child(
                                div()
                                    .flex_none()
                                    .text_color(theme.muted_foreground)
                                    .child(extension),
                            )
                        })
                        .when_some(parent, |this, parent| {
                            // In a narrow tab the folder gives way before
                            // the name does.
                            this.child(
                                div()
                                    .min_w_0()
                                    .flex_shrink(8.)
                                    .truncate()
                                    .ml_1p5()
                                    .text_xs()
                                    .text_color(theme.muted_foreground)
                                    .child(parent),
                            )
                        }),
                )
                .child(trailing)
        });

        // The title bar never shrinks below its content, so the strip takes
        // the leftover width from a zero basis instead of asking for its
        // tabs' full width. Tabs then narrow to their minimum and scroll.
        // `flex_1` would not do: its 0% basis falls back to the content
        // width while the title bar measures itself.
        div()
            .flex()
            .flex_grow(1.)
            .w_0()
            // Clips the scrolled tabs; the padding keeps the new tab
            // button's focus ring inside.
            .overflow_hidden()
            .py_1()
            .pr_1()
            .items_center()
            .gap_1()
            .min_w_0()
            .child(
                Tabs::new("document-tabs")
                    .flex()
                    .items_center()
                    .gap_0p5()
                    .min_w_0()
                    .overflow_x_scroll()
                    .track_scroll(&self.tab_scroll)
                    .children(tabs),
            )
            .child(
                Button::new("new-tab")
                    .small()
                    .ghost()
                    .icon(IconName::Plus)
                    .accessibility_label("New tab")
                    .tooltip_with_action("New tab", &NewTab, Some(KEY_CONTEXT))
                    .disabled(self.is_busy())
                    .on_click(cx.listener(|this, _, window, cx| this.new_tab(&NewTab, window, cx))),
            )
    }
}
