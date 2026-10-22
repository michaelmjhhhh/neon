use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::folder::{folder_name_error, is_markdown, scan_folder};
use crate::session_file::Session;
use crate::typography::Typography;
use export::ExportFormat;
use gpui_kit::assets::IconName as AssetIcon;
use gpui_kit::base::{ResizeHandleRenderer, ResizeHandleState};
use gpui_kit::component::button::{Button, ButtonVariant, ButtonVariants as _};
use gpui_kit::component::dialog::{DialogAction, DialogClose, DialogFooter};
use gpui_kit::component::empty::{
    Empty, EmptyDescription, EmptyHeader, EmptyMedia, EmptyMediaVariant, EmptyTitle,
};
use gpui_kit::component::input::{
    EditorState, Enter, Escape, Indent, IndentInline, Input, InputEvent, InputState, Outdent,
    OutdentInline,
};
use gpui_kit::component::list::ListItem;
use gpui_kit::component::menu::DropdownMenu as _;
use gpui_kit::component::notification::Notification;
use gpui_kit::component::resizable::{
    ResizableState, h_resizable, resizable_panel, resize_handle_appearance,
};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::status_bar::StatusBar;
use gpui_kit::component::tab::{Tab, TabBar};
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::tree::{Tree, TreeEvent, TreeState};
use gpui_kit::component::{ActiveTheme as _, TitleBar, WindowExt as _};
use gpui_kit::component::{Disableable as _, IconName, Sizable as _};
use gpui_kit::{
    Anchor, AnyElement, AppContext as _, Context, Entity, FocusHandle, Focusable, FontFeatures,
    FontWeight, InteractiveElement as _, IntoElement, ParentElement as _, PathPromptOptions,
    Pixels, Render, ScrollHandle, SharedString, StatefulInteractiveElement as _, StyleRefinement,
    Styled as _, Subscription, Task, Window, div, prelude::FluentBuilder as _,
};
use markdown_preview::MarkdownPreview;

use crate::KEY_CONTEXT;
use crate::document_file::DocumentFile;
use crate::inline_format::{self, Style};
use crate::list_continuation;
use crate::present_now::{self, NativeView};
use crate::scroll_metrics::ScrollMetrics;
use crate::settings::{LineNumbersChanged, SettingsView};
use crate::slash_commands::{Insertion, SlashCommands};
use crate::theme::{self, Appearance};
use crate::word_count::WordCount;
use crate::zen::{Zen, ZenView};

mod tabs;
mod zen_mode;
use tabs::DocumentTab;

#[cfg(feature = "memory-bench")]
#[path = "memory_bench.rs"]
mod memory_bench;
#[cfg(feature = "memory-bench")]
pub use memory_bench::MemoryBench;

gpui_kit::actions!(
    workspace,
    [
        NewFile,
        NewTab,
        CloseTab,
        NextTab,
        PreviousTab,
        NewFolder,
        Open,
        OpenFolder,
        RefreshFolder,
        OpenSettings,
        CloseSettings,
        ToggleSidebar,
        ToggleLineNumbers,
        ToggleBold,
        ToggleItalic,
        Save,
        SaveAs,
        ExportHtml,
        ExportPdf,
        ExportDocx,
        ShowEditor,
        ShowSplit,
        ShowPreview,
        ToggleTheme,
        ToggleZen,
        ExitZen,
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

    fn icon(self) -> AssetIcon {
        match self {
            Self::Editor => AssetIcon::PenLine,
            Self::Split => AssetIcon::Columns2,
            Self::Preview => AssetIcon::BookOpen,
        }
    }

    fn action(self) -> &'static dyn gpui_kit::Action {
        match self {
            Self::Editor => &ShowEditor,
            Self::Split => &ShowSplit,
            Self::Preview => &ShowPreview,
        }
    }

    fn shows_editor(self) -> bool {
        self != Self::Preview
    }
}

/// One document window: owns its open files, each in a tab with its own
/// editor and preview, and the folder beside them.
pub struct Workspace {
    /// Never empty.
    tabs: Vec<DocumentTab>,
    active_tab: usize,
    /// The last tab id handed out.
    next_tab_id: u64,
    tab_scroll: ScrollHandle,
    folder: Option<PathBuf>,
    tree: Entity<TreeState>,
    folder_task: Option<Task<()>>,
    folder_empty: bool,
    expanded_folders: HashSet<SharedString>,
    /// How many Markdown files each listed folder holds, at any depth.
    folder_file_counts: Rc<HashMap<SharedString, usize>>,
    sidebar_visible: bool,
    /// The sidebar and sheet widths, and the split between editor and
    /// preview. Owned here so they outlive the frames that do not show them:
    /// hiding the sidebar or leaving Split does not reset them.
    panes: Entity<ResizableState>,
    split: Entity<ResizableState>,
    /// Starts from the saved setting; the shortcut changes it for this
    /// window only.
    line_numbers: bool,
    /// Settings cover the window's content while open.
    settings_open: bool,
    settings: Entity<SettingsView>,
    sidebar: Entity<WorkspaceSidebar>,
    typography: Entity<Typography>,
    scroll_metrics: Entity<ScrollMetrics>,
    word_count: Entity<WordCount>,
    /// The `/` template menu, kept to place the cursor in what it inserts.
    slash_commands: Rc<SlashCommands>,
    layout: LayoutMode,
    /// While set, the window shows the document alone, as source or
    /// preview; see [`crate::zen`].
    zen: Option<Zen>,
    focus_handle: FocusHandle,
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
        let line_numbers = crate::settings_file::saved(cx).line_numbers;
        let typography = cx.new(|cx| Typography::new(window, cx));
        let tab = DocumentTab::new(0, line_numbers, &slash_commands, &typography, window, cx);
        let scroll_metrics =
            cx.new(|cx| ScrollMetrics::new(tab.editor.clone(), tab.preview.clone(), window, cx));
        let word_count = cx.new(|_| WordCount::default());

        let tree = cx.new(|cx| TreeState::new(cx));
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
                    .filter(|entry| !entry.is_folder() && !entry.is_disabled())
                    .map(|entry| PathBuf::from(entry.item().id.as_ref()));
                if let Some(path) = path
                    && this.tab().document.path() != Some(path.as_path())
                    && !this.is_busy()
                {
                    // A file open in another tab is shown there, not twice.
                    match this.tab_index_of_path(&path) {
                        Some(ix) => this.activate_tab(ix, window, cx),
                        None => this.confirm_discard(window, cx, move |this, window, cx| {
                            this.open_path(path.clone(), window, cx);
                        }),
                    }
                }
            }),
            cx.subscribe(&typography, |this, state, _, cx| {
                let state = state.read(cx);
                let font = state.preview_font(cx);
                let scale = state.preview_size(cx) / 16.;
                for tab in &this.tabs {
                    let font = font.clone();
                    tab.preview
                        .update(cx, |preview, cx| preview.set_typography(font, scale, cx));
                }
                cx.notify();
            }),
            cx.subscribe_in(&settings, window, |this, _, event, window, cx| {
                let LineNumbersChanged(visible) = *event;
                this.set_line_numbers(visible, window, cx);
            }),
            cx.observe_window_appearance(window, |_, window, cx| {
                theme::apply_appearance(window, cx);
            }),
            cx.observe_window_bounds(window, |this, window, cx| {
                if this.zen.is_some() {
                    this.glide_zen_caret(window, cx);
                }
            }),
            cx.observe_window_activation(window, |this, window, cx| {
                if window.is_window_active() {
                    this.remember_session(cx);
                }
            }),
        ];

        let workspace = Self {
            tabs: vec![tab],
            active_tab: 0,
            next_tab_id: 0,
            tab_scroll: ScrollHandle::new(),
            folder: None,
            tree,
            folder_task: None,
            folder_empty: true,
            expanded_folders: HashSet::new(),
            folder_file_counts: Rc::default(),
            sidebar_visible: true,
            panes: cx.new(|_| ResizableState::default()),
            split: cx.new(|_| ResizableState::default()),
            line_numbers,
            settings_open: false,
            settings,
            sidebar,
            typography,
            scroll_metrics,
            word_count,
            slash_commands,
            layout: LayoutMode::default(),
            zen: None,
            focus_handle: cx.focus_handle(),
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
        &self.tab().editor
    }

    pub fn preview(&self) -> &Entity<MarkdownPreview> {
        &self.tab().preview
    }

    pub fn document(&self) -> &DocumentFile {
        &self.tab().document
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
            && self.tabs.len() == 1
            && self.tab().document.path().is_none()
            && !self.tab().document.is_dirty()
            && !self.is_loading()
            && self.tab().editor.read(cx).text().len() == 0
    }

    /// Focus the pane that takes keyboard input in the current layout, or
    /// Settings while they cover the window.
    pub fn focus_content(&self, window: &mut Window, cx: &mut Context<Self>) {
        if self.settings_open {
            self.settings.read(cx).focus_handle(cx).focus(window, cx);
        } else if self
            .zen_view()
            .map_or(self.layout.shows_editor(), |view| view == ZenView::Editor)
        {
            self.tab()
                .editor
                .update(cx, |editor, cx| editor.focus(window, cx));
        } else {
            let handle = self.tab().preview.read(cx).focus_handle(cx);
            handle.focus(window, cx);
        }
    }

    /// Read `path` on a background thread and show it.
    pub fn open_path(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_busy() {
            return;
        }
        if path.is_dir() {
            self.load_folder(path, true, None, window, cx);
            return;
        }
        // A file already open in a tab is shown there instead. This checks
        // the path as given; the read checks it again once resolved.
        if let Some(ix) = self.tab_index_of_path(&path) {
            self.activate_tab(ix, window, cx);
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
                    // Tabs cannot change during a read, so a match here is
                    // another tab, reached through a different path.
                    Ok((path, _))
                        if this.tab().document.path() != Some(path.as_path())
                            && this.tab_index_of_path(&path).is_some() =>
                    {
                        if let Some(ix) = this.tab_index_of_path(&path) {
                            this.activate_tab(ix, window, cx);
                        }
                    }
                    Ok((path, text)) => {
                        let previous_focus = window.focused(cx);
                        this.show_document(Some(path), &text, window, cx);
                        if let Some(focus) = previous_focus {
                            focus.focus(window, cx);
                        }
                    }
                    Err(error) => {
                        this.remember_session(cx);
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
        self.tab()
            .editor_pane
            .update(cx, |pane, cx| pane.set_readonly(readonly, cx));
    }

    fn show_document(
        &mut self,
        path: Option<PathBuf>,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.tab()
            .editor
            .update(cx, |editor, cx| editor.set_value(text, window, cx));
        let source = self.tab().editor.read(cx).text().clone();
        self.word_count
            .update(cx, |count, cx| count.set_source(source.clone(), cx));
        let base_dir = path.as_deref().and_then(Path::parent).map(Arc::from);
        self.tab()
            .preview
            .update(cx, |preview, cx| preview.load_source(source, base_dir, cx));
        let tab = self.tab_mut();
        tab.document = path.map(DocumentFile::saved).unwrap_or_default();
        tab.revealed_cursor = None;
        self.remember_session(cx);
        self.sync_tree_selection(cx);
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
        let Some(ix) = self.tab_index_of_editor(editor) else {
            return;
        };
        let tab = &mut self.tabs[ix];
        let was_dirty = tab.document.is_dirty();
        tab.document.record_edit();
        // Cloning the rope is O(1); the preview flattens it off the UI thread.
        let source = editor.read(cx).text().clone();
        tab.preview
            .update(cx, |preview, cx| preview.set_source(source.clone(), cx));
        if ix != self.active_tab {
            if !was_dirty {
                self.update_window_title(window);
            }
            cx.notify();
            return;
        }
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
            .update(cx, |count, cx| count.set_source(source, cx));
        self.zen_follow_cursor(cx);
        self.hide_zen_chrome(cx);
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
        if self.tab().editor != editor {
            return;
        }
        self.settle_zen_lift(cx);
        let cursor = editor.read(cx).cursor();
        if self.tab().revealed_cursor == Some(cursor) {
            return;
        }
        self.tab_mut().revealed_cursor = Some(cursor);
        self.zen_follow_cursor(cx);
        self.glide_zen_caret(window, cx);
        self.draw_soon(window, cx);
        if self.layout == LayoutMode::Split {
            self.tab()
                .preview
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
        if self.folder.is_some() {
            self.prompt_create_file(window, cx);
        } else {
            self.confirm_discard(window, cx, |this, window, cx| {
                this.show_document(None, "", window, cx);
            });
        }
    }

    fn open(&mut self, _: &Open, window: &mut Window, cx: &mut Context<Self>) {
        self.prompt_open(window, cx);
    }

    /// Ask for a file, then show it in the tab that asked. File panels do
    /// not block the window, so the tab is found again by id when the panel
    /// closes; the user may have switched tabs meanwhile.
    fn prompt_open(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let tab_id = self.tab().id;
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
            this.update_in(cx, |this, window, cx| {
                // A file open in another tab is shown there, with nothing
                // to discard.
                if let Some(ix) = this.tab_index_of_path(&path) {
                    this.activate_tab(ix, window, cx);
                } else if this.return_to_tab(tab_id, window, cx) {
                    this.confirm_discard(window, cx, move |this, window, cx| {
                        this.open_path(path.clone(), window, cx);
                    });
                }
            })
            .ok();
        })
        .detach();
    }

    fn save(&mut self, _: &Save, window: &mut Window, cx: &mut Context<Self>) {
        match self.tab().document.path() {
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
        self.tab()
            .document
            .path()
            .and_then(Path::parent)
            .map(Path::to_path_buf)
            .or_else(|| self.folder.clone())
            .or_else(home_dir)
            .unwrap_or_default()
    }

    fn prompt_save_as(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let directory = self.default_directory();
        let suggested_name = match self.tab().document.path() {
            Some(_) => self.tab().document.display_name(),
            None => "Untitled.md".to_string(),
        };
        let tab_id = self.tab().id;
        let path = cx.prompt_for_new_path(&directory, Some(&suggested_name));
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(path))) = path.await else {
                return;
            };
            this.update_in(cx, |this, window, cx| {
                if this.return_to_tab(tab_id, window, cx) {
                    this.save_to(path, window, cx);
                }
            })
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
            .tab()
            .document
            .path()
            .and_then(Path::file_stem)
            .map(|stem| stem.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Untitled".to_string());
        let suggested_name = format!("{stem}.{}", format.extension());
        let tab_id = self.tab().id;
        let path = cx.prompt_for_new_path(&directory, Some(&suggested_name));
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(path))) = path.await else {
                return;
            };
            this.update_in(cx, |this, window, cx| {
                if this.return_to_tab(tab_id, window, cx) {
                    this.export_to(format, path, window, cx);
                }
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
        let markdown = self.tab().editor.read(cx).text().to_string();
        let title = self
            .tab()
            .document
            .path()
            .and_then(Path::file_stem)
            .map(|stem| stem.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Untitled".to_string());
        let base_dir = self
            .tab()
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
        let tab_id = self.tab().id;
        // Saving as a file another tab shows would leave two tabs on it.
        // A clean one closes once the save lands; edits in one stop the save.
        let mut replaced_tab = None;
        if self.tab().document.path() != Some(path.as_path())
            && let Ok(target) = std::fs::canonicalize(&path)
            && let Some(other) = self
                .tabs
                .iter()
                .find(|tab| tab.id != tab_id && tab.document.path() == Some(target.as_path()))
        {
            if other.document.is_dirty() {
                let name = file_name(&path);
                window.push_notification(
                    Notification::error(format!(
                        "Couldn’t save “{name}”. It is open in another tab with unsaved edits."
                    )),
                    cx,
                );
                return;
            }
            replaced_tab = Some(other.id);
        }
        let revision = self.tab().document.edit_revision();
        let text = self.tab().editor.read(cx).text().clone();
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
                        // Tabs cannot change while a save runs, but the
                        // saved tab is found by id all the same.
                        if let Some(tab) = this.tabs.iter_mut().find(|tab| tab.id == tab_id) {
                            tab.document.record_save(path, revision);
                            let base_dir =
                                tab.document.path().and_then(Path::parent).map(Arc::from);
                            tab.preview
                                .update(cx, |preview, cx| preview.set_base_dir(base_dir, cx));
                        }
                        if let Some(ix) = replaced_tab
                            .and_then(|id| this.tabs.iter().position(|tab| tab.id == id))
                        {
                            this.remove_tab(ix, window, cx);
                        }
                        this.remember_session(cx);
                        this.update_window_title(window);
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
            .tab()
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

    /// Save the folder and file this window shows, so the next launch opens
    /// them again.
    ///
    /// A window still reading what it was asked to open waits: saving then
    /// would record an empty or half-open window if the app quit before the
    /// reads finish. Each read records the state once it ends.
    fn remember_session(&self, cx: &mut Context<Self>) {
        if self.load_task.is_some() || self.folder_task.is_some() {
            return;
        }
        let session = Session {
            folder: self.folder.clone(),
            file: self.tab().document.path().map(Path::to_path_buf),
        };
        crate::session_file::remember(session, cx);
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
                    this.load_folder(path, true, None, window, cx)
                })
                .ok();
            }
        })
        .detach();
    }

    /// Scan `path` into the sidebar tree. `reveal` shows a hidden sidebar:
    /// opening a folder should, but a refresh after a save should not.
    /// `select` is selected in place of the open document, if it is inside
    /// the folder: a folder just created.
    fn load_folder(
        &mut self,
        path: PathBuf,
        reveal: bool,
        select: Option<SharedString>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let scan = cx.background_spawn({
            let path = path.clone();
            async move {
                let path = std::fs::canonicalize(path)?;
                let entries = scan_folder(&path)?;
                let mut counts = HashMap::new();
                let total = entries
                    .iter()
                    .map(|entry| entry.count_files(&mut counts))
                    .sum();
                counts.insert(path.to_string_lossy().into_owned().into(), total);
                Ok::<_, std::io::Error>((path, entries, counts))
            }
        });
        self.folder_task = Some(cx.spawn_in(window, async move |this, cx| {
            let result = scan.await;
            this.update_in(cx, |this, window, cx| {
                this.folder_task = None;
                match result {
                    Ok((path, entries, counts)) => {
                        this.folder_empty = entries.is_empty();
                        this.folder_file_counts = Rc::new(counts);
                        if this.folder.as_ref() != Some(&path) {
                            this.expanded_folders.clear();
                        }
                        // Another folder may have opened since `select` was made.
                        let selected = select
                            .filter(|id| Path::new(id.as_ref()).starts_with(&path))
                            .or_else(|| {
                                this.tab().document.path().map(|path| {
                                    SharedString::from(path.to_string_lossy().into_owned())
                                })
                            });
                        this.folder = Some(path);
                        this.remember_session(cx);
                        this.sidebar_visible |= reveal;
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
                    Err(error) => {
                        this.remember_session(cx);
                        window.push_notification(
                            Notification::error(format!(
                                "Couldn’t read folder “{}”. {error}",
                                path.display()
                            )),
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

    fn refresh_folder(&mut self, _: &RefreshFolder, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(path) = self.folder.clone() {
            self.load_folder(path, false, None, window, cx);
        }
    }

    /// Where a new file or folder goes: into the selected folder, beside the
    /// selected file, or at the top of the open folder.
    fn target_directory(&self, cx: &gpui_kit::App) -> Option<PathBuf> {
        let folder = self.folder.as_ref()?;
        let directory = self
            .tree
            .read(cx)
            .selected_entry()
            .map(|entry| {
                let path = PathBuf::from(entry.item().id.as_ref());
                if entry.is_folder() {
                    path
                } else {
                    path.parent().unwrap_or(folder).to_path_buf()
                }
            })
            .unwrap_or_else(|| folder.clone());
        Some(directory)
    }

    /// Ask for a new file's name, then create it and show it in the tab
    /// that asked, once any edits there may be discarded.
    fn prompt_create_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(directory) = self.target_directory(cx) else {
            return;
        };
        let tab_id = self.tab().id;
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
                if this.return_to_tab(tab_id, window, cx) {
                    this.confirm_discard(window, cx, move |this, window, cx| {
                        this.create_file(path.clone(), window, cx);
                    });
                }
            })
            .ok();
        })
        .detach();
    }

    /// Create an empty file at `path` and show it.
    fn create_file(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_busy() {
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
        self.set_editor_readonly(true, cx);
        self.load_task = Some(cx.spawn_in(window, async move |this, cx| {
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
    }

    /// Ask for a name, then create the folder where a new file would go.
    fn new_folder(&mut self, _: &NewFolder, window: &mut Window, cx: &mut Context<Self>) {
        let Some(directory) = self.target_directory(cx) else {
            return;
        };
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("Folder name"));
        // The name is checked as typed, but only after the first attempt to
        // create, so an empty field does not open with an error.
        let attempted = Rc::new(Cell::new(false));
        let this = cx.weak_entity();
        // The path from the open folder, so two folders with one name differ.
        let location: SharedString = format!(
            "In {}",
            self.folder
                .as_deref()
                .and_then(|folder| {
                    let relative = directory.strip_prefix(folder).ok()?;
                    let parts = std::iter::once(file_name(folder)).chain(
                        relative
                            .components()
                            .map(|part| part.as_os_str().to_string_lossy().into_owned()),
                    );
                    Some(parts.collect::<Vec<_>>().join(" / "))
                })
                .unwrap_or_else(|| file_name(&directory))
        )
        .into();
        window.open_dialog(cx, {
            let input = input.clone();
            move |dialog, window, cx| {
                let name = input.read(cx).value().trim().to_string();
                let error = name_problem(&directory, &name).filter(|_| attempted.get());
                let theme = cx.theme();
                let (muted, danger) = (theme.muted_foreground, theme.danger);
                let on_ok = {
                    let input = input.clone();
                    let attempted = attempted.clone();
                    let this = this.clone();
                    let directory = directory.clone();
                    move |_: &gpui_kit::ClickEvent, window: &mut Window, cx: &mut gpui_kit::App| {
                        let name = input.read(cx).value().trim().to_string();
                        if name_problem(&directory, &name).is_some() {
                            attempted.set(true);
                            window.refresh();
                            return false;
                        }
                        this.update(cx, |this, cx| {
                            this.create_folder(directory.join(name), window, cx)
                        })
                        .ok();
                        true
                    }
                };
                dialog
                    .title("New Folder")
                    .w(window.rem_size() * 22.)
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_2()
                            // The field's 3 px focus ring is drawn outside its
                            // border, and the dialog body clips at its top edge.
                            .pt_1()
                            .child(Input::new(&input))
                            .child(
                                div()
                                    .min_w_0()
                                    // A long path may shorten; an error may not.
                                    .when(error.is_none(), |this| this.truncate())
                                    .text_xs()
                                    .text_color(if error.is_some() { danger } else { muted })
                                    .child(error.unwrap_or_else(|| location.clone())),
                            ),
                    )
                    .footer(
                        DialogFooter::new()
                            .gap_2()
                            .child(
                                DialogClose::new()
                                    .child(Button::new("cancel").label("Cancel").outline()),
                            )
                            .child(
                                DialogAction::new()
                                    .child(Button::new("create-folder").label("Create").primary()),
                            ),
                    )
                    .on_ok(on_ok)
            }
        });
        // The dialog takes focus as it opens; hand it to the field.
        input.update(cx, |input, cx| input.focus(window, cx));
    }

    /// Create the folder on a background thread, then show it selected and
    /// open in the tree, so a new file lands inside it.
    fn create_folder(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let create = cx.background_spawn({
            let path = path.clone();
            async move {
                std::fs::create_dir(&path)?;
                std::fs::canonicalize(path)
            }
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = create.await;
            this.update_in(cx, |this, window, cx| match result {
                Ok(path) => {
                    let id = SharedString::from(path.to_string_lossy().into_owned());
                    this.expanded_folders.insert(id.clone());
                    if let Some(folder) = this.folder.clone() {
                        this.load_folder(folder, false, Some(id), window, cx);
                    }
                }
                Err(error) => window.push_notification(
                    Notification::error(format!(
                        "Couldn’t create “{}”. {}",
                        file_name(&path),
                        sentence(&error.to_string())
                    )),
                    cx,
                ),
            })
            .ok();
        })
        .detach();
    }

    /// Start the next list item when Enter ends one in the editor, in place of
    /// the editor's plain newline.
    fn continue_list(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let editor = self.tab().editor.read(cx);
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
        self.tab().editor.update(cx, |editor, cx| {
            editor.set_selected_range(range, cx);
            editor.replace(text, window, cx);
        });
    }

    /// Whether the editor's cursor sits on a list item, where Tab and
    /// Shift+Tab indent the whole item instead of inserting spaces.
    fn on_list_item(&self, window: &Window, cx: &gpui_kit::App) -> bool {
        let editor = self.tab().editor.read(cx);
        let selection = editor.selected_range();
        !self.is_loading()
            && editor.focus_handle(cx).is_focused(window)
            && !editor.completion_menu_state().open
            && selection.is_empty()
            && list_continuation::is_list_item(editor.text(), selection.start)
    }

    /// Toggle bold or italic markers on the editor's selection.
    fn toggle_style(&mut self, style: Style, window: &mut Window, cx: &mut Context<Self>) {
        let editor = self.tab().editor.read(cx);
        if self.is_loading() || !editor.focus_handle(cx).is_focused(window) {
            return;
        }
        let edit = inline_format::toggle_in_rope(editor.text(), editor.selected_range(), style);
        self.tab().editor.update(cx, |editor, cx| {
            editor.set_selected_range(edit.range, cx);
            editor.replace(edit.text, window, cx);
            editor.set_selected_range(edit.selection, cx);
        });
    }

    fn toggle_sidebar(&mut self, _: &ToggleSidebar, window: &mut Window, cx: &mut Context<Self>) {
        // Zen hides the sidebar; asking for it leaves Zen and shows it.
        if self.zen.is_some() {
            self.exit_zen(window, cx);
            self.sidebar_visible = true;
            cx.notify();
            return;
        }
        self.sidebar_visible = !self.sidebar_visible;
        if !self.sidebar_visible {
            self.focus_content(window, cx);
        }
        cx.notify();
    }

    fn toggle_line_numbers(
        &mut self,
        _: &ToggleLineNumbers,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.set_line_numbers(!self.line_numbers, window, cx);
    }

    fn set_line_numbers(&mut self, visible: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.line_numbers = visible;
        // Zen keeps them hidden until it ends.
        let shown = visible && self.zen.is_none();
        for tab in &self.tabs {
            tab.editor
                .update(cx, |editor, cx| editor.set_line_number(shown, window, cx));
        }
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
        let preview_shown = self.preview_shown();
        self.tab()
            .preview
            .update(cx, |preview, cx| preview.set_active(preview_shown, cx));
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
            .tab()
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
            let total = self
                .folder_file_counts
                .get(folder.to_string_lossy().as_ref())
                .copied()
                .unwrap_or_default();
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
                        .flex()
                        .flex_1()
                        .items_baseline()
                        .gap_1p5()
                        .min_w_0()
                        .text_sm()
                        .child(
                            div()
                                .min_w_0()
                                .truncate()
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(file_name(folder)),
                        )
                        .when(total > 0 && !reading, |this| {
                            this.child(
                                div()
                                    .flex_none()
                                    .text_xs()
                                    .text_color(theme.muted_foreground)
                                    .child(format_count(total)),
                            )
                        }),
                )
                .when(reading, |this| {
                    this.child(Spinner::new().xsmall().color(theme.muted_foreground))
                })
                .child(
                    Button::new("new-file")
                        .small()
                        .ghost()
                        .icon(AssetIcon::FilePlus)
                        .accessibility_label("New file…")
                        .tooltip_with_action("New file…", &NewFile, Some(KEY_CONTEXT))
                        .disabled(self.is_busy())
                        .on_click(
                            cx.listener(|this, _, window, cx| this.new_file(&NewFile, window, cx)),
                        ),
                )
                .child(
                    Button::new("new-folder")
                        .small()
                        .ghost()
                        .icon(AssetIcon::FolderPlus)
                        .accessibility_label("New folder…")
                        .tooltip_with_action("New folder…", &NewFolder, Some(KEY_CONTEXT))
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.new_folder(&NewFolder, window, cx)
                        })),
                )
                .child(
                    Button::new("folder-actions")
                        .small()
                        .ghost()
                        .icon(IconName::Ellipsis)
                        .accessibility_label("Folder actions")
                        .dropdown_menu_with_anchor(Anchor::TopRight, move |menu, _, _| {
                            menu.menu("New Folder…", Box::new(NewFolder))
                                .separator()
                                .menu_with_disabled(
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
                div()
                    .flex()
                    .gap_2()
                    .child(
                        Button::new("new-file-empty")
                            .small()
                            .outline()
                            .icon(AssetIcon::FilePlus)
                            .label("New File…")
                            .disabled(self.is_busy())
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.new_file(&NewFile, window, cx)
                            })),
                    )
                    .child(
                        Button::new("new-folder-empty")
                            .small()
                            .outline()
                            .icon(AssetIcon::FolderPlus)
                            .label("New Folder…")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.new_folder(&NewFolder, window, cx)
                            })),
                    ),
            )
            .into_any_element()
        } else {
            let muted = theme.muted_foreground;
            let active_icon = theme.primary;
            let guide = theme.sidebar_border;
            let row_radius = theme.radius;
            let counts = self.folder_file_counts.clone();
            // A press on a row marks itself here before the press reaches
            // the space around the rows, which clears the selection so new
            // files and folders go to the top of the open folder.
            let row_pressed = Rc::new(Cell::new(false));
            let tree = self.tree.clone();
            div()
                .id("folder-tree")
                .flex_1()
                .min_h_0()
                .pt_1()
                .on_mouse_down(gpui_kit::MouseButton::Left, {
                    let row_pressed = row_pressed.clone();
                    cx.listener(move |this, _, _, cx| {
                        if !row_pressed.replace(false) {
                            this.tree
                                .update(cx, |tree, cx| tree.set_selected_index(None, cx));
                        }
                    })
                })
                .child(Tree::new(&self.tree, move |_, entry, _, window, _| {
                    let item = entry.item();
                    let rem = window.rem_size();
                    let is_active = active.as_deref() == Some(item.id.as_ref());
                    let (chevron, icon) = if entry.is_disabled() {
                        (None, None)
                    } else if entry.is_folder() {
                        if entry.is_expanded() {
                            (Some(IconName::ChevronDown), Some(IconName::FolderOpen))
                        } else {
                            (Some(IconName::ChevronRight), Some(IconName::Folder))
                        }
                    } else {
                        (None, Some(IconName::FileText))
                    };
                    // A collapsed folder says how much it holds.
                    let count = counts
                        .get(&item.id)
                        .copied()
                        .filter(|count| *count > 0 && !entry.is_expanded());
                    // Files show their extension quietly: every row is Markdown.
                    let label = item.label.as_ref();
                    let (stem, extension) = match label.rfind('.') {
                        Some(ix) if ix > 0 && !entry.is_folder() => {
                            (&label[..ix], Some(label[ix..].to_string()))
                        }
                        _ => (label, None),
                    };
                    // One slot per ancestor, each with a hairline under that
                    // ancestor's chevron, so a nested row reads as a branch.
                    let guides = (0..entry.depth()).map(|_| {
                        div()
                            .flex_none()
                            .w(rem * 1.25)
                            .h_7()
                            .pl(rem * 0.5)
                            .child(div().w(gpui_kit::px(1.)).h_full().bg(guide))
                    });
                    // An empty folder's "Empty" row cannot be selected; a press
                    // on it selects the folder, where a new file would go.
                    let select_folder = entry.is_disabled().then(|| {
                        let folder = Path::new(item.id.as_ref())
                            .parent()
                            .map(|path| SharedString::from(path.to_string_lossy().into_owned()));
                        let tree = tree.clone();
                        let row_pressed = row_pressed.clone();
                        move |_: &gpui_kit::MouseDownEvent,
                              _: &mut Window,
                              cx: &mut gpui_kit::App| {
                            row_pressed.set(true);
                            tree.update(cx, |tree, cx| {
                                let ix = folder.as_ref().and_then(|folder| tree.index_of(folder));
                                tree.set_selected_index(ix, cx);
                            });
                        }
                    });
                    let row_pressed = row_pressed.clone();
                    ListItem::new(item.id.clone())
                        .on_mouse_down(gpui_kit::MouseButton::Left, move |_, _, _| {
                            row_pressed.set(true)
                        })
                        .h_7()
                        .mx_2()
                        .px_0()
                        .py_0()
                        .pl(rem * 0.25)
                        .pr_2()
                        .rounded(row_radius)
                        .text_sm()
                        .accessibility_label(item.label.clone())
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .w_full()
                                .min_w_0()
                                .when_some(select_folder, |this, select_folder| {
                                    this.on_mouse_down(gpui_kit::MouseButton::Left, select_folder)
                                })
                                .children(guides)
                                .child(
                                    div()
                                        .flex()
                                        .flex_1()
                                        .items_center()
                                        .gap_1()
                                        .min_w_0()
                                        // Fixed slots keep labels on one spine
                                        // whether or not a row has a chevron.
                                        .child(icon_slot().text_color(muted).children(chevron.map(
                                            |chevron| {
                                                gpui_kit::component::Icon::new(chevron).xsmall()
                                            },
                                        )))
                                        .child(icon_slot().children(icon.map(|icon| {
                                            gpui_kit::component::Icon::new(icon).small().text_color(
                                                if is_active { active_icon } else { muted },
                                            )
                                        })))
                                        .child(
                                            div()
                                                .flex()
                                                .flex_1()
                                                .ml_1()
                                                .min_w_0()
                                                .when(is_active, |row| {
                                                    row.font_weight(FontWeight::MEDIUM)
                                                })
                                                .child(
                                                    div()
                                                        .min_w_0()
                                                        .truncate()
                                                        .child(stem.to_string()),
                                                )
                                                .children(extension.map(|extension| {
                                                    div()
                                                        .flex_none()
                                                        .text_color(muted)
                                                        .child(extension)
                                                })),
                                        )
                                        .children(count.map(|count| {
                                            div()
                                                .flex_none()
                                                .text_xs()
                                                .text_color(muted)
                                                .child(format_count(count))
                                        })),
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
        // In Zen, Editor and Preview switch what Zen shows; Split leaves it.
        if self.zen.is_some() {
            self.set_zen_view(ZenView::Editor, window, cx);
            return;
        }
        self.set_layout(LayoutMode::Editor, window, cx);
    }

    fn show_split(&mut self, _: &ShowSplit, window: &mut Window, cx: &mut Context<Self>) {
        self.set_layout(LayoutMode::Split, window, cx);
    }

    fn show_preview(&mut self, _: &ShowPreview, window: &mut Window, cx: &mut Context<Self>) {
        if self.zen.is_some() {
            self.set_zen_view(ZenView::Preview, window, cx);
            return;
        }
        self.set_layout(LayoutMode::Preview, window, cx);
    }

    pub fn set_layout(&mut self, layout: LayoutMode, window: &mut Window, cx: &mut Context<Self>) {
        // Choosing a layout leaves Zen, even for the one Zen started from.
        self.exit_zen(window, cx);
        if self.layout == layout {
            return;
        }
        self.layout = layout;
        let preview_shown = self.preview_shown();
        self.tab()
            .preview
            .update(cx, |preview, cx| preview.set_active(preview_shown, cx));
        self.scroll_metrics
            .update(cx, |metrics, cx| metrics.set_layout(layout, cx));
        if layout != LayoutMode::Editor {
            let cursor = self.tab().editor.read(cx).cursor();
            self.tab()
                .preview
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
        self.confirm_discard_all(window, cx, |_, window, _| window.remove_window());
    }

    fn quit(&mut self, _: &Quit, window: &mut Window, cx: &mut Context<Self>) {
        self.confirm_discard_all(window, cx, |_, _, cx| cx.quit());
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
            if !this.read(cx).tabs.iter().any(|tab| tab.document.is_dirty()) {
                return true;
            }
            this.update(cx, |this, cx| {
                this.confirm_discard_all(window, cx, |_, window, _| window.remove_window());
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
        if !self.tab().document.is_dirty() {
            then(self, window, cx);
            return;
        }
        let title = format!(
            "Discard changes to “{}”?",
            self.tab().document.display_name()
        );
        // The alert names this tab, so its answer applies to this tab even
        // if another was shown meanwhile, from the menu bar or Finder.
        let tab_id = self.tab().id;
        self.ask_to_discard(title.into(), window, cx, move |this, window, cx| {
            if this.return_to_tab(tab_id, window, cx) {
                then(this, window, cx);
            }
        });
    }

    /// Run `then` now, or after the user agrees to drop the unsaved edits in
    /// every tab. The first edited tab comes to the front, so the alert names
    /// what the window shows.
    fn confirm_discard_all(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        then: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) {
        if self.is_busy() {
            return;
        }
        let edited: Vec<usize> = (0..self.tabs.len())
            .filter(|ix| self.tabs[*ix].document.is_dirty())
            .collect();
        let Some(&first) = edited.first() else {
            then(self, window, cx);
            return;
        };
        self.activate_tab(first, window, cx);
        if edited.len() == 1 {
            self.confirm_discard(window, cx, then);
        } else {
            let title = format!("Discard changes to {} documents?", edited.len());
            self.ask_to_discard(title.into(), window, cx, then);
        }
    }

    fn ask_to_discard(
        &mut self,
        title: SharedString,
        window: &mut Window,
        cx: &mut Context<Self>,
        then: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) {
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
        window.set_window_title(&format!("{} — Neon", self.tab().document.display_name()));
        // The close button's dot warns about edits in any tab.
        window.set_window_edited(self.tabs.iter().any(|tab| tab.document.is_dirty()));
    }

    /// The sheet's header doubles as the window's title bar: it names the
    /// document and whether it has unsaved edits, and its trailing edge holds
    /// Save, then the view controls, then Settings.
    fn render_sheet_header(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let selected = LayoutMode::ALL
            .iter()
            .position(|mode| *mode == self.layout)
            .unwrap_or_default();
        let is_dirty = self.tab().document.is_dirty();
        // Without the sidebar, the window controls sit on this header, so its
        // content starts after them. They do not follow the rem size.
        let clears_window_controls = !self.sidebar_visible && !window.is_fullscreen();
        let save_label = if self.tab().document.path().is_some() {
            "Save"
        } else {
            "Save…"
        };

        let (stem, extension) = split_extension(&self.tab().document.display_name());

        // Shown once there are edits to save, always in the same place: it
        // leads the trailing controls, so it appears without moving them.
        let save_button = is_dirty.then(|| {
            Button::new("save-file")
                .xsmall()
                .outline()
                .label(save_label)
                .tooltip_with_action(save_label, &Save, Some(KEY_CONTEXT))
                .loading(self.save_task.is_some())
                .disabled(self.is_busy())
                .on_click(cx.listener(|this, _, window, cx| this.save(&Save, window, cx)))
        });
        // With two or more documents open, tabs take the title's place and
        // each marks its own unsaved edits.
        let tabbed = self.tabs.len() > 1;

        // The single title, while no tabs take its place.
        let title = (!tabbed).then(|| {
            div()
                .flex()
                .items_center()
                .gap_1()
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
                            gpui_kit::component::Icon::new(IconName::ChevronRight)
                                .xsmall()
                                .flex_none()
                                .text_color(theme.muted_foreground),
                        )
                    },
                )
                .child(
                    // The name leads; its extension is recessive, so names that
                    // share a type still differ at a glance.
                    div()
                        .flex()
                        .flex_shrink_0()
                        .max_w_80()
                        .min_w_0()
                        .child(
                            div()
                                .min_w_0()
                                .truncate()
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(stem),
                        )
                        .when_some(extension, |this, extension| {
                            this.child(
                                div()
                                    .flex_none()
                                    .text_color(theme.muted_foreground)
                                    .child(extension),
                            )
                        }),
                )
        });

        TitleBar::new()
            .h(window.rem_size() * SHEET_HEADER_REMS)
            .map(|this| {
                if clears_window_controls {
                    this.pl(WINDOW_CONTROLS_END - window.rem_size() * SHEET_INSET_REMS)
                } else if tabbed {
                    // The first tab's name lines up with the single title.
                    this.pl_1p5()
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
                    .children(title)
                    .when(tabbed, |this| this.child(self.render_tab_strip(window, cx)))
                    .when(is_dirty && !tabbed, |this| {
                        // A dot and a word: the state does not rely on color.
                        // With tabs, each tab marks its own edits.
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
                    }),
            )
            .child(
                // Three groups, separated more widely than their members:
                // the document command, how to view it, and the app.
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap_3()
                    .children(save_button)
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_1()
                            .child(
                                TabBar::new("layout")
                                    .segmented()
                                    .small()
                                    .children(LayoutMode::ALL.map(|mode| {
                                        // Icons keep the switch compact; the
                                        // tooltip names each mode and its
                                        // shortcut.
                                        Tab::new()
                                            .icon(mode.icon())
                                            .aria_label(mode.label())
                                            .tooltip(move |window, cx| {
                                                Tooltip::new(mode.label())
                                                    .action(mode.action(), Some(KEY_CONTEXT))
                                                    .build(window, cx)
                                            })
                                    }))
                                    .selected_index(selected)
                                    .on_click(cx.listener(|this, ix: &usize, window, cx| {
                                        if let Some(mode) = LayoutMode::ALL.get(*ix) {
                                            this.set_layout(*mode, window, cx);
                                        }
                                    })),
                            )
                            .child(zen_mode::enter_zen_button(cx)),
                    )
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
                    ),
            )
    }

    /// The folder sidebar beside the document sheet. Without the sidebar the
    /// sheet stands alone, and the group's sizes wait untouched for its
    /// return: a group with one panel would forget the sidebar's width.
    fn render_panes(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        if !self.sidebar_visible {
            return self.render_sheet(window, cx);
        }
        h_resizable("workspace-panes")
            .with_state(&self.panes)
            .with_handle_appearance(quiet_resize_handle())
            .child(
                resizable_panel()
                    .size(window.rem_size() * SIDEBAR_WIDTH_REMS)
                    .size_range(window.rem_size() * 12. ..window.rem_size() * 22.)
                    .child(
                        self.sidebar
                            .clone()
                            .cached(StyleRefinement::default().size_full()),
                    ),
            )
            .child(
                resizable_panel()
                    .size_range(window.rem_size() * 20. ..Pixels::MAX)
                    .child(self.render_sheet(window, cx)),
            )
            .into_any_element()
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
                .child(self.tab().editor_pane.clone().cached(pane_style()))
        };
        let border = cx.theme().border;
        let preview = |divided: bool| {
            div()
                .size_full()
                .min_w_0()
                .when(divided, |this| this.border_l_1().border_color(border))
                .child(MarkdownPreview::pane(&self.tab().preview, cx))
        };

        let panes = match self.layout {
            LayoutMode::Editor => editor().into_any_element(),
            LayoutMode::Preview => preview(false).into_any_element(),
            LayoutMode::Split => h_resizable("split")
                .with_state(&self.split)
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
        let position = self.tab().editor.read(cx).cursor_position();
        let theme = cx.theme();

        StatusBar::new()
            .bg(gpui_kit::transparent_black())
            .border_color(theme.border)
            .h_7()
            .py_0()
            .px_4()
            .gap_4()
            // Equal-width digits, so counts do not shift as they change.
            .font_features(FontFeatures(Arc::new(vec![("tnum".into(), 1)])))
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
            .right(self.word_count.clone())
            .right(self.scroll_metrics.clone())
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
        } else if self.zen.is_some() {
            self.render_zen(window, cx).into_any_element()
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
                if this.tab().editor.read(cx).completion_menu_state().open
                    && this.slash_commands.complete_next()
                {
                    cx.stop_propagation();
                    let enter = Enter {
                        secondary: false,
                        shift: false,
                    };
                    window.dispatch_action(Box::new(enter), cx);
                } else if this.on_list_item(window, cx) {
                    cx.stop_propagation();
                    window.dispatch_action(Box::new(Indent), cx);
                }
            }))
            .capture_action(cx.listener(|this, _: &OutdentInline, window, cx| {
                if this.on_list_item(window, cx) {
                    cx.stop_propagation();
                    window.dispatch_action(Box::new(Outdent), cx);
                }
            }))
            .capture_action(cx.listener(|this, enter: &Enter, window, cx| {
                if !enter.shift && !enter.secondary {
                    this.continue_list(window, cx);
                }
            }))
            .on_action(cx.listener(Self::new_file))
            .on_action(cx.listener(Self::new_tab))
            .on_action(cx.listener(Self::close_tab))
            .on_action(cx.listener(Self::next_tab))
            .on_action(cx.listener(Self::previous_tab))
            .on_action(cx.listener(Self::new_folder))
            .on_action(cx.listener(Self::open))
            .on_action(cx.listener(Self::open_folder))
            .on_action(cx.listener(Self::refresh_folder))
            .on_action(cx.listener(Self::open_settings))
            .on_action(cx.listener(Self::close_settings))
            .on_action(cx.listener(Self::toggle_sidebar))
            .on_action(cx.listener(Self::toggle_line_numbers))
            .on_action(cx.listener(|this, _: &ToggleBold, window, cx| {
                this.toggle_style(Style::Bold, window, cx);
            }))
            .on_action(cx.listener(|this, _: &ToggleItalic, window, cx| {
                this.toggle_style(Style::Italic, window, cx);
            }))
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
            .on_action(cx.listener(Self::toggle_zen))
            .on_action(cx.listener(Self::exit_zen_action))
            // Escape that nothing inside had a use for leaves Zen.
            .on_action(cx.listener(|this, _: &Escape, window, cx| {
                if this.zen.is_some() {
                    this.exit_zen(window, cx);
                } else {
                    cx.propagate();
                }
            }))
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

/// A one-rem square that centers a tree row's chevron or icon.
fn icon_slot() -> gpui_kit::Div {
    div()
        .flex_none()
        .size_4()
        .flex()
        .items_center()
        .justify_center()
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

/// Why `name` cannot name a new folder in `directory`, if it cannot. The
/// dialog says so in place, rather than failing after it closes.
fn name_problem(directory: &Path, name: &str) -> Option<SharedString> {
    if let Some(error) = folder_name_error(name) {
        return Some(error.into());
    }
    directory
        .join(name)
        .exists()
        .then(|| format!("“{name}” already exists here.").into())
}

/// `notes.md` as `notes` and `.md`, so the extension can recede. A name
/// with no stem before its dot, like `.env`, stays whole.
fn split_extension(name: &str) -> (String, Option<String>) {
    match name.rsplit_once('.') {
        Some((stem, extension)) if !stem.is_empty() => {
            (stem.to_string(), Some(format!(".{extension}")))
        }
        _ => (name.to_string(), None),
    }
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
