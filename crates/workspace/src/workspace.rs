use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::base::ResizeHandleState;
use gpui_kit::component::button::ButtonVariant;
use gpui_kit::component::input::{Editor, EditorState, InputEvent};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::resizable::{h_resizable, resizable_panel, resize_handle_appearance};
use gpui_kit::component::status_bar::StatusBar;
use gpui_kit::component::tab::{Tab, TabBar};
use gpui_kit::component::{ActiveTheme as _, Theme, ThemeMode, TitleBar, WindowExt as _};
use gpui_kit::{
    AnyElement, AppContext as _, Context, Entity, FocusHandle, Focusable, InteractiveElement as _,
    IntoElement, ParentElement as _, PathPromptOptions, Pixels, Render, SharedString, Styled as _,
    Subscription, Task, Window, div, prelude::FluentBuilder as _,
};
use markdown_preview::{MarkdownPreview, MarkdownPreviewEvent};

use crate::KEY_CONTEXT;
use crate::document_file::DocumentFile;
use crate::scroll_metrics::ScrollMetrics;

gpui_kit::actions!(
    workspace,
    [
        NewFile,
        Open,
        Save,
        SaveAs,
        ShowEditor,
        ShowSplit,
        ShowPreview,
        ToggleTheme,
        CloseWindow,
        Quit
    ]
);

/// Which panes the window shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LayoutMode {
    Editor,
    #[default]
    Split,
    Preview,
}

impl LayoutMode {
    const ALL: [Self; 3] = [Self::Editor, Self::Split, Self::Preview];

    fn label(self) -> &'static str {
        match self {
            Self::Editor => "Editor",
            Self::Split => "Split",
            Self::Preview => "Preview",
        }
    }

    fn shows_editor(self) -> bool {
        self != Self::Preview
    }
}

/// One document window: owns the open file, its editor, and its preview.
pub struct Workspace {
    document: DocumentFile,
    editor: Entity<EditorState>,
    preview: Entity<MarkdownPreview>,
    scroll_metrics: Entity<ScrollMetrics>,
    layout: LayoutMode,
    focus_handle: FocusHandle,
    /// Cursor offset last revealed in the preview, to react to moves only.
    revealed_cursor: Option<usize>,
    load_task: Option<Task<()>>,
    save_task: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl Workspace {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let editor = cx.new(|cx| {
            EditorState::new(window, cx)
                .language("markdown")
                .placeholder("Start writing, or open a Markdown file…")
        });
        let preview = cx.new(MarkdownPreview::new);
        let scroll_metrics =
            cx.new(|cx| ScrollMetrics::new(editor.clone(), preview.clone(), window, cx));

        let subscriptions = vec![
            cx.subscribe_in(&editor, window, Self::on_editor_event),
            cx.observe_in(&editor, window, Self::on_editor_notified),
            cx.subscribe(&preview, |_, _, event, cx| match event {
                MarkdownPreviewEvent::DocumentChanged => cx.notify(),
            }),
            cx.observe_window_appearance(window, |_, window, cx| {
                Theme::sync_system_appearance(Some(window), cx);
            }),
        ];

        let workspace = Self {
            document: DocumentFile::default(),
            editor,
            preview,
            scroll_metrics,
            layout: LayoutMode::default(),
            focus_handle: cx.focus_handle(),
            revealed_cursor: None,
            load_task: None,
            save_task: None,
            _subscriptions: subscriptions,
        };
        workspace.install_close_guard(window, cx);
        workspace.update_window_title(window);
        // Focus can only land on an element that has been rendered.
        cx.on_next_frame(window, |this, window, cx| this.focus_content(window, cx));
        workspace
    }

    pub fn editor(&self) -> &Entity<EditorState> {
        &self.editor
    }

    pub fn preview(&self) -> &Entity<MarkdownPreview> {
        &self.preview
    }

    pub fn document(&self) -> &DocumentFile {
        &self.document
    }

    pub fn layout(&self) -> LayoutMode {
        self.layout
    }

    pub fn is_loading(&self) -> bool {
        self.load_task.is_some()
    }

    /// Whether this is still the empty, untitled window a launch opens, so a
    /// file opened next can take it over instead of opening another window.
    pub fn is_pristine(&self, cx: &gpui_kit::App) -> bool {
        self.document.path().is_none()
            && !self.document.is_dirty()
            && !self.is_loading()
            && self.editor.read(cx).text().len() == 0
    }

    /// Focus the pane that takes keyboard input in the current layout.
    pub fn focus_content(&self, window: &mut Window, cx: &mut Context<Self>) {
        if self.layout.shows_editor() {
            self.editor
                .update(cx, |editor, cx| editor.focus(window, cx));
        } else {
            let handle = self.preview.read(cx).focus_handle(cx);
            handle.focus(window, cx);
        }
    }

    /// Read `path` on a background thread and show it.
    pub fn open_path(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let read = cx.background_spawn({
            let path = path.clone();
            async move { std::fs::read_to_string(&path) }
        });
        self.load_task = Some(cx.spawn_in(window, async move |this, cx| {
            let result = read.await;
            this.update_in(cx, |this, window, cx| {
                this.load_task = None;
                match result {
                    Ok(text) => this.show_document(Some(path), &text, window, cx),
                    Err(error) => {
                        let name = file_name(&path);
                        window.push_notification(
                            Notification::error(format!("Couldn’t open “{name}”. {error}.")),
                            cx,
                        );
                    }
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    fn show_document(
        &mut self,
        path: Option<PathBuf>,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.editor
            .update(cx, |editor, cx| editor.set_value(text, window, cx));
        let source = self.editor.read(cx).text().clone();
        let base_dir = path.as_deref().and_then(Path::parent).map(Arc::from);
        self.preview
            .update(cx, |preview, cx| preview.load_source(source, base_dir, cx));
        self.document = path.map(DocumentFile::saved).unwrap_or_default();
        self.revealed_cursor = None;
        self.update_window_title(window);
        self.focus_content(window, cx);
        cx.notify();
    }

    fn on_editor_event(
        &mut self,
        editor: &Entity<EditorState>,
        event: &InputEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !matches!(event, InputEvent::Change) {
            return;
        }
        let was_dirty = self.document.is_dirty();
        self.document.record_edit();
        // Cloning the rope is O(1); the preview flattens it off the UI thread.
        let source = editor.read(cx).text().clone();
        self.preview
            .update(cx, |preview, cx| preview.set_source(source, cx));
        // Only the first edit changes this view; the cursor position and word
        // count notify through their own paths, so typing stays cheap.
        if !was_dirty {
            self.update_window_title(window);
            cx.notify();
        }
    }

    /// Keep the preview on the block being edited when the cursor moves.
    fn on_editor_notified(
        &mut self,
        editor: Entity<EditorState>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let cursor = editor.read(cx).cursor();
        if self.revealed_cursor == Some(cursor) {
            return;
        }
        self.revealed_cursor = Some(cursor);
        if self.layout == LayoutMode::Split {
            self.preview
                .update(cx, |preview, cx| preview.reveal_offset(cursor, cx));
        }
        cx.notify();
    }

    fn new_file(&mut self, _: &NewFile, window: &mut Window, cx: &mut Context<Self>) {
        self.confirm_discard(window, cx, |this, window, cx| {
            this.show_document(None, "", window, cx);
        });
    }

    fn open(&mut self, _: &Open, window: &mut Window, cx: &mut Context<Self>) {
        self.confirm_discard(window, cx, |this, window, cx| this.prompt_open(window, cx));
    }

    fn prompt_open(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Open".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else {
                return;
            };
            let Some(path) = paths.into_iter().next() else {
                return;
            };
            this.update_in(cx, |this, window, cx| this.open_path(path, window, cx))
                .ok();
        })
        .detach();
    }

    fn save(&mut self, _: &Save, window: &mut Window, cx: &mut Context<Self>) {
        match self.document.path() {
            Some(path) => self.save_to(path.to_path_buf(), window, cx),
            None => self.prompt_save_as(window, cx),
        }
    }

    fn save_as(&mut self, _: &SaveAs, window: &mut Window, cx: &mut Context<Self>) {
        self.prompt_save_as(window, cx);
    }

    fn prompt_save_as(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let directory = self
            .document
            .path()
            .and_then(Path::parent)
            .map(Path::to_path_buf)
            .or_else(home_dir)
            .unwrap_or_default();
        let suggested_name = match self.document.path() {
            Some(_) => self.document.display_name(),
            None => "Untitled.md".to_string(),
        };
        let path = cx.prompt_for_new_path(&directory, Some(&suggested_name));
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(path))) = path.await else {
                return;
            };
            this.update_in(cx, |this, window, cx| this.save_to(path, window, cx))
                .ok();
        })
        .detach();
    }

    /// Write the current text to `path` on a background thread.
    fn save_to(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let revision = self.document.edit_revision();
        let text = self.editor.read(cx).text().clone();
        let write = cx.background_spawn({
            let path = path.clone();
            async move { std::fs::write(&path, text.to_string()) }
        });
        self.save_task = Some(cx.spawn_in(window, async move |this, cx| {
            let result = write.await;
            this.update_in(cx, |this, window, cx| {
                this.save_task = None;
                match result {
                    Ok(()) => {
                        this.document.record_save(path, revision);
                        this.update_window_title(window);
                    }
                    Err(error) => {
                        let name = file_name(&path);
                        window.push_notification(
                            Notification::error(format!("Couldn’t save “{name}”. {error}.")),
                            cx,
                        );
                    }
                }
                cx.notify();
            })
            .ok();
        }));
    }

    fn show_editor(&mut self, _: &ShowEditor, window: &mut Window, cx: &mut Context<Self>) {
        self.set_layout(LayoutMode::Editor, window, cx);
    }

    fn show_split(&mut self, _: &ShowSplit, window: &mut Window, cx: &mut Context<Self>) {
        self.set_layout(LayoutMode::Split, window, cx);
    }

    fn show_preview(&mut self, _: &ShowPreview, window: &mut Window, cx: &mut Context<Self>) {
        self.set_layout(LayoutMode::Preview, window, cx);
    }

    pub fn set_layout(&mut self, layout: LayoutMode, window: &mut Window, cx: &mut Context<Self>) {
        if self.layout == layout {
            return;
        }
        self.layout = layout;
        self.scroll_metrics
            .update(cx, |metrics, cx| metrics.set_layout(layout, cx));
        if layout != LayoutMode::Editor {
            let cursor = self.editor.read(cx).cursor();
            self.preview
                .update(cx, |preview, cx| preview.reveal_offset(cursor, cx));
        }
        self.focus_content(window, cx);
        cx.notify();
    }

    fn toggle_theme(&mut self, _: &ToggleTheme, window: &mut Window, cx: &mut Context<Self>) {
        let mode = if cx.theme().is_dark() {
            ThemeMode::Light
        } else {
            ThemeMode::Dark
        };
        Theme::change(mode, Some(window), cx);
    }

    fn close_window(&mut self, _: &CloseWindow, window: &mut Window, cx: &mut Context<Self>) {
        self.confirm_discard(window, cx, |_, window, _| window.remove_window());
    }

    fn quit(&mut self, _: &Quit, window: &mut Window, cx: &mut Context<Self>) {
        self.confirm_discard(window, cx, |_, _, cx| cx.quit());
    }

    /// Ask before the platform closes a window with unsaved edits.
    fn install_close_guard(&self, window: &mut Window, cx: &mut Context<Self>) {
        let this = cx.weak_entity();
        window.on_window_should_close(cx, move |window, cx| {
            let Some(this) = this.upgrade() else {
                return true;
            };
            if !this.read(cx).document.is_dirty() {
                return true;
            }
            this.update(cx, |this, cx| {
                this.confirm_discard(window, cx, |_, window, _| window.remove_window());
            });
            false
        });
    }

    /// Run `then` now, or after the user agrees to drop unsaved edits.
    fn confirm_discard(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        then: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) {
        if !self.document.is_dirty() {
            then(self, window, cx);
            return;
        }
        let title: SharedString =
            format!("Discard changes to “{}”?", self.document.display_name()).into();
        let this = cx.weak_entity();
        let then = Rc::new(then);
        window.open_alert_dialog(cx, move |alert, _, _| {
            let this = this.clone();
            let then = then.clone();
            alert
                .title(title.clone())
                .description("Your unsaved edits will be lost.")
                .confirm()
                .ok_text("Discard changes")
                .ok_variant(ButtonVariant::Danger)
                .cancel_text("Cancel")
                .on_ok(move |_, window, cx| {
                    let then = then.clone();
                    this.update(cx, |this, cx| then(this, window, cx)).ok();
                    true
                })
        });
    }

    fn update_window_title(&self, window: &mut Window) {
        window.set_window_title(&format!("{} — Neon", self.document.display_name()));
        window.set_window_edited(self.document.is_dirty());
    }

    fn render_title_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let selected = LayoutMode::ALL
            .iter()
            .position(|mode| *mode == self.layout)
            .unwrap_or_default();
        let theme = cx.theme();

        TitleBar::new()
            .border_b_0()
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .text_sm()
                    .child(self.document.display_name())
                    .when(self.document.is_dirty(), |this| {
                        this.child(div().text_color(theme.muted_foreground).child("Edited"))
                    }),
            )
            .child(
                div().pr_2().child(
                    TabBar::new("layout")
                        .segmented()
                        .children(LayoutMode::ALL.map(|mode| Tab::new().label(mode.label())))
                        .selected_index(selected)
                        .on_click(cx.listener(|this, ix: &usize, window, cx| {
                            if let Some(mode) = LayoutMode::ALL.get(*ix) {
                                this.set_layout(*mode, window, cx);
                            }
                        })),
                ),
            )
    }

    fn render_content(&self, window: &Window, cx: &Context<Self>) -> AnyElement {
        // Below this a pane cannot show a readable line of either source or
        // prose; it follows the rem size so zoom keeps the proportion.
        let min_pane_width = window.rem_size() * 16.;
        let radius = cx.theme().radius_tokens().lg;
        let editor = || {
            div()
                .size_full()
                .min_w_0()
                .p(cx.theme().spacing_tokens().xs)
                .child(
                    Editor::new(&self.editor)
                        .bordered(true)
                        .rounded(radius)
                        .overflow_hidden()
                        .size_full(),
                )
        };
        let preview = || {
            div()
                .size_full()
                .min_w_0()
                .p(cx.theme().spacing_tokens().xs)
                .child(self.preview.clone())
        };

        match self.layout {
            LayoutMode::Editor => editor().into_any_element(),
            LayoutMode::Preview => preview().into_any_element(),
            LayoutMode::Split => h_resizable("split")
                .with_handle_appearance(Rc::new(|handle, window, cx| {
                    if handle.state() == ResizeHandleState::Idle {
                        Some(div().into_any_element())
                    } else {
                        resize_handle_appearance()(handle, window, cx)
                    }
                }))
                .child(
                    resizable_panel()
                        .size_range(min_pane_width..Pixels::MAX)
                        .child(editor()),
                )
                .child(
                    resizable_panel()
                        .size_range(min_pane_width..Pixels::MAX)
                        .child(preview()),
                )
                .into_any_element(),
        }
    }

    fn render_status_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let position = self.editor.read(cx).cursor_position();
        let stats = self.preview.read(cx).document().stats();
        let words = stats.words();

        StatusBar::new()
            .left(
                div()
                    .when(self.layout.shows_editor(), |this| {
                        this.child(format!(
                            "Ln {}, Col {}",
                            position.line + 1,
                            position.character + 1
                        ))
                    })
                    .when(self.is_loading(), |this| this.child("Opening…")),
            )
            .right(self.scroll_metrics.clone())
            .right(div().child(format!(
                "{} {}",
                format_count(words),
                if words == 1 { "word" } else { "words" }
            )))
    }
}

impl Focusable for Workspace {
    fn focus_handle(&self, _: &gpui_kit::App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        div()
            .id("workspace")
            .key_context(KEY_CONTEXT)
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::new_file))
            .on_action(cx.listener(Self::open))
            .on_action(cx.listener(Self::save))
            .on_action(cx.listener(Self::save_as))
            .on_action(cx.listener(Self::show_editor))
            .on_action(cx.listener(Self::show_split))
            .on_action(cx.listener(Self::show_preview))
            .on_action(cx.listener(Self::toggle_theme))
            .on_action(cx.listener(Self::close_window))
            .on_action(cx.listener(Self::quit))
            .flex()
            .flex_col()
            .size_full()
            .bg(theme.background)
            .text_color(theme.foreground)
            .child(self.render_title_bar(cx))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_row()
                    .items_stretch()
                    .child(self.render_content(window, cx)),
            )
            .child(self.render_status_bar(cx))
    }
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

/// `12345` as `12,345`.
fn format_count(count: usize) -> String {
    let digits = count.to_string();
    let mut formatted = String::with_capacity(digits.len() + digits.len() / 3);
    for (ix, digit) in digits.chars().enumerate() {
        if ix > 0 && (digits.len() - ix).is_multiple_of(3) {
            formatted.push(',');
        }
        formatted.push(digit);
    }
    formatted
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_counts_with_thousands_separators() {
        assert_eq!(format_count(0), "0");
        assert_eq!(format_count(999), "999");
        assert_eq!(format_count(1000), "1,000");
        assert_eq!(format_count(1234567), "1,234,567");
    }
}
