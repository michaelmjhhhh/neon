use std::cell::Cell;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::folder::{is_markdown, scan_folder};
use crate::typography::Typography;
use gpui_kit::base::ResizeHandleState;
use gpui_kit::component::button::{Button, ButtonVariant, ButtonVariants as _};
use gpui_kit::component::input::{EditorState, InputEvent};
use gpui_kit::component::list::ListItem;
use gpui_kit::component::notification::Notification;
use gpui_kit::component::resizable::{h_resizable, resizable_panel, resize_handle_appearance};
use gpui_kit::component::status_bar::StatusBar;
use gpui_kit::component::tab::{Tab, TabBar};
use gpui_kit::component::tree::{Tree, TreeEvent, TreeState};
use gpui_kit::component::{ActiveTheme as _, Theme, ThemeMode, TitleBar, WindowExt as _};
use gpui_kit::component::{Disableable as _, IconName, Sizable as _};
use gpui_kit::{
    AnyElement, AppContext as _, Context, Entity, FocusHandle, Focusable, InteractiveElement as _,
    IntoElement, ParentElement as _, PathPromptOptions, Pixels, Render, SharedString,
    StyleRefinement, Styled as _, Subscription, Task, Window, div, prelude::FluentBuilder as _,
};
use markdown_preview::MarkdownPreview;

use crate::KEY_CONTEXT;
use crate::document_file::DocumentFile;
use crate::editor_pane::EditorPane;
use crate::present_now::{self, NativeView};
use crate::scroll_metrics::ScrollMetrics;

gpui_kit::actions!(
    workspace,
    [
        NewFile,
        Open,
        OpenFolder,
        RefreshFolder,
        TypographySettings,
        ToggleSidebar,
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

/// At most one frame per 60 Hz refresh is presented ahead of the display
/// link; see [`Workspace::draw_soon`].
const MIN_EAGER_PRESENT_INTERVAL: Duration = Duration::from_micros(16_600);

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
    folder: Option<PathBuf>,
    tree: Entity<TreeState>,
    folder_task: Option<Task<()>>,
    folder_empty: bool,
    expanded_folders: HashSet<SharedString>,
    sidebar_visible: bool,
    typography: Entity<Typography>,
    sidebar: Entity<WorkspaceSidebar>,
    editor: Entity<EditorState>,
    editor_pane: Entity<EditorPane>,
    preview: Entity<MarkdownPreview>,
    scroll_metrics: Entity<ScrollMetrics>,
    layout: LayoutMode,
    focus_handle: FocusHandle,
    /// Cursor offset last revealed in the preview, to react to moves only.
    revealed_cursor: Option<usize>,
    load_task: Option<Task<()>>,
    save_task: Option<Task<()>>,
    /// An editor change waiting to be drawn ahead of the next display refresh.
    eager_draw: Option<Task<()>>,
    /// When a frame was last presented ahead of the display refresh.
    last_eager_present: Option<Instant>,
    _subscriptions: Vec<Subscription>,
}

impl Workspace {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let editor = cx.new(|cx| {
            let mut editor = EditorState::new(window, cx)
                .language("markdown")
                .placeholder("Start writing, or open a Markdown file…");
            editor.set_highlighter_factory(crate::highlighter::factory(), cx);
            editor
        });
        let preview = cx.new(MarkdownPreview::new);
        let scroll_metrics =
            cx.new(|cx| ScrollMetrics::new(editor.clone(), preview.clone(), window, cx));

        let tree = cx.new(|cx| TreeState::new(cx));
        let typography = cx.new(|cx| Typography::new(window, cx));
        let editor_pane = cx.new(|_| EditorPane::new(editor.clone(), typography.clone()));
        let sidebar = cx.new({
            let workspace = cx.weak_entity();
            |_| WorkspaceSidebar { workspace }
        });
        let subscriptions = vec![
            cx.subscribe(&tree, |this, _, event, _| match event {
                TreeEvent::Expanded(id) => {
                    this.expanded_folders.insert(id.clone());
                }
                TreeEvent::Collapsed(id) => {
                    this.expanded_folders.remove(id);
                }
            }),
            cx.observe_in(&tree, window, |this, tree, window, cx| {
                let path = tree
                    .read(cx)
                    .selected_entry()
                    .filter(|entry| !entry.is_folder())
                    .map(|entry| PathBuf::from(entry.item().id.as_ref()));
                if let Some(path) = path
                    && this.document.path() != Some(path.as_path())
                    && !this.is_busy()
                {
                    this.confirm_discard(window, cx, move |this, window, cx| {
                        this.open_path(path.clone(), window, cx);
                    });
                }
            }),
            cx.subscribe(&typography, |this, state, _, cx| {
                let state = state.read(cx);
                let font = state.preview_font(cx);
                let scale = state.preview_size(cx) / 16.;
                this.preview
                    .update(cx, |preview, cx| preview.set_typography(font, scale, cx));
                cx.notify();
            }),
            cx.subscribe_in(&editor, window, Self::on_editor_event),
            cx.observe_in(&editor, window, Self::on_editor_notified),
            cx.observe_window_appearance(window, |_, window, cx| {
                Theme::sync_system_appearance(Some(window), cx);
            }),
        ];

        let workspace = Self {
            document: DocumentFile::default(),
            folder: None,
            tree,
            folder_task: None,
            folder_empty: true,
            expanded_folders: HashSet::new(),
            sidebar_visible: true,
            typography,
            sidebar,
            editor,
            editor_pane,
            preview,
            scroll_metrics,
            layout: LayoutMode::default(),
            focus_handle: cx.focus_handle(),
            revealed_cursor: None,
            load_task: None,
            save_task: None,
            eager_draw: None,
            last_eager_present: None,
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
        self.folder.is_none()
            && self.document.path().is_none()
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
        if self.is_busy() {
            return;
        }
        if path.is_dir() {
            self.load_folder(path, window, cx);
            return;
        }
        let read = cx.background_spawn({
            let path = path.clone();
            async move {
                let path = std::fs::canonicalize(path)?;
                std::fs::read_to_string(&path).map(|text| (path, text))
            }
        });
        self.set_editor_readonly(true, cx);
        self.load_task = Some(cx.spawn_in(window, async move |this, cx| {
            let result = read.await;
            this.update_in(cx, |this, window, cx| {
                this.load_task = None;
                this.set_editor_readonly(false, cx);
                match result {
                    Ok((path, text)) => {
                        let previous_focus = window.focused(cx);
                        this.show_document(Some(path), &text, window, cx);
                        if let Some(focus) = previous_focus {
                            focus.focus(window, cx);
                        }
                    }
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

    fn set_editor_readonly(&self, readonly: bool, cx: &mut Context<Self>) {
        self.editor_pane
            .update(cx, |pane, cx| pane.set_readonly(readonly, cx));
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
        self.sync_tree_selection(cx);
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
        self.draw_soon(window, cx);
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
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let cursor = editor.read(cx).cursor();
        if self.revealed_cursor == Some(cursor) {
            return;
        }
        self.revealed_cursor = Some(cursor);
        self.draw_soon(window, cx);
        if self.layout == LayoutMode::Split {
            self.preview
                .update(cx, |preview, cx| preview.reveal_offset(cursor, cx));
        }
        // No notify: the status bar reads the cursor during this view's
        // render, so the editor's own notification already redraws it.
        // Notifying here would also invalidate the cached sidebar.
    }

    /// Draw and present an edit or cursor move as soon as the input event
    /// returns, instead of at the next display refresh.
    ///
    /// GPUI draws on display-link ticks: a keystroke waits for the next tick,
    /// is drawn, and reaches the screen a refresh after that tick. Presenting
    /// right away lets the frame make the next refresh instead; see
    /// [`present_now`](crate::present_now).
    ///
    /// Only input takes this path. A second synchronous present within one
    /// refresh blocks the main thread until that refresh, so animations and
    /// other notifications stay on the display link, and input arriving
    /// within a refresh of the last early present waits for the tick too. If
    /// a tick comes first, it draws the change and this is skipped.
    fn draw_soon(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.eager_draw.is_some()
            || self
                .last_eager_present
                .is_some_and(|at| at.elapsed() < MIN_EAGER_PRESENT_INTERVAL)
        {
            return;
        }
        let ticked = Rc::new(Cell::new(false));
        window.on_next_frame({
            let ticked = ticked.clone();
            move |_, _| ticked.set(true)
        });
        self.eager_draw = Some(cx.spawn_in(window, async move |this, cx| {
            this.update(cx, |this, _| this.eager_draw = None).ok();
            if ticked.get() {
                return;
            }
            // Fails once the window has closed, so the view is still alive.
            let Ok(view) = cx.update(|window, _| NativeView::of(window)) else {
                return;
            };
            let Some(view) = view else {
                cx.update(|window, cx| window.draw(cx).clear(cx)).ok();
                return;
            };
            this.update(cx, |this, _| this.last_eager_present = Some(Instant::now()))
                .ok();
            // Outside any update: GPUI's frame callback updates the app.
            present_now::present(view);
        }));
    }

    fn new_file(&mut self, _: &NewFile, window: &mut Window, cx: &mut Context<Self>) {
        self.confirm_discard(window, cx, |this, window, cx| {
            if this.folder.is_some() {
                this.prompt_create_file(window, cx);
            } else {
                this.show_document(None, "", window, cx);
            }
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
            .or_else(|| self.folder.clone())
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
        if self.is_busy() {
            return;
        }
        let revision = self.document.edit_revision();
        let text = self.editor.read(cx).text().clone();
        let write = cx.background_spawn({
            let path = path.clone();
            async move {
                std::fs::write(&path, text.to_string())?;
                std::fs::canonicalize(path)
            }
        });
        self.save_task = Some(cx.spawn_in(window, async move |this, cx| {
            let result = write.await;
            this.update_in(cx, |this, window, cx| {
                this.save_task = None;
                match result {
                    Ok(path) => {
                        this.document.record_save(path, revision);
                        this.update_window_title(window);
                        let base_dir = this.document.path().and_then(Path::parent).map(Arc::from);
                        this.preview
                            .update(cx, |preview, cx| preview.set_base_dir(base_dir, cx));
                        this.refresh_folder(&RefreshFolder, window, cx);
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
        cx.notify();
    }

    fn sync_tree_selection(&self, cx: &mut Context<Self>) {
        let id = self
            .document
            .path()
            .map(|path| SharedString::from(path.to_string_lossy().into_owned()));
        self.tree.update(cx, |tree, cx| {
            if let Some(id) = id {
                tree.reveal_item(&id, gpui_kit::ScrollStrategy::Center, cx);
                tree.set_selected_index(tree.index_of(&id), cx);
            } else {
                tree.set_selected_index(None, cx);
            }
        });
    }

    fn is_busy(&self) -> bool {
        self.load_task.is_some() || self.save_task.is_some()
    }

    fn open_folder(&mut self, _: &OpenFolder, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_busy() || self.folder_task.is_some() {
            return;
        }
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Open Folder".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else {
                return;
            };
            if let Some(path) = paths.into_iter().next() {
                this.update_in(cx, |this, window, cx| this.load_folder(path, window, cx))
                    .ok();
            }
        })
        .detach();
    }

    fn load_folder(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let scan = cx.background_spawn({
            let path = path.clone();
            async move {
                let path = std::fs::canonicalize(path)?;
                scan_folder(&path).map(|entries| (path, entries))
            }
        });
        self.folder_task = Some(cx.spawn_in(window, async move |this, cx| {
            let result = scan.await;
            this.update_in(cx, |this, window, cx| {
                this.folder_task = None;
                match result {
                    Ok((path, entries)) => {
                        this.folder_empty = entries.is_empty();
                        if this.folder.as_ref() != Some(&path) {
                            this.expanded_folders.clear();
                        }
                        this.folder = Some(path);
                        this.sidebar_visible = true;
                        let selected = this
                            .document
                            .path()
                            .map(|path| SharedString::from(path.to_string_lossy().into_owned()));
                        this.tree.update(cx, |tree, cx| {
                            tree.set_items(
                                entries
                                    .into_iter()
                                    .map(|entry| entry.into_tree_item(&this.expanded_folders))
                                    .collect::<Vec<_>>(),
                                cx,
                            );
                            if let Some(id) = selected {
                                tree.reveal_item(&id, gpui_kit::ScrollStrategy::Center, cx);
                                tree.set_selected_index(tree.index_of(&id), cx);
                            }
                        });
                    }
                    Err(error) => window.push_notification(
                        Notification::error(format!(
                            "Couldn’t read folder “{}”. {error}",
                            path.display()
                        )),
                        cx,
                    ),
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    fn refresh_folder(&mut self, _: &RefreshFolder, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(path) = self.folder.clone() {
            self.load_folder(path, window, cx);
        }
    }

    fn prompt_create_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(folder) = self.folder.clone() else {
            return;
        };
        let directory = self
            .tree
            .read(cx)
            .selected_entry()
            .map(|entry| {
                let path = PathBuf::from(entry.item().id.as_ref());
                if entry.is_folder() {
                    path
                } else {
                    path.parent().unwrap_or(&folder).to_path_buf()
                }
            })
            .unwrap_or(folder);
        let path = cx.prompt_for_new_path(&directory, Some("Untitled.md"));
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(mut path))) = path.await else {
                return;
            };
            if !is_markdown(&path) {
                let mut name = path.as_os_str().to_os_string();
                name.push(".md");
                path = name.into();
            }
            this.update_in(cx, |this, window, cx| {
                if this.is_busy() {
                    return;
                }
                let create = cx.background_spawn({
                    let path = path.clone();
                    async move {
                        std::fs::OpenOptions::new()
                            .write(true)
                            .create_new(true)
                            .open(&path)?;
                        std::fs::canonicalize(path)
                    }
                });
                this.set_editor_readonly(true, cx);
                this.load_task = Some(cx.spawn_in(window, async move |this, cx| {
                    let result = create.await;
                    this.update_in(cx, |this, window, cx| {
                        this.load_task = None;
                        this.set_editor_readonly(false, cx);
                        match result {
                            Ok(path) => {
                                this.show_document(Some(path), "", window, cx);
                                this.refresh_folder(&RefreshFolder, window, cx);
                            }
                            Err(error) => window.push_notification(
                                Notification::error(format!(
                                    "Couldn’t create “{}”. {error}",
                                    path.display()
                                )),
                                cx,
                            ),
                        }
                        cx.notify();
                    })
                    .ok();
                }));
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn toggle_sidebar(&mut self, _: &ToggleSidebar, window: &mut Window, cx: &mut Context<Self>) {
        self.sidebar_visible = !self.sidebar_visible;
        if !self.sidebar_visible {
            self.focus_content(window, cx);
        }
        cx.notify();
    }

    fn typography_settings(
        &mut self,
        _: &TypographySettings,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let typography = self.typography.clone();
        window.open_dialog(cx, move |dialog, window, _| {
            dialog
                .width(window.rem_size() * 28.)
                .title("Typography")
                .child(typography.clone())
                .footer(
                    div().flex().justify_end().child(
                        Button::new("done")
                            .label("Done")
                            .small()
                            .on_click(|_, window, cx| window.close_dialog(cx)),
                    ),
                )
        });
    }

    fn render_sidebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let active = self
            .document
            .path()
            .map(|path| path.to_string_lossy().into_owned());
        div()
            .flex()
            .flex_col()
            .size_full()
            .min_h_0()
            .bg(cx.theme().sidebar)
            .border_r_1()
            .border_color(cx.theme().border)
            .text_color(cx.theme().sidebar_foreground)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_3()
                    .h_12()
                    .child(
                        div().flex_1().truncate().text_sm().child(
                            self.folder
                                .as_deref()
                                .map(file_name)
                                .unwrap_or_else(|| "Files".into()),
                        ),
                    )
                    .child(
                        Button::new("new-file")
                            .small()
                            .ghost()
                            .icon(IconName::Plus)
                            .accessibility_label("New file…")
                            .tooltip_with_action("New file…", &NewFile, Some(KEY_CONTEXT))
                            .disabled(self.is_busy())
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.new_file(&NewFile, window, cx)
                            })),
                    )
                    .child(
                        Button::new("refresh-folder")
                            .small()
                            .ghost()
                            .icon(IconName::RefreshCw)
                            .accessibility_label("Refresh folder")
                            .disabled(self.folder.is_none() || self.folder_task.is_some())
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.refresh_folder(&RefreshFolder, window, cx)
                            })),
                    ),
            )
            .when(self.folder_task.is_some(), |view| {
                view.child(div().px_3().text_sm().child("Reading folder…"))
            })
            .when(self.folder.is_none(), |view| {
                view.child(
                    div()
                        .px_4()
                        .py_6()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child("Your Markdown files, in one place. Open a folder to get started."),
                )
            })
            .when(
                self.folder.is_some() && self.folder_empty && self.folder_task.is_none(),
                |view| {
                    view.child(
                        div()
                            .p_3()
                            .text_sm()
                            .child("No Markdown files. Choose New File to create one."),
                    )
                },
            )
            .child(div().flex_1().min_h_0().child(Tree::new(
                &self.tree,
                move |_, entry, _, window, _cx| {
                    let item = entry.item();
                    let icon = if entry.is_folder() {
                        if entry.is_expanded() {
                            IconName::FolderOpen
                        } else {
                            IconName::Folder
                        }
                    } else {
                        IconName::FileText
                    };
                    ListItem::new(item.id.clone())
                        .text_sm()
                        .accessibility_label(item.label.clone())
                        .pl(window.rem_size() * (0.75 + entry.depth() as f32))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .child(gpui_kit::component::Icon::new(icon).small())
                                .child(div().truncate().child(item.label.clone()))
                                .when(active.as_deref() == Some(item.id.as_ref()), |row| {
                                    row.font_weight(gpui_kit::FontWeight::SEMIBOLD)
                                }),
                        )
                },
            )))
            .child(
                div()
                    .p_3()
                    .border_t_1()
                    .border_color(cx.theme().border)
                    .child(
                        Button::new("open-folder")
                            .small()
                            .ghost()
                            .icon(IconName::FolderOpen)
                            .label("Open Folder…")
                            .w_full()
                            .disabled(self.folder_task.is_some() || self.is_busy())
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.open_folder(&OpenFolder, window, cx)
                            })),
                    ),
            )
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
            if this.read(cx).is_busy() {
                return false;
            }
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
        if self.is_busy() {
            return;
        }
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
            let cancel_view = this.clone();
            alert
                .title(title.clone())
                .description("Your unsaved edits will be lost.")
                .confirm()
                .ok_text("Discard changes")
                .ok_variant(ButtonVariant::Danger)
                .cancel_text("Cancel")
                .on_cancel(move |_, _, cx| {
                    cancel_view
                        .update(cx, |this, cx| this.sync_tree_selection(cx))
                        .ok();
                    true
                })
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
        TitleBar::new()
            .border_b_1()
            .border_color(cx.theme().border)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .min_w_0()
                    .child(
                        Button::new("toggle-sidebar")
                            .small()
                            .ghost()
                            .icon(if self.sidebar_visible {
                                IconName::PanelLeftClose
                            } else {
                                IconName::PanelLeftOpen
                            })
                            .accessibility_label("Toggle sidebar")
                            .tooltip_with_action(
                                "Toggle sidebar",
                                &ToggleSidebar,
                                Some(KEY_CONTEXT),
                            )
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.toggle_sidebar(&ToggleSidebar, window, cx)
                            })),
                    )
                    .child(
                        div()
                            .text_sm()
                            .truncate()
                            .child(self.document.display_name()),
                    )
                    .when(self.document.is_dirty(), |view| {
                        view.child(
                            div()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child("Edited"),
                        )
                    }),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .pr_2()
                    .child(
                        TabBar::new("layout")
                            .segmented()
                            .small()
                            .children(LayoutMode::ALL.map(|mode| Tab::new().label(mode.label())))
                            .selected_index(selected)
                            .on_click(cx.listener(|this, ix: &usize, window, cx| {
                                if let Some(mode) = LayoutMode::ALL.get(*ix) {
                                    this.set_layout(*mode, window, cx);
                                }
                            })),
                    )
                    .child(div().h_4().border_l_1().border_color(cx.theme().border))
                    .child(
                        Button::new("typography")
                            .small()
                            .ghost()
                            .label("Aa")
                            .accessibility_label("Typography…")
                            .tooltip_with_action(
                                "Typography…",
                                &TypographySettings,
                                Some(KEY_CONTEXT),
                            )
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.typography_settings(&TypographySettings, window, cx)
                            })),
                    )
                    .child(
                        Button::new("save-file")
                            .small()
                            .ghost()
                            .label("Save")
                            .accessibility_label("Save file")
                            .tooltip_with_action("Save", &Save, Some(KEY_CONTEXT))
                            .disabled(self.is_busy())
                            .on_click(
                                cx.listener(|this, _, window, cx| this.save(&Save, window, cx)),
                            ),
                    ),
            )
    }

    fn render_content(&self, window: &Window, cx: &Context<Self>) -> AnyElement {
        // Below this a pane cannot show a readable line of either source or
        // prose; it follows the rem size so zoom keeps the proportion.
        let min_pane_width = window.rem_size() * 16.;
        // Cached views keep their last frame until notified, so typing does
        // not re-render the preview, and preview updates skip the editor.
        let pane_style = || StyleRefinement::default().size_full();
        let editor = || {
            div()
                .size_full()
                .min_w_0()
                .flex()
                .flex_col()
                .p_2()
                .child(
                    div()
                        .h_8()
                        .flex()
                        .items_center()
                        .px_3()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child("Markdown"),
                )
                .child(
                    div()
                        .flex_1()
                        .min_h_0()
                        .child(self.editor_pane.clone().cached(pane_style())),
                )
        };
        let preview = || {
            div()
                .size_full()
                .min_w_0()
                .flex()
                .flex_col()
                .p_2()
                .child(
                    div()
                        .h_8()
                        .flex()
                        .items_center()
                        .px_3()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child("Preview"),
                )
                .child(
                    div()
                        .flex_1()
                        .min_h_0()
                        .child(MarkdownPreview::pane(&self.preview, cx)),
                )
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
                    .when(self.is_loading(), |this| this.child("Opening…"))
                    .when(self.save_task.is_some(), |this| this.child("Saving…")),
            )
            .right(self.scroll_metrics.clone())
            .right(div().child(format!(
                "{} {}",
                format_count(words),
                if words == 1 { "word" } else { "words" }
            )))
    }
}

/// The sidebar as a cached view: it re-renders only when the workspace or
/// the file tree notifies, not on every keystroke's frame.
struct WorkspaceSidebar {
    workspace: gpui_kit::WeakEntity<Workspace>,
}

impl Render for WorkspaceSidebar {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.workspace
            .update(cx, |workspace, cx| {
                workspace.render_sidebar(cx).into_any_element()
            })
            .unwrap_or_else(|_| div().into_any_element())
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
            .on_action(cx.listener(Self::open_folder))
            .on_action(cx.listener(Self::refresh_folder))
            .on_action(cx.listener(Self::typography_settings))
            .on_action(cx.listener(Self::toggle_sidebar))
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
                    .child(
                        h_resizable("workspace-panes")
                            .when(self.sidebar_visible, |panes| {
                                panes.child(
                                    resizable_panel()
                                        .size(window.rem_size() * 14.)
                                        .size_range(
                                            window.rem_size() * 12. ..window.rem_size() * 20.,
                                        )
                                        .child(
                                            self.sidebar
                                                .clone()
                                                .cached(StyleRefinement::default().size_full()),
                                        ),
                                )
                            })
                            .child(
                                resizable_panel()
                                    .size_range(window.rem_size() * 20. ..Pixels::MAX)
                                    .child(self.render_content(window, cx)),
                            ),
                    ),
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
