use std::cell::Cell;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::folder::{is_markdown, scan_folder};
use crate::typography::Typography;
use export::ExportFormat;
use gpui_kit::base::{ResizeHandleRenderer, ResizeHandleState};
use gpui_kit::component::button::{Button, ButtonVariant, ButtonVariants as _};
use gpui_kit::component::empty::{
    Empty, EmptyDescription, EmptyHeader, EmptyMedia, EmptyMediaVariant, EmptyTitle,
};
use gpui_kit::component::input::{EditorState, Enter, Escape, IndentInline, InputEvent};
use gpui_kit::component::list::ListItem;
use gpui_kit::component::menu::DropdownMenu as _;
use gpui_kit::component::notification::Notification;
use gpui_kit::component::resizable::{h_resizable, resizable_panel, resize_handle_appearance};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::status_bar::StatusBar;
use gpui_kit::component::tab::{Tab, TabBar};
use gpui_kit::component::tree::{Tree, TreeEvent, TreeState};
use gpui_kit::component::{ActiveTheme as _, TitleBar, WindowExt as _};
use gpui_kit::component::{Disableable as _, IconName, Sizable as _};
use gpui_kit::{
    Anchor, AnyElement, AppContext as _, Context, Entity, FocusHandle, Focusable, FontWeight,
    InteractiveElement as _, IntoElement, ParentElement as _, PathPromptOptions, Pixels, Render,
    SharedString, StyleRefinement, Styled as _, Subscription, Task, Window, div,
    prelude::FluentBuilder as _,
};
use markdown_preview::MarkdownPreview;

use crate::KEY_CONTEXT;
use crate::document_file::DocumentFile;
use crate::editor_pane::EditorPane;
use crate::list_continuation;
use crate::present_now::{self, NativeView};
use crate::scroll_metrics::ScrollMetrics;
use crate::settings::SettingsView;
use crate::slash_commands::{Insertion, SlashCommands};
use crate::theme::{self, Appearance};
use crate::word_count::WordCount;

#[cfg(feature = "memory-bench")]
#[path = "memory_bench.rs"]
mod memory_bench;
#[cfg(feature = "memory-bench")]
pub use memory_bench::MemoryBench;

gpui_kit::actions!(
    workspace,
    [
        NewFile,
        Open,
        OpenFolder,
        RefreshFolder,
        OpenSettings,
        CloseSettings,
        ToggleSidebar,
        Save,
        SaveAs,
        ExportHtml,
        ExportPdf,
        ExportDocx,
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

/// The chrome margin around the document sheet, in rems.
pub(crate) const SHEET_INSET_REMS: f32 = 0.5;
/// The sheet header's height, in rems.
pub(crate) const SHEET_HEADER_REMS: f32 = 2.5;
/// The sidebar's default width, in rems.
pub(crate) const SIDEBAR_WIDTH_REMS: f32 = 15.;
/// Where content may start beside the macOS window controls, measured from
/// the window's leading edge: three 16 px controls from 20 px, their gaps,
/// and a small clearance. Platform geometry: it does not follow the rem size.
const WINDOW_CONTROLS_END: Pixels = gpui_kit::px(88.);

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
    /// Settings cover the window's content while open.
    settings_open: bool,
    settings: Entity<SettingsView>,
    sidebar: Entity<WorkspaceSidebar>,
    editor: Entity<EditorState>,
    editor_pane: Entity<EditorPane>,
    typography: Entity<Typography>,
    preview: Entity<MarkdownPreview>,
    scroll_metrics: Entity<ScrollMetrics>,
    word_count: Entity<WordCount>,
    /// The `/` template menu, kept to place the cursor in what it inserts.
    slash_commands: Rc<SlashCommands>,
    layout: LayoutMode,
    focus_handle: FocusHandle,
    /// Cursor offset last revealed in the preview, to react to moves only.
    revealed_cursor: Option<usize>,
    load_task: Option<Task<()>>,
    save_task: Option<Task<()>>,
    export_task: Option<Task<()>>,
    /// An editor change waiting to be drawn ahead of the next display refresh.
    eager_draw: Option<Task<()>>,
    /// When a frame was last presented ahead of the display refresh.
    last_eager_present: Option<Instant>,
    _subscriptions: Vec<Subscription>,
}

impl Workspace {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let slash_commands = Rc::new(SlashCommands::default());
        let editor = cx.new(|cx| {
            let mut editor = EditorState::new(window, cx)
                .language("markdown")
                .placeholder("Start writing, or open a Markdown file…");
            editor.set_highlighter_factory(crate::highlighter::factory(), cx);
            editor.lsp_mut().completion_provider = Some(slash_commands.clone());
            editor
        });
        let preview = cx.new(MarkdownPreview::new);
        let scroll_metrics =
            cx.new(|cx| ScrollMetrics::new(editor.clone(), preview.clone(), window, cx));
        let word_count = cx.new(|_| WordCount::default());

        let tree = cx.new(|cx| TreeState::new(cx));
        let typography = cx.new(|cx| Typography::new(window, cx));
        let editor_pane = cx.new(|_| EditorPane::new(editor.clone(), typography.clone()));
        let settings = cx.new(|cx| SettingsView::new(typography.clone(), cx));
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
                theme::apply_appearance(window, cx);
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
            settings_open: false,
            settings,
            sidebar,
            editor,
            editor_pane,
            typography,
            preview,
            scroll_metrics,
            word_count,
            slash_commands,
            layout: LayoutMode::default(),
            focus_handle: cx.focus_handle(),
            revealed_cursor: None,
            load_task: None,
            save_task: None,
            export_task: None,
            eager_draw: None,
            last_eager_present: None,
            _subscriptions: subscriptions,
        };
        theme::apply_appearance(window, cx);
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

    /// Focus the pane that takes keyboard input in the current layout, or
    /// Settings while they cover the window.
    pub fn focus_content(&self, window: &mut Window, cx: &mut Context<Self>) {
        if self.settings_open {
            self.settings.read(cx).focus_handle(cx).focus(window, cx);
        } else if self.layout.shows_editor() {
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
            self.load_folder(path, true, window, cx);
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
        self.word_count
            .update(cx, |count, cx| count.set_source(source.clone(), cx));
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
        let cursor = editor.read(cx).cursor();
        match self.slash_commands.take_insertion(&source, cursor) {
            Some(Insertion::Select(selection)) => {
                editor.update(cx, |editor, cx| editor.set_selected_range(selection, cx));
            }
            Some(Insertion::Complete { range, command }) => {
                editor.update(cx, |editor, cx| {
                    editor.set_selected_range(range, cx);
                    editor.replace(command, window, cx);
                    editor.show_completions(window, cx);
                });
            }
            None if self.slash_commands.needs_refresh(cursor) => {
                // A deletion closes the menu without asking for a new one.
                editor.update(cx, |editor, cx| editor.show_completions(window, cx));
            }
            None => {}
        }
        self.word_count
            .update(cx, |count, cx| count.set_source(source.clone(), cx));
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

    /// Where save panels open: beside the document, else in the open
    /// folder, else at home.
    fn default_directory(&self) -> PathBuf {
        self.document
            .path()
            .and_then(Path::parent)
            .map(Path::to_path_buf)
            .or_else(|| self.folder.clone())
            .or_else(home_dir)
            .unwrap_or_default()
    }

    fn prompt_save_as(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let directory = self.default_directory();
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

    fn prompt_export(&mut self, format: ExportFormat, window: &mut Window, cx: &mut Context<Self>) {
        if self.export_task.is_some() || self.is_loading() {
            return;
        }
        let directory = self.default_directory();
        let stem = self
            .document
            .path()
            .and_then(Path::file_stem)
            .map(|stem| stem.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Untitled".to_string());
        let suggested_name = format!("{stem}.{}", format.extension());
        let path = cx.prompt_for_new_path(&directory, Some(&suggested_name));
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(path))) = path.await else {
                return;
            };
            this.update_in(cx, |this, window, cx| {
                this.export_to(format, path, window, cx)
            })
            .ok();
        })
        .detach();
    }

    /// Export the text as it is now, set in the preview's typography, on a
    /// background thread.
    fn export_to(
        &mut self,
        format: ExportFormat,
        path: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.export_task.is_some() {
            return;
        }
        let markdown = self.editor.read(cx).text().to_string();
        let title = self
            .document
            .path()
            .and_then(Path::file_stem)
            .map(|stem| stem.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Untitled".to_string());
        let base_dir = self
            .document
            .path()
            .and_then(Path::parent)
            .map(Path::to_path_buf);
        let mut style = self.typography.read(cx).export_style(cx);
        #[cfg(target_os = "macos")]
        if let Some(paper) = export::pdf::default_paper() {
            style.paper = paper;
        }
        let run = cx.background_spawn({
            let path = path.clone();
            async move {
                let source = export::ExportSource {
                    markdown: &markdown,
                    title: &title,
                    base_dir: base_dir.as_deref(),
                };
                export::export(format, &source, &style, &path)
            }
        });
        self.export_task = Some(cx.spawn_in(window, async move |this, cx| {
            let result = run.await;
            this.update_in(cx, |this, window, cx| {
                this.export_task = None;
                let name = file_name(&path);
                let notification = match result {
                    Ok(()) => Notification::success(format!("Exported “{name}”.")),
                    Err(error) => Notification::error(format!(
                        "Couldn’t export “{name}”. {}",
                        sentence(&format!("{error:#}"))
                    )),
                };
                window.push_notification(notification, cx);
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
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
                this.update_in(cx, |this, window, cx| {
                    this.load_folder(path, true, window, cx)
                })
                .ok();
            }
        })
        .detach();
    }

    /// Scan `path` into the sidebar tree. `reveal` shows a hidden sidebar:
    /// opening a folder should, but a refresh after a save should not.
    fn load_folder(
        &mut self,
        path: PathBuf,
        reveal: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
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
                        this.sidebar_visible |= reveal;
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
            self.load_folder(path, false, window, cx);
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

    /// Start the next list item when Enter ends one in the editor, in place of
    /// the editor's plain newline.
    fn continue_list(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let editor = self.editor.read(cx);
        if self.is_loading()
            || !editor.focus_handle(cx).is_focused(window)
            || editor.completion_menu_state().open
        {
            return;
        }
        let selection = editor.selected_range();
        if !selection.is_empty() {
            return;
        }
        let Some((range, text)) = list_continuation::on_enter(editor.text(), selection.start)
        else {
            return;
        };
        cx.stop_propagation();
        self.editor.update(cx, |editor, cx| {
            editor.set_selected_range(range, cx);
            editor.replace(text, window, cx);
        });
    }

    fn toggle_sidebar(&mut self, _: &ToggleSidebar, window: &mut Window, cx: &mut Context<Self>) {
        self.sidebar_visible = !self.sidebar_visible;
        if !self.sidebar_visible {
            self.focus_content(window, cx);
        }
        cx.notify();
    }

    fn open_settings(&mut self, _: &OpenSettings, window: &mut Window, cx: &mut Context<Self>) {
        self.set_settings_open(true, window, cx);
    }

    fn close_settings(&mut self, _: &CloseSettings, window: &mut Window, cx: &mut Context<Self>) {
        self.set_settings_open(false, window, cx);
    }

    fn set_settings_open(&mut self, open: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.settings_open == open {
            return;
        }
        self.settings_open = open;
        self.settings
            .update(cx, |settings, cx| settings.set_open(open, window, cx));
        self.preview.update(cx, |preview, cx| {
            preview.set_active(!open && self.layout != LayoutMode::Editor, cx);
        });
        // Move focus now: the pane that held it is not rendered next frame,
        // and keys sent to it would not reach the workspace.
        self.focus_content(window, cx);
        cx.notify();
    }

    /// The folder sidebar. It runs the full height of the window on the
    /// chrome surface, with the window controls in its top row, so the
    /// document sheet beside it is the only raised region.
    fn render_sidebar(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let active = self
            .document
            .path()
            .map(|path| path.to_string_lossy().into_owned());
        let reading = self.folder_task.is_some();
        let can_open_folder = !reading && !self.is_busy();

        // The window controls' row. It matches the sheet's margin, header
        // and margin, so the controls and this toggle share the header's
        // center line.
        let controls_row = div().id("sidebar-title-bar").child(
            TitleBar::new()
                .h(window.rem_size() * (2. * SHEET_INSET_REMS + SHEET_HEADER_REMS))
                .bg(theme.sidebar)
                .border_color(gpui_kit::transparent_black())
                .child(
                    div()
                        .flex()
                        .flex_1()
                        .justify_end()
                        .pr_2()
                        .child(sidebar_toggle(true, cx)),
                ),
        );

        let header = self.folder.as_deref().map(|folder| {
            div()
                .flex()
                .flex_none()
                .items_center()
                .gap_1()
                .h_8()
                .mx_2()
                .pl_1()
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_sm()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(file_name(folder)),
                )
                .when(reading, |this| {
                    this.child(Spinner::new().xsmall().color(theme.muted_foreground))
                })
                .child(
                    Button::new("new-file")
                        .small()
                        .ghost()
                        .icon(IconName::Plus)
                        .accessibility_label("New file…")
                        .tooltip_with_action("New file…", &NewFile, Some(KEY_CONTEXT))
                        .disabled(self.is_busy())
                        .on_click(
                            cx.listener(|this, _, window, cx| this.new_file(&NewFile, window, cx)),
                        ),
                )
                .child(
                    Button::new("folder-actions")
                        .small()
                        .ghost()
                        .icon(IconName::Ellipsis)
                        .accessibility_label("Folder actions")
                        .dropdown_menu_with_anchor(Anchor::TopRight, move |menu, _, _| {
                            menu.menu_with_disabled(
                                "Open Folder…",
                                Box::new(OpenFolder),
                                !can_open_folder,
                            )
                            .menu_with_disabled(
                                "Refresh Folder",
                                Box::new(RefreshFolder),
                                reading,
                            )
                        }),
                )
        });

        let body = if self.folder.is_none() && reading {
            Empty::new()
                .flex_none()
                .justify_start()
                .px_4()
                .pt_10()
                .header(
                    EmptyHeader::new()
                        .media(EmptyMedia::new().child(Spinner::new().small()))
                        .title(EmptyTitle::new().child("Opening folder…")),
                )
                .into_any_element()
        } else if self.folder.is_none() {
            sidebar_empty_state(
                IconName::FolderOpen,
                "No folder open",
                "Open a folder to browse its Markdown files here.",
            )
            .child(
                Button::new("open-folder-empty")
                    .small()
                    .outline()
                    .icon(IconName::FolderOpen)
                    .label("Open Folder…")
                    .tooltip_with_action("Open Folder…", &OpenFolder, Some(KEY_CONTEXT))
                    .disabled(!can_open_folder)
                    .on_click(
                        cx.listener(|this, _, window, cx| {
                            this.open_folder(&OpenFolder, window, cx)
                        }),
                    ),
            )
            .into_any_element()
        } else if self.folder_empty && !reading {
            sidebar_empty_state(
                IconName::FileText,
                "No Markdown files",
                "Files ending in .md in this folder appear here.",
            )
            .child(
                Button::new("new-file-empty")
                    .small()
                    .outline()
                    .icon(IconName::Plus)
                    .label("New File…")
                    .disabled(self.is_busy())
                    .on_click(
                        cx.listener(|this, _, window, cx| this.new_file(&NewFile, window, cx)),
                    ),
            )
            .into_any_element()
        } else {
            let folder_icon = theme.muted_foreground;
            let active_icon = theme.primary;
            let row_radius = theme.radius;
            div()
                .flex_1()
                .min_h_0()
                .child(Tree::new(&self.tree, move |_, entry, _, window, _| {
                    let item = entry.item();
                    let is_active = active.as_deref() == Some(item.id.as_ref());
                    let (chevron, icon) = if entry.is_folder() {
                        if entry.is_expanded() {
                            (Some(IconName::ChevronDown), IconName::FolderOpen)
                        } else {
                            (Some(IconName::ChevronRight), IconName::Folder)
                        }
                    } else {
                        (None, IconName::FileText)
                    };
                    let rem = window.rem_size();
                    ListItem::new(item.id.clone())
                        .h_7()
                        .mx_2()
                        .px_0()
                        .py_0()
                        // Each level indents by one icon slot, so a child's
                        // chevron sits under its parent's folder icon.
                        .pl(rem * (0.25 + entry.depth() as f32 * 1.25))
                        .pr_2()
                        .rounded(row_radius)
                        .text_sm()
                        .accessibility_label(item.label.clone())
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_1()
                                .min_w_0()
                                // Fixed slots keep labels on one spine whether
                                // or not a row has a disclosure chevron.
                                .child(
                                    div()
                                        .flex_none()
                                        .size_4()
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .text_color(folder_icon)
                                        .children(chevron.map(|chevron| {
                                            gpui_kit::component::Icon::new(chevron).xsmall()
                                        })),
                                )
                                .child(
                                    gpui_kit::component::Icon::new(icon)
                                        .small()
                                        .flex_none()
                                        .text_color(if is_active {
                                            active_icon
                                        } else {
                                            folder_icon
                                        }),
                                )
                                .child(
                                    div()
                                        .ml_1()
                                        .min_w_0()
                                        .truncate()
                                        .when(is_active, |row| row.font_weight(FontWeight::MEDIUM))
                                        .child(item.label.clone()),
                                ),
                        )
                }))
                .into_any_element()
        };

        div()
            .flex()
            .flex_col()
            .size_full()
            .min_h_0()
            .bg(theme.sidebar)
            .text_color(theme.sidebar_foreground)
            .child(controls_row)
            .children(header)
            .child(body)
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
        self.preview.update(cx, |preview, cx| {
            preview.set_active(!self.settings_open && layout != LayoutMode::Editor, cx);
        });
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
        let appearance = if cx.theme().is_dark() {
            Appearance::Light
        } else {
            Appearance::Dark
        };
        theme::set_appearance(appearance, window, cx);
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

    /// The sheet's header doubles as the window's title bar: it names the
    /// document, shows whether it has unsaved edits next to the command that
    /// saves them, and holds the view controls.
    fn render_sheet_header(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let selected = LayoutMode::ALL
            .iter()
            .position(|mode| *mode == self.layout)
            .unwrap_or_default();
        let is_dirty = self.document.is_dirty();
        // Without the sidebar, the window controls sit on this header, so its
        // content starts after them. They do not follow the rem size.
        let clears_window_controls = !self.sidebar_visible && !window.is_fullscreen();
        let save_label = if self.document.path().is_some() {
            "Save"
        } else {
            "Save…"
        };

        let title = div()
            .flex()
            .items_center()
            .gap_1p5()
            .min_w_0()
            .text_sm()
            .when_some(
                self.folder.as_deref().filter(|_| !self.sidebar_visible),
                |this, folder| {
                    this.child(
                        div()
                            .flex_shrink_1()
                            .min_w_0()
                            .truncate()
                            .text_color(theme.muted_foreground)
                            .child(file_name(folder)),
                    )
                    .child(
                        div()
                            .flex_none()
                            .text_color(theme.muted_foreground)
                            .child("/"),
                    )
                },
            )
            .child(
                div()
                    .flex_shrink_0()
                    .max_w_80()
                    .truncate()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(self.document.display_name()),
            );

        TitleBar::new()
            .h(window.rem_size() * SHEET_HEADER_REMS)
            .map(|this| {
                if clears_window_controls {
                    this.pl(WINDOW_CONTROLS_END - window.rem_size() * SHEET_INSET_REMS)
                } else {
                    this.pl_4()
                }
            })
            .pr_2()
            // The sheet paints the surface, so its rounded corners stay clean.
            .bg(gpui_kit::transparent_black())
            .border_color(theme.border)
            .child(
                div()
                    .flex()
                    .flex_1()
                    .items_center()
                    .gap_2()
                    .min_w_0()
                    .when(!self.sidebar_visible, |this| {
                        this.child(sidebar_toggle(false, cx))
                    })
                    .child(title)
                    .when(is_dirty, |this| {
                        // A dot and a word: the state does not rely on color.
                        this.child(
                            div()
                                .flex()
                                .flex_none()
                                .items_center()
                                .gap_1p5()
                                .text_xs()
                                .text_color(theme.muted_foreground)
                                .child(div().size_1p5().rounded_full().bg(theme.primary))
                                .child("Edited"),
                        )
                        .child(
                            Button::new("save-file")
                                .xsmall()
                                .outline()
                                .label(save_label)
                                .tooltip_with_action(save_label, &Save, Some(KEY_CONTEXT))
                                .loading(self.save_task.is_some())
                                .disabled(self.is_busy())
                                .on_click(
                                    cx.listener(|this, _, window, cx| this.save(&Save, window, cx)),
                                ),
                        )
                    }),
            )
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap_2()
                    .child(
                        Button::new("settings")
                            .small()
                            .ghost()
                            .icon(IconName::Settings)
                            .accessibility_label("Settings…")
                            .tooltip_with_action("Settings…", &OpenSettings, Some(KEY_CONTEXT))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.open_settings(&OpenSettings, window, cx)
                            })),
                    )
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
                    ),
            )
    }

    /// The folder sidebar beside the document sheet.
    fn render_panes(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        h_resizable("workspace-panes")
            .with_handle_appearance(quiet_resize_handle())
            .when(self.sidebar_visible, |panes| {
                panes.child(
                    resizable_panel()
                        .size(window.rem_size() * SIDEBAR_WIDTH_REMS)
                        .size_range(window.rem_size() * 12. ..window.rem_size() * 22.)
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
                    .child(self.render_sheet(window, cx)),
            )
    }

    /// The document as one sheet: its header, the editor and preview, and
    /// a footer with facts about the text. Split mode divides the panes with
    /// a single hairline that the preview owns.
    fn render_sheet(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
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
                .child(self.editor_pane.clone().cached(pane_style()))
        };
        let border = cx.theme().border;
        let preview = |divided: bool| {
            div()
                .size_full()
                .min_w_0()
                .when(divided, |this| this.border_l_1().border_color(border))
                .child(MarkdownPreview::pane(&self.preview, cx))
        };

        let panes = match self.layout {
            LayoutMode::Editor => editor().into_any_element(),
            LayoutMode::Preview => preview(false).into_any_element(),
            LayoutMode::Split => h_resizable("split")
                .with_handle_appearance(quiet_resize_handle())
                .child(
                    resizable_panel()
                        .size_range(min_pane_width..Pixels::MAX)
                        .child(editor()),
                )
                .child(
                    resizable_panel()
                        .size_range(min_pane_width..Pixels::MAX)
                        .child(preview(true)),
                )
                .into_any_element(),
        };

        let header = self.render_sheet_header(window, cx).into_any_element();
        let footer = self.render_sheet_footer(cx).into_any_element();
        let theme = cx.theme();
        div()
            .size_full()
            .min_w_0()
            .p(window.rem_size() * SHEET_INSET_REMS)
            // The sidebar's rows already end one inset short of its edge.
            .when(self.sidebar_visible, |this| this.pl_0())
            .child(
                div()
                    .flex()
                    .flex_col()
                    .size_full()
                    .rounded(theme.radius_lg)
                    .border_1()
                    .border_color(theme.border)
                    .bg(theme.background)
                    .overflow_hidden()
                    .child(header)
                    .child(div().flex_1().min_h_0().child(panes))
                    .child(footer),
            )
            .into_any_element()
    }

    fn render_sheet_footer(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let position = self.editor.read(cx).cursor_position();
        let theme = cx.theme();

        StatusBar::new()
            .bg(gpui_kit::transparent_black())
            .border_color(theme.border)
            .h_7()
            .py_0()
            .px_4()
            .gap_4()
            .left(
                div()
                    .flex()
                    .items_center()
                    .gap_4()
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
            .right(self.word_count.clone())
    }
}

/// The sidebar as a cached view: it re-renders only when the workspace or
/// the file tree notifies, not on every keystroke's frame.
struct WorkspaceSidebar {
    workspace: gpui_kit::WeakEntity<Workspace>,
}

impl Render for WorkspaceSidebar {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.workspace
            .update(cx, |workspace, cx| {
                workspace.render_sidebar(window, cx).into_any_element()
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
        let content = if self.settings_open {
            self.settings.clone().into_any_element()
        } else {
            self.render_panes(window, cx).into_any_element()
        };
        let theme = cx.theme();
        div()
            .id("workspace")
            .key_context(KEY_CONTEXT)
            .track_focus(&self.focus_handle)
            .capture_action(cx.listener(|this, _: &Escape, _, _| {
                this.slash_commands.dismiss();
            }))
            .capture_action(cx.listener(|this, _: &IndentInline, window, cx| {
                // Tab accepts the highlighted template like Enter, through the
                // menu, then writes out its command in place of the template.
                if this.editor.read(cx).completion_menu_state().open
                    && this.slash_commands.complete_next()
                {
                    cx.stop_propagation();
                    let enter = Enter {
                        secondary: false,
                        shift: false,
                    };
                    window.dispatch_action(Box::new(enter), cx);
                }
            }))
            .capture_action(cx.listener(|this, enter: &Enter, window, cx| {
                if !enter.shift && !enter.secondary {
                    this.continue_list(window, cx);
                }
            }))
            .on_action(cx.listener(Self::new_file))
            .on_action(cx.listener(Self::open))
            .on_action(cx.listener(Self::open_folder))
            .on_action(cx.listener(Self::refresh_folder))
            .on_action(cx.listener(Self::open_settings))
            .on_action(cx.listener(Self::close_settings))
            .on_action(cx.listener(Self::toggle_sidebar))
            .on_action(cx.listener(Self::save))
            .on_action(cx.listener(Self::save_as))
            .on_action(cx.listener(|this, _: &ExportHtml, window, cx| {
                this.prompt_export(ExportFormat::Html, window, cx)
            }))
            .on_action(cx.listener(|this, _: &ExportPdf, window, cx| {
                this.prompt_export(ExportFormat::Pdf, window, cx)
            }))
            .on_action(cx.listener(|this, _: &ExportDocx, window, cx| {
                this.prompt_export(ExportFormat::Docx, window, cx)
            }))
            .on_action(cx.listener(Self::show_editor))
            .on_action(cx.listener(Self::show_split))
            .on_action(cx.listener(Self::show_preview))
            .on_action(cx.listener(Self::toggle_theme))
            .on_action(cx.listener(Self::close_window))
            .on_action(cx.listener(Self::quit))
            .size_full()
            .bg(theme.sidebar)
            .text_color(theme.foreground)
            .child(content)
    }
}

/// The sidebar's show/hide control. It sits at the sidebar's top trailing
/// corner while the sidebar shows, and in the sheet's header otherwise.
fn sidebar_toggle(sidebar_visible: bool, cx: &Context<Workspace>) -> Button {
    let (id, icon, label) = if sidebar_visible {
        ("hide-sidebar", IconName::PanelLeftClose, "Hide sidebar")
    } else {
        ("show-sidebar", IconName::PanelLeftOpen, "Show sidebar")
    };
    Button::new(id)
        .small()
        .ghost()
        .icon(icon)
        .accessibility_label(label)
        .tooltip_with_action(label, &ToggleSidebar, Some(KEY_CONTEXT))
        .on_click(
            cx.listener(|this, _, window, cx| this.toggle_sidebar(&ToggleSidebar, window, cx)),
        )
}

/// A resize handle that stays out of sight until hovered or dragged, for
/// panes whose boundary is already drawn by a hairline or a gap.
fn quiet_resize_handle() -> ResizeHandleRenderer {
    Rc::new(|handle, window, cx| {
        if handle.state() == ResizeHandleState::Idle {
            Some(div().into_any_element())
        } else {
            resize_handle_appearance()(handle, window, cx)
        }
    })
}

/// A sidebar placeholder that names what is missing and offers the action
/// that fills it.
fn sidebar_empty_state(icon: IconName, title: &'static str, description: &'static str) -> Empty {
    Empty::new()
        .flex_none()
        .justify_start()
        .px_4()
        .pt_10()
        .header(
            EmptyHeader::new()
                .media(
                    EmptyMedia::new()
                        .with_variant(EmptyMediaVariant::Icon)
                        .child(gpui_kit::component::Icon::new(icon).small()),
                )
                .title(EmptyTitle::new().child(title))
                .description(EmptyDescription::new().child(description)),
        )
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

pub(crate) fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

/// `12345` as `12,345`.
/// An error message as a sentence: capitalized, ending in one period.
fn sentence(message: &str) -> String {
    let message = message.trim().trim_end_matches('.');
    let mut chars = message.chars();
    match chars.next() {
        Some(first) => format!("{}{}.", first.to_uppercase(), chars.as_str()),
        None => String::new(),
    }
}

pub(super) fn format_count(count: usize) -> String {
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
