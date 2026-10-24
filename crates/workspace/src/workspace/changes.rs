//! The sidebar's Changes view: what changed in the Git repository the open
//! folder is in, grouped by folder, which of it the next commit takes, the
//! commit message, and pushing. A change picked in the list shows its diff
//! in the sheet, in place of the document, until Escape, Edit or another
//! view.
//!
//! The view, and History beside it, appear only for a folder inside a Git
//! working tree.

use std::collections::HashSet;
use std::hash::{DefaultHasher, Hash as _, Hasher as _};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::assets::IconName as AssetIcon;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::diff::{
    Diff, DiffChangeIndicator, DiffFile, DiffFileStatus, DiffHunkSeparator, DiffState,
};
use gpui_kit::component::empty::{
    Empty, EmptyDescription, EmptyHeader, EmptyMedia, EmptyMediaVariant, EmptyTitle,
};
use gpui_kit::component::input::{Enter, InputEvent, Textarea, TextareaState};
use gpui_kit::component::list::ListItem;
use gpui_kit::component::menu::DropdownMenu as _;
use gpui_kit::component::notification::Notification;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::status_bar::StatusBar;
use gpui_kit::component::tab::{Tab, TabBar};
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, IconName, Sizable as _, TitleBar, WindowExt as _,
};
use gpui_kit::{
    Anchor, AnyElement, AppContext as _, Context, Entity, Focusable as _, FontFeatures, FontWeight,
    InteractiveElement as _, IntoElement, ParentElement as _, SharedString,
    StatefulInteractiveElement as _, Styled as _, Subscription, Task, UniformListScrollHandle,
    Window, div, prelude::FluentBuilder as _, uniform_list,
};

use super::{
    RefreshFolder, SHEET_HEADER_REMS, Workspace, format_count, sentence, sidebar_empty_state,
    split_extension,
};
use crate::KEY_CONTEXT;
use crate::change_tree::{ChangeRow, change_rows, paths_inside};
use crate::git::{Change, ChangeKind, GitError, Repo, Staged, Status};

gpui_kit::actions!(
    workspace,
    [
        ShowFiles,
        ShowChanges,
        ShowHistory,
        StageAll,
        UnstageAll,
        Commit,
        Push
    ]
);

/// What the sidebar lists.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum SidebarView {
    #[default]
    Files,
    Changes,
    History,
}

/// A commit or push in flight. Staging has no state of its own: a checkbox
/// shows its new value at once.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Operation {
    Committing,
    Pushing,
}

/// The window's Git state, found again each time the folder is read.
pub(super) struct Changes {
    /// The working tree the open folder is in; `None` outside one, or
    /// without Git installed.
    repo: Option<Repo>,
    /// The last status read, kept while the next one runs.
    status: Option<Status>,
    status_error: Option<SharedString>,
    status_task: Option<Task<()>>,
    /// A refresh asked for while one ran; it runs when that one ends.
    refresh_queued: bool,
    /// Counts the commands that change the repository. A status read that
    /// started before one of them is stale when it lands.
    revision: u64,
    discover_task: Option<Task<()>>,
    /// The last command that changes the repository. The next waits for it,
    /// so two never hold Git's index lock at once.
    git_task: Option<Task<()>>,
    /// Commands that change the repository, started and not yet done. A
    /// status read landing meanwhile may predate them, so it is dropped.
    git_pending: usize,
    operation: Option<Operation>,
    message: Entity<TextareaState>,
    /// Whether the message is blank, so typing redraws the panel only when
    /// that, and with it the Commit button, changes.
    message_empty: bool,
    /// The change whose diff the sheet shows, by path.
    selected: Option<String>,
    diff: Entity<DiffState>,
    /// The path the diff shows and a hash of its patch, so an unchanged
    /// patch is not installed again and the reader keeps their place.
    shown_patch: Option<(String, u64)>,
    diff_error: Option<SharedString>,
    diff_task: Option<Task<()>>,
    /// Folders of the list shown closed, by path.
    collapsed: HashSet<String>,
    /// The list's rows, made again when the status or a folder changes
    /// rather than on every frame.
    rows: Rc<Vec<ListRow>>,
    list_scroll: UniformListScrollHandle,
    _subscriptions: Vec<Subscription>,
}

impl Changes {
    pub(super) fn new(window: &mut Window, cx: &mut Context<Workspace>) -> Self {
        let message = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(2, 8)
                .placeholder("Message (⌘↵ to commit)")
        });
        let diff = cx.new(|cx| DiffState::new([], cx));
        // The Commit button follows whether a message is written.
        let subscriptions = vec![
            // The diff measures itself once painted and asks for another
            // frame through its state; the sheet that shows it must follow.
            cx.observe(&diff, |_, _, cx| cx.notify()),
            cx.subscribe(&message, |this, message, event, cx| {
                if matches!(event, InputEvent::Change) {
                    let empty = message.read(cx).value().trim().is_empty();
                    if empty != this.changes.message_empty {
                        this.changes.message_empty = empty;
                        cx.notify();
                    }
                }
            }),
        ];
        Self {
            repo: None,
            status: None,
            status_error: None,
            status_task: None,
            refresh_queued: false,
            revision: 0,
            discover_task: None,
            git_task: None,
            git_pending: 0,
            operation: None,
            message,
            message_empty: true,
            selected: None,
            diff,
            shown_patch: None,
            diff_error: None,
            diff_task: None,
            collapsed: HashSet::new(),
            rows: Rc::default(),
            list_scroll: UniformListScrollHandle::new(),
            _subscriptions: subscriptions,
        }
    }

    pub(super) fn repo(&self) -> Option<&Repo> {
        self.repo.as_ref()
    }

    fn change(&self, path: &str) -> Option<&Change> {
        self.status.as_ref()?.change(path)
    }

    /// Make the list's rows again from the status and the closed folders.
    fn rebuild_rows(&mut self) {
        let Some(status) = &self.status else {
            self.rows = Rc::default();
            return;
        };
        let changes = status.changes();
        let rows = change_rows(changes, &self.collapsed)
            .into_iter()
            .map(|row| match row {
                ChangeRow::Folder {
                    path,
                    label,
                    depth,
                    count,
                    staged,
                    expanded,
                } => ListRow::Folder {
                    path: path.into(),
                    label: label.into(),
                    depth,
                    included: match staged {
                        0 => Included::None,
                        staged if staged == count => Included::All,
                        _ => Included::Some,
                    },
                    expanded,
                },
                ChangeRow::File { index, depth } => {
                    let change = &changes[index];
                    let path = change.path();
                    let name = path.rsplit('/').next().unwrap_or(path);
                    // As in the Files view, Markdown names go without `.md`.
                    let name = if crate::folder::is_markdown(Path::new(name)) {
                        split_extension(name).0
                    } else {
                        name.to_string()
                    };
                    ListRow::File {
                        path: path.to_string().into(),
                        name: name.into(),
                        depth,
                        included: match change.staged() {
                            Staged::No => Included::None,
                            Staged::Partly => Included::Some,
                            Staged::Yes => Included::All,
                        },
                        mark: Mark::of(change),
                    }
                }
            })
            .collect();
        self.rows = Rc::new(rows);
    }

    fn staged_count(&self) -> usize {
        self.status.as_ref().map_or(0, |status| {
            status
                .changes()
                .iter()
                .filter(|change| change.staged() != Staged::No)
                .count()
        })
    }
}

impl Workspace {
    pub(super) fn has_repository(&self) -> bool {
        self.changes.repo.is_some()
    }

    /// Whether the sheet shows a change's or a commit's diff in place of
    /// the document.
    pub(super) fn showing_diff(&self) -> bool {
        let picked = match self.sidebar_view {
            SidebarView::Files => false,
            SidebarView::Changes => self.changes.selected.is_some(),
            SidebarView::History => self.history.selected().is_some(),
        };
        picked && self.sidebar_visible && self.zen.is_none() && !self.settings_open
    }

    /// Find the repository `folder` is in, then read its status. Runs on
    /// every folder read, so a `git init` in Terminal shows up on refresh.
    pub(super) fn discover_repository(&mut self, folder: PathBuf, cx: &mut Context<Self>) {
        let discover = cx.background_spawn(async move { Repo::discover(&folder) });
        self.changes.discover_task = Some(cx.spawn(async move |this, cx| {
            let repo = discover.await;
            this.update(cx, |this, cx| {
                this.changes.discover_task = None;
                let moved =
                    this.changes.repo.as_ref().map(Repo::root) != repo.as_ref().map(Repo::root);
                if moved {
                    this.changes.status = None;
                    this.changes.status_error = None;
                    this.changes.selected = None;
                    this.changes.shown_patch = None;
                    this.changes.collapsed.clear();
                    this.changes.rebuild_rows();
                    this.reset_history();
                }
                this.changes.repo = repo;
                if this.changes.repo.is_some() {
                    this.refresh_changes(cx);
                } else {
                    this.sidebar_view = SidebarView::Files;
                }
                cx.notify();
            })
            .ok();
        }));
    }

    /// Read the repository's status again, and the shown diff with it.
    pub(super) fn refresh_changes(&mut self, cx: &mut Context<Self>) {
        let Some(repo) = self.changes.repo.clone() else {
            return;
        };
        if self.changes.status_task.is_some() {
            self.changes.refresh_queued = true;
            return;
        }
        let revision = self.changes.revision;
        let read = cx.background_spawn(async move { repo.status() });
        self.changes.status_task = Some(cx.spawn(async move |this, cx| {
            let result = read.await;
            this.update(cx, |this, cx| {
                this.changes.status_task = None;
                // A read from before a command that changed the repository
                // would undo what its checkbox already shows. While one
                // runs, its end reads the status again.
                let pending = this.changes.git_pending > 0;
                let stale = pending || this.changes.revision != revision;
                let queued = std::mem::take(&mut this.changes.refresh_queued);
                if !pending && (stale || queued) {
                    this.refresh_changes(cx);
                }
                if stale {
                    return;
                }
                match result {
                    Ok(status) => {
                        this.changes.status_error = None;
                        // A committed or reverted change leaves the list; the
                        // sheet goes back to the document.
                        if let Some(path) = &this.changes.selected
                            && status.change(path).is_none()
                        {
                            this.changes.selected = None;
                        }
                        this.changes.status = Some(status);
                        this.changes.rebuild_rows();
                        this.load_diff(cx);
                        // A commit, push or branch switch changes the history.
                        if this.history_visible() {
                            this.load_history(cx);
                        }
                    }
                    Err(error) => {
                        this.changes.status_error = Some(sentence(&error.to_string()).into());
                    }
                }
                cx.notify();
            })
            .ok();
        }));
    }

    /// Run `command` after any Git command still running, then read the
    /// status again. `done` gets the result back on the main thread.
    fn run_git<T: Send + 'static>(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        command: impl FnOnce(&Repo) -> Result<T, GitError> + Send + 'static,
        done: impl FnOnce(&mut Self, Result<T, GitError>, &mut Window, &mut Context<Self>) + 'static,
    ) {
        let Some(repo) = self.changes.repo.clone() else {
            return;
        };
        self.changes.revision += 1;
        self.changes.git_pending += 1;
        let previous = self.changes.git_task.take();
        self.changes.git_task = Some(cx.spawn_in(window, async move |this, cx| {
            if let Some(previous) = previous {
                previous.await;
            }
            let result = cx.background_spawn(async move { command(&repo) }).await;
            this.update_in(cx, |this, window, cx| {
                this.changes.git_pending -= 1;
                done(this, result, window, cx);
                this.changes.revision += 1;
                this.refresh_changes(cx);
                cx.notify();
            })
            .ok();
        }));
    }

    /// Stage or unstage the changes at `paths`. Their checkboxes show the
    /// new state at once; a failure puts them back on the next status read.
    fn set_staged(
        &mut self,
        paths: Vec<String>,
        stage: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.changes.operation.is_some() {
            return;
        }
        let Some(status) = self.changes.status.as_mut() else {
            return;
        };
        let changes: Vec<Change> = status
            .changes_mut()
            .iter_mut()
            .filter(|change| paths.iter().any(|path| path == change.path()))
            .map(|change| {
                let original = change.clone();
                change.set_staged(stage);
                original
            })
            .collect();
        if changes.is_empty() {
            return;
        }
        self.changes.rebuild_rows();
        self.run_git(
            window,
            cx,
            move |repo| {
                if stage {
                    repo.stage(&changes)
                } else {
                    repo.unstage(&changes)
                }
            },
            move |_, result, window, cx| {
                if let Err(error) = result {
                    let verb = if stage { "stage" } else { "unstage" };
                    window.push_notification(
                        Notification::error(format!(
                            "Couldn’t {verb} the files. {}",
                            sentence(&error.to_string())
                        )),
                        cx,
                    );
                }
            },
        );
        cx.notify();
    }

    pub(super) fn stage_all(&mut self, _: &StageAll, window: &mut Window, cx: &mut Context<Self>) {
        self.stage_every(true, window, cx);
    }

    pub(super) fn unstage_all(
        &mut self,
        _: &UnstageAll,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.stage_every(false, window, cx);
    }

    fn stage_every(&mut self, stage: bool, window: &mut Window, cx: &mut Context<Self>) {
        let paths = self
            .changes
            .status
            .iter()
            .flat_map(Status::changes)
            // Stage All takes the rest of partly staged files; Unstage All
            // takes back any part.
            .filter(|change| match change.staged() {
                Staged::No => stage,
                Staged::Partly => true,
                Staged::Yes => !stage,
            })
            .map(|change| change.path().to_string())
            .collect();
        self.set_staged(paths, stage, window, cx);
    }

    fn can_commit(&self) -> bool {
        self.changes.operation.is_none()
            && self.changes.staged_count() > 0
            && !self.changes.message_empty
    }

    pub(super) fn commit(&mut self, _: &Commit, window: &mut Window, cx: &mut Context<Self>) {
        if !self.can_commit() {
            return;
        }
        let message = self.changes.message.read(cx).value().trim().to_string();
        self.changes.operation = Some(Operation::Committing);
        self.run_git(
            window,
            cx,
            move |repo| repo.commit(&message),
            |this, result, window, cx| {
                this.changes.operation = None;
                match result {
                    // The committed files leave the list, which shows it worked.
                    Ok(()) => {
                        this.changes.message.update(cx, |message, cx| {
                            message.set_value("", window, cx);
                        });
                        // Setting the value emits no change event.
                        this.changes.message_empty = true;
                    }
                    Err(error) => window.push_notification(
                        Notification::error(format!(
                            "Couldn’t commit. {}",
                            sentence(&error.to_string())
                        )),
                        cx,
                    ),
                }
            },
        );
        cx.notify();
    }

    pub(super) fn push(&mut self, _: &Push, window: &mut Window, cx: &mut Context<Self>) {
        if self.changes.operation.is_some() {
            return;
        }
        let Some(status) = self.changes.status.clone().filter(Status::can_push) else {
            return;
        };
        self.changes.operation = Some(Operation::Pushing);
        self.run_git(
            window,
            cx,
            move |repo| {
                let branch = status.branch().unwrap_or("The branch").to_string();
                repo.push(&status).map(|remote| (branch, remote))
            },
            |this, result, window, cx| {
                this.changes.operation = None;
                match result {
                    // Nothing in the window changes but a count, so say so.
                    Ok((branch, remote)) => {
                        window.push_notification(
                            Notification::success(format!("Pushed {branch} to {remote}.")),
                            cx,
                        );
                    }
                    Err(error) => window.push_notification(
                        Notification::error(format!("Couldn’t push. {}", push_problem(&error))),
                        cx,
                    ),
                }
            },
        );
        cx.notify();
    }

    pub(super) fn show_files(&mut self, _: &ShowFiles, _: &mut Window, cx: &mut Context<Self>) {
        self.set_sidebar_view(SidebarView::Files, cx);
    }

    pub(super) fn show_changes(
        &mut self,
        _: &ShowChanges,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.require_repository("changes", window, cx) {
            self.set_sidebar_view(SidebarView::Changes, cx);
        }
    }

    pub(super) fn show_history(
        &mut self,
        _: &ShowHistory,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.require_repository("history", window, cx) {
            self.set_sidebar_view(SidebarView::History, cx);
        }
    }

    /// Whether the folder is in a repository; says why not if it isn't.
    fn require_repository(&self, what: &str, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.has_repository() {
            return true;
        }
        let message = match &self.folder {
            Some(folder) => format!(
                "“{}” isn’t in a Git repository, so it has no {what} to show.",
                super::file_name(folder)
            ),
            None => format!("Open a folder in a Git repository to see its {what}."),
        };
        window.push_notification(Notification::info(message), cx);
        false
    }

    fn set_sidebar_view(&mut self, view: SidebarView, cx: &mut Context<Self>) {
        if view != SidebarView::Files && !self.has_repository() {
            return;
        }
        let shown = self.sidebar_visible && self.sidebar_view == view;
        self.sidebar_visible = true;
        self.sidebar_view = view;
        if view == SidebarView::Changes && !shown {
            // Open on the document being written, if it changed; otherwise
            // on the first change.
            let active = self.tab().document.path().and_then(|path| {
                let repo = self.changes.repo.as_ref()?;
                let relative = path.strip_prefix(repo.root()).ok()?;
                Some(relative.to_string_lossy().replace('\\', "/"))
            });
            let changes = self.changes.status.as_ref().map(Status::changes);
            let selected = active
                .filter(|path| changes.is_some_and(|c| c.iter().any(|c| c.path() == path)))
                .or_else(|| {
                    changes
                        .and_then(|c| c.first())
                        .map(|c| c.path().to_string())
                });
            self.changes.selected = selected;
            self.changes.diff_error = None;
            // Diffs load only while Changes shows, so this one may be stale.
            self.load_diff(cx);
            self.refresh_changes(cx);
        }
        if view == SidebarView::History && !shown {
            self.load_history(cx);
        }
        cx.notify();
    }

    fn select_change(&mut self, path: Option<String>, cx: &mut Context<Self>) {
        if self.changes.selected == path {
            return;
        }
        self.changes.selected = path;
        self.changes.diff_error = None;
        self.load_diff(cx);
        cx.notify();
    }

    /// Back to the document from a change's or a commit's diff.
    pub(super) fn close_diff(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.sidebar_view {
            SidebarView::History => self.select_commit(None, cx),
            _ => self.select_change(None, cx),
        }
        self.focus_content(window, cx);
    }

    fn toggle_folder(&mut self, path: &str, cx: &mut Context<Self>) {
        if !self.changes.collapsed.remove(path) {
            self.changes.collapsed.insert(path.to_string());
        }
        self.changes.rebuild_rows();
        cx.notify();
    }

    /// Stage or unstage everything inside the folder at `path`.
    fn stage_folder(
        &mut self,
        path: &str,
        stage: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(status) = &self.changes.status else {
            return;
        };
        let paths = paths_inside(status.changes(), path);
        self.set_staged(paths, stage, window, cx);
    }

    /// Open the shown change's file in the editor, in place of its diff.
    fn edit_change(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(path) = self
            .changes
            .selected
            .as_ref()
            .zip(self.changes.repo.as_ref())
            .map(|(path, repo)| repo.absolute(path))
        else {
            return;
        };
        self.close_diff(window, cx);
        let path = std::fs::canonicalize(&path).unwrap_or(path);
        match self.tab_index_of_path(&path) {
            Some(ix) => self.activate_tab(ix, window, cx),
            None => self.confirm_discard(window, cx, move |this, window, cx| {
                this.open_path(path.clone(), window, cx);
            }),
        }
    }

    /// Read the selected change's patch into the diff, unless it is the one
    /// already shown.
    fn load_diff(&mut self, cx: &mut Context<Self>) {
        // Nothing shows it outside the Changes view; entering it loads.
        if self.sidebar_view != SidebarView::Changes {
            self.changes.diff_task = None;
            return;
        }
        let (Some(repo), Some(change), Some(status)) = (
            self.changes.repo.clone(),
            self.changes
                .selected
                .as_deref()
                .and_then(|path| self.changes.change(path))
                .cloned(),
            self.changes.status.as_ref(),
        ) else {
            self.changes.diff_task = None;
            return;
        };
        let has_commits = status.has_commits();
        let read = cx.background_spawn(async move {
            let patch = repo.diff(&change, has_commits)?;
            let files = DiffFile::parse(&patch)
                .map_err(|error| GitError::new(error.message().to_string()))?;
            let mut hasher = DefaultHasher::new();
            patch.hash(&mut hasher);
            Ok::<_, GitError>((change.path().to_string(), hasher.finish(), files))
        });
        self.changes.diff_task = Some(cx.spawn(async move |this, cx| {
            let result = read.await;
            this.update(cx, |this, cx| {
                this.changes.diff_task = None;
                match result {
                    Ok((path, patch, files)) => {
                        if this.changes.selected.as_ref() != Some(&path) {
                            return;
                        }
                        this.changes.diff_error = None;
                        let shown = Some((path, patch));
                        if this.changes.shown_patch != shown {
                            this.changes.shown_patch = shown;
                            this.changes
                                .diff
                                .update(cx, |diff, cx| diff.set_files(files, cx));
                        }
                    }
                    Err(error) => {
                        this.changes.diff_error = Some(sentence(&error.to_string()).into());
                    }
                }
                cx.notify();
            })
            .ok();
        }));
    }

    /// The Files, Changes and History switch, at the top of the sidebar,
    /// for a folder in a repository.
    pub(super) fn render_sidebar_switch(&self, cx: &mut Context<Self>) -> Option<TabBar> {
        self.changes.repo.as_ref()?;
        const VIEWS: [SidebarView; 3] = [
            SidebarView::Files,
            SidebarView::Changes,
            SidebarView::History,
        ];
        let selected = VIEWS
            .iter()
            .position(|view| *view == self.sidebar_view)
            .unwrap_or_default();
        let tab = |icon: gpui_kit::component::Icon,
                   label: &'static str,
                   action: &'static dyn gpui_kit::Action| {
            Tab::new()
                .icon(icon)
                .aria_label(label)
                .tooltip(move |window, cx| {
                    Tooltip::new(label)
                        .action(action, Some(KEY_CONTEXT))
                        .build(window, cx)
                })
        };
        Some(
            TabBar::new("sidebar-view")
                .segmented()
                .small()
                .child(tab(IconName::Folder.into(), "Files", &ShowFiles))
                .child(tab(AssetIcon::GitCompare.into(), "Changes", &ShowChanges))
                .child(tab(AssetIcon::GitGraph.into(), "History", &ShowHistory))
                .selected_index(selected)
                .on_click(cx.listener(|this, ix: &usize, _, cx| {
                    if let Some(view) = VIEWS.get(*ix) {
                        this.set_sidebar_view(*view, cx);
                    }
                })),
        )
    }

    /// The branch, how far it is from its upstream, and pushing, where the
    /// Files view shows the folder's name. Shared by Changes and History.
    pub(super) fn render_branch_header(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let status = self.changes.status.as_ref()?;
        let muted = cx.theme().muted_foreground;
        let busy = self.changes.operation.is_some();
        let pushing = self.changes.operation == Some(Operation::Pushing);
        let staged = self.changes.staged_count();
        let total = status.changes().len();
        let distance = [(status.ahead(), "↑"), (status.behind(), "↓")]
            .into_iter()
            .filter(|(count, _)| *count > 0)
            .map(|(count, arrow)| format!("{arrow}{}", format_count(count)))
            .collect::<Vec<_>>()
            .join(" ");
        let distance_label = match (status.ahead(), status.behind()) {
            (0, 0) => None,
            (ahead, 0) => Some(format!("{} to push", commits(ahead))),
            (0, behind) => Some(format!("{} to pull", commits(behind))),
            (ahead, behind) => Some(format!("{} to push, {behind} to pull", commits(ahead))),
        };
        let can_stage_all = !busy && staged < total;
        let can_unstage_all = !busy && staged > 0;
        // Staging applies to the Changes list only.
        let staging = self.sidebar_view == SidebarView::Changes;
        let reading = self.changes.status_task.is_some() || self.history.is_reading();
        Some(
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
                        .items_center()
                        .gap_1p5()
                        .min_w_0()
                        .text_sm()
                        .child(Icon::new(AssetIcon::GitBranch).xsmall().text_color(muted))
                        .child(
                            div()
                                .min_w_0()
                                .truncate()
                                .font_weight(FontWeight::SEMIBOLD)
                                .when(status.branch().is_none(), |this| this.text_color(muted))
                                .child(status.branch().unwrap_or("Detached").to_string()),
                        )
                        .when_some(distance_label, |this, label| {
                            this.child(
                                div()
                                    .id("branch-distance")
                                    .flex_none()
                                    .text_xs()
                                    .text_color(muted)
                                    .font_features(FontFeatures(Arc::new(vec![("tnum".into(), 1)])))
                                    .tooltip(move |window, cx| {
                                        Tooltip::new(label.clone()).build(window, cx)
                                    })
                                    .child(distance),
                            )
                        }),
                )
                .when(reading && !busy, |this| {
                    this.child(Spinner::new().xsmall().color(muted))
                })
                .when(status.can_push() || pushing, |this| {
                    let target = status.upstream().map_or_else(
                        || "Push this branch".to_string(),
                        |upstream| format!("Push to {upstream}"),
                    );
                    this.child(
                        Button::new("push")
                            .xsmall()
                            .outline()
                            .icon(IconName::ArrowUp)
                            .label("Push")
                            .tooltip(target)
                            .loading(pushing)
                            .disabled(busy)
                            .on_click(
                                cx.listener(|this, _, window, cx| this.push(&Push, window, cx)),
                            ),
                    )
                })
                .child(
                    Button::new("changes-actions")
                        .small()
                        .ghost()
                        .icon(IconName::Ellipsis)
                        .accessibility_label("Repository actions")
                        .dropdown_menu_with_anchor(Anchor::TopRight, move |menu, _, _| {
                            let menu = if staging {
                                menu.menu_with_disabled(
                                    "Stage All",
                                    Box::new(StageAll),
                                    !can_stage_all,
                                )
                                .menu_with_disabled(
                                    "Unstage All",
                                    Box::new(UnstageAll),
                                    !can_unstage_all,
                                )
                                .separator()
                            } else {
                                menu
                            };
                            menu.menu("Refresh", Box::new(RefreshFolder))
                        }),
                )
                .into_any_element(),
        )
    }

    /// The Changes view below the window controls: the branch, the changed
    /// files, and the commit message.
    pub(super) fn render_changes(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let header = self.render_branch_header(cx);
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        let Some(status) = self.changes.status.as_ref() else {
            let body = match &self.changes.status_error {
                Some(error) => sidebar_empty_state(
                    IconName::TriangleAlert,
                    "Couldn’t read changes",
                    "Git reported a problem with this repository.",
                )
                .child(div().text_xs().text_color(muted).child(error.clone()))
                .into_any_element(),
                None => Empty::new()
                    .flex_none()
                    .justify_start()
                    .px_4()
                    .pt_10()
                    .header(
                        EmptyHeader::new()
                            .media(EmptyMedia::new().child(Spinner::new().small()))
                            .title(EmptyTitle::new().child("Reading changes…")),
                    )
                    .into_any_element(),
            };
            return div()
                .flex()
                .flex_col()
                .flex_1()
                .child(body)
                .into_any_element();
        };

        let busy = self.changes.operation.is_some();
        let staged = self.changes.staged_count();
        let total = status.changes().len();
        let all_staged = total > 0 && staged == total;

        if total == 0 {
            let description = if status.ahead() > 0 {
                "Every change is committed. Push to send the commits to the remote."
            } else {
                "Edits to files in this repository show here, ready to commit."
            };
            return div()
                .flex()
                .flex_col()
                .flex_1()
                .min_h_0()
                .children(header)
                .child(sidebar_empty_state(
                    IconName::Check,
                    "No changes",
                    description,
                ))
                .into_any_element();
        }

        let rem = window.rem_size();
        let rows = self.changes.rows.clone();
        let selected = self.changes.selected.clone();
        let workspace = cx.weak_entity();
        // Virtualized: an unignored build folder can list thousands of files.
        let list = uniform_list("changes-list", rows.len(), move |range, _, cx| {
            range
                .map(|ix| render_row(&rows[ix], selected.as_deref(), busy, rem, &workspace, cx))
                .collect()
        })
        .track_scroll(&self.changes.list_scroll)
        .flex_1()
        .min_h_0()
        .pt_1();

        let theme = cx.theme();
        let count = match staged {
            0 => "Nothing staged".to_string(),
            staged if all_staged => files(staged),
            staged => format!("{staged} of {}", files(total)),
        };
        let root = self.changes.repo.as_ref().map(Repo::root);
        let unsaved = self.tabs.iter().any(|tab| {
            tab.document.is_dirty()
                && tab
                    .document
                    .path()
                    .zip(root)
                    .is_some_and(|(path, root)| path.starts_with(root))
        });
        let committing = self.changes.operation == Some(Operation::Committing);
        let can_commit = self.can_commit();
        let focused = self.changes.message.focus_handle(cx).is_focused(window);
        // One box: the message, then what it commits and the button. The
        // button sits inside the box's corner, so its radius is the box's
        // less the inset between them.
        let composer = div()
            .flex()
            .flex_none()
            .flex_col()
            .gap_1p5()
            .p_2()
            .when(unsaved, |this| {
                this.child(
                    div()
                        .flex()
                        .items_center()
                        .gap_1p5()
                        .px_1()
                        .text_xs()
                        .text_color(muted)
                        .child(
                            div()
                                .flex_none()
                                .size_1p5()
                                .rounded_full()
                                .bg(theme.warning),
                        )
                        .child(
                            div()
                                .min_w_0()
                                .truncate()
                                .child("Unsaved edits won’t be included"),
                        ),
                )
            })
            .child(
                div()
                    .flex()
                    .flex_col()
                    .rounded(theme.radius_lg)
                    .border_1()
                    .border_color(if focused { theme.ring } else { theme.input })
                    .bg(theme.background)
                    .child(
                        div()
                            .id("commit-message")
                            .px_1()
                            .pt_1()
                            // Cmd-Enter commits instead of starting a new line.
                            .capture_action(cx.listener(|this, enter: &Enter, window, cx| {
                                if enter.secondary {
                                    cx.stop_propagation();
                                    this.commit(&Commit, window, cx);
                                }
                            }))
                            .child(
                                Textarea::new(&self.changes.message)
                                    .small()
                                    .appearance(false)
                                    .aria_label("Commit message")
                                    .disabled(committing),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap_2()
                            .p_1()
                            // The count starts where the message's text does.
                            .pl_3()
                            .child(
                                div()
                                    .min_w_0()
                                    .truncate()
                                    .text_xs()
                                    .text_color(muted)
                                    .child(count),
                            )
                            .child(
                                Button::new("commit")
                                    .small()
                                    // Neutral until there is something to
                                    // commit, so the accent marks readiness.
                                    .when(can_commit || committing, |this| this.primary())
                                    .rounded(theme.radius_lg - rem * 0.25)
                                    .label("Commit")
                                    .loading(committing)
                                    .disabled(!can_commit)
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.commit(&Commit, window, cx)
                                    })),
                            ),
                    ),
            );

        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .children(header)
            .child(list)
            .child(composer)
            .into_any_element()
    }

    /// The sheet's header while it shows a diff: the file, how it changed,
    /// and the way back to writing.
    pub(super) fn render_diff_header(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        if self.sidebar_view == SidebarView::History {
            return self.render_commit_header(window, cx);
        }
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        let path = self.changes.selected.clone().unwrap_or_default();
        let change = self.changes.change(&path);
        let name = path.rsplit('/').next().unwrap_or(&path).to_string();
        let (stem, extension) = split_extension(&name);
        let file = self.changes.diff.read(cx).files().first().cloned();
        let shown = self
            .changes
            .shown_patch
            .as_ref()
            .is_some_and(|(shown, _)| *shown == path);
        let counts = file
            .filter(|_| shown)
            .map(|file| (file.additions(), file.deletions()));
        let can_edit = change.is_some_and(|change| change.kind() != ChangeKind::Deleted);
        let tnum = FontFeatures(Arc::new(vec![("tnum".into(), 1)]));

        TitleBar::new()
            .h(window.rem_size() * SHEET_HEADER_REMS)
            .pl_4()
            .pr_2()
            .bg(gpui_kit::transparent_black())
            .border_color(theme.border)
            .child(
                div()
                    .flex()
                    .flex_1()
                    .items_center()
                    .gap_2()
                    .min_w_0()
                    .text_sm()
                    .child(
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
                                this.child(div().flex_none().text_color(muted).child(extension))
                            }),
                    )
                    .when_some(change, |this, change| {
                        let label = match change.original() {
                            Some(original) => format!("Renamed from {original}"),
                            None => change.kind().label().to_string(),
                        };
                        this.child(
                            div()
                                .min_w_0()
                                .truncate()
                                .text_xs()
                                .text_color(muted)
                                .child(label),
                        )
                    })
                    // Signs and colors together: the counts do not rely on
                    // color alone.
                    .when_some(counts, |this, (added, removed)| {
                        this.child(
                            div()
                                .flex()
                                .flex_none()
                                .gap_1p5()
                                .text_xs()
                                .font_features(tnum)
                                .when(added > 0, |this| {
                                    this.child(
                                        div()
                                            .text_color(theme.success)
                                            .child(format!("+{}", format_count(added))),
                                    )
                                })
                                .when(removed > 0, |this| {
                                    this.child(
                                        div()
                                            .text_color(theme.danger)
                                            .child(format!("−{}", format_count(removed))),
                                    )
                                }),
                        )
                    }),
            )
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap_1()
                    .when(can_edit, |this| {
                        this.child(
                            Button::new("edit-change")
                                .xsmall()
                                .outline()
                                .label("Edit")
                                .on_click(
                                    cx.listener(|this, _, window, cx| this.edit_change(window, cx)),
                                ),
                        )
                    })
                    .child(
                        Button::new("close-diff")
                            .small()
                            .ghost()
                            .icon(IconName::Close)
                            .accessibility_label("Close diff")
                            .tooltip("Close diff (Esc)")
                            .on_click(
                                cx.listener(|this, _, window, cx| this.close_diff(window, cx)),
                            ),
                    ),
            )
            .into_any_element()
    }

    /// The diff itself, or why there is none to show.
    pub(super) fn render_diff_body(&self, cx: &mut Context<Self>) -> AnyElement {
        if self.sidebar_view == SidebarView::History {
            return self.render_commit_body(cx);
        }
        let path = self.changes.selected.as_deref().unwrap_or_default();
        let shown = self
            .changes
            .shown_patch
            .as_ref()
            .is_some_and(|(shown, _)| shown == path);
        let message = |icon: IconName, title: &'static str, description: String| {
            Empty::new()
                .size_full()
                .header(
                    EmptyHeader::new()
                        .media(
                            EmptyMedia::new()
                                .with_variant(EmptyMediaVariant::Icon)
                                .child(Icon::new(icon).small()),
                        )
                        .title(EmptyTitle::new().child(title))
                        .description(EmptyDescription::new().child(description)),
                )
                .into_any_element()
        };
        if let Some(error) = &self.changes.diff_error {
            return message(
                IconName::TriangleAlert,
                "Couldn’t show this change",
                error.to_string(),
            );
        }
        if !shown {
            return div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .child(Spinner::new().small().color(cx.theme().muted_foreground))
                .into_any_element();
        }
        let file = self.changes.diff.read(cx).files().first().cloned();
        match file {
            Some(file) if file.is_binary() => message(
                IconName::File,
                "No text to compare",
                "This file isn’t text, so its changes can’t be shown line by line.".into(),
            ),
            Some(file) if file.has_changes() => Diff::new(&self.changes.diff)
                .soft_wrap(true)
                // As in History: signs and word emphasis, no line tint.
                .change_background(false)
                .header_visible(false)
                .hunk_separator(DiffHunkSeparator::LineInfo)
                .change_indicator(DiffChangeIndicator::Signs)
                .size_full()
                .into_any_element(),
            Some(file) if file.status() == DiffFileStatus::Renamed => message(
                IconName::Check,
                "Only the name changed",
                "The text is the same as in the last commit.".into(),
            ),
            Some(file) if file.status() == DiffFileStatus::Added => message(
                IconName::File,
                "Empty file",
                "This new file has no text yet.".into(),
            ),
            Some(file) if file.status() == DiffFileStatus::Deleted => message(
                IconName::File,
                "Empty file",
                "The deleted file had no text.".into(),
            ),
            _ => message(
                IconName::Check,
                "No changes to the text",
                "Only the file’s mode or line endings changed.".into(),
            ),
        }
    }

    /// The sheet's footer while it shows a diff: what it compares with.
    pub(super) fn render_diff_footer(&self, cx: &mut Context<Self>) -> AnyElement {
        if self.sidebar_view == SidebarView::History {
            return self.render_commit_footer(cx);
        }
        let theme = cx.theme();
        let unsaved = self
            .changes
            .selected
            .as_ref()
            .zip(self.changes.repo.as_ref())
            .and_then(|(path, repo)| self.tab_index_of_path(&repo.absolute(path)))
            .is_some_and(|ix| self.tabs[ix].document.is_dirty());
        let has_commits = self
            .changes
            .status
            .as_ref()
            .is_some_and(Status::has_commits);
        let note = if unsaved {
            "Unsaved edits aren’t shown. Save to include them."
        } else if has_commits {
            "Compared with the last commit"
        } else {
            "Nothing committed yet"
        };
        StatusBar::new()
            .bg(gpui_kit::transparent_black())
            .border_color(theme.border)
            .h_7()
            .py_0()
            .px_4()
            .left(div().child(note))
            .into_any_element()
    }
}

fn commits(count: usize) -> String {
    match count {
        1 => "1 commit".into(),
        count => format!("{} commits", format_count(count)),
    }
}

fn files(count: usize) -> String {
    match count {
        1 => "1 file".into(),
        count => format!("{} files", format_count(count)),
    }
}

/// Why a push failed, in terms of what to do next.
fn push_problem(error: &GitError) -> String {
    let text = error.to_string();
    let says = |words: &[&str]| words.iter().any(|word| text.contains(word));
    // Git's own mark for a push the remote is ahead of; a hook's
    // `[remote rejected]` says something else, so its words show as they are.
    if says(&["[rejected]", "(fetch first)", "(non-fast-forward)"]) {
        "The remote has commits this branch doesn’t. Pull them in Terminal, then push again.".into()
    } else if says(&[
        "Authentication failed",
        "could not read Username",
        "Permission denied",
        "terminal prompts disabled",
    ]) {
        "Git couldn’t sign in to the remote. Check your credentials in Terminal, then push again."
            .into()
    } else {
        sentence(&text)
    }
}

/// How much of a file or folder the next commit takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Included {
    None,
    /// Part of a file's edits, or some of a folder's files.
    Some,
    All,
}

/// Git's mark for a change, as the list's status lane shows it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mark {
    Modified,
    Added,
    Untracked,
    Deleted,
    Renamed,
    Conflict,
}

impl Mark {
    fn of(change: &Change) -> Self {
        match change.kind() {
            ChangeKind::Edited => Self::Modified,
            ChangeKind::New if change.tracked() => Self::Added,
            ChangeKind::New => Self::Untracked,
            ChangeKind::Deleted => Self::Deleted,
            ChangeKind::Renamed => Self::Renamed,
            ChangeKind::Conflicted => Self::Conflict,
        }
    }

    /// The letter, what it means, and its color: the letters and colors
    /// most Git tools share, so the column reads at a glance.
    fn appearance(self, cx: &gpui_kit::App) -> (&'static str, &'static str, gpui_kit::Hsla) {
        let theme = cx.theme();
        match self {
            Self::Modified => ("M", "Modified", theme.warning),
            Self::Added => ("A", "Added", theme.success),
            Self::Untracked => ("U", "Untracked", theme.success),
            Self::Deleted => ("D", "Deleted", theme.danger),
            Self::Renamed => ("R", "Renamed", theme.info),
            Self::Conflict => ("C", "Conflict", theme.danger),
        }
    }
}

/// One row of the Changes list, resolved when the status is read.
#[derive(Clone, Debug)]
enum ListRow {
    Folder {
        path: SharedString,
        label: SharedString,
        depth: usize,
        included: Included,
        expanded: bool,
    },
    File {
        path: SharedString,
        name: SharedString,
        depth: usize,
        included: Included,
        mark: Mark,
    },
}

/// A row of the Changes list: the Files view's tree, on its leading edge,
/// with Git's letter and the staging ring in fixed lanes at the end.
fn render_row(
    row: &ListRow,
    selected: Option<&str>,
    busy: bool,
    rem: gpui_kit::Pixels,
    workspace: &gpui_kit::WeakEntity<Workspace>,
    cx: &mut gpui_kit::App,
) -> AnyElement {
    let theme = cx.theme();
    let muted = theme.muted_foreground;
    let guide = theme.sidebar_border;
    // One slot per enclosing folder, with a hairline under that folder's
    // chevron.
    let guides = |depth: usize| {
        (0..depth).map(move |_| {
            div()
                .flex_none()
                .w(rem * 1.25)
                .h_7()
                .pl(rem * 0.5)
                .child(div().w(gpui_kit::px(1.)).h_full().bg(guide))
        })
    };
    let item = |id: String| {
        ListItem::new(SharedString::from(id))
            .h_7()
            .px_0()
            .py_0()
            .pl(rem * 0.25)
            // The ring's center lines up with the ⋯ button above it.
            .pr(rem * 0.125)
            .rounded(theme.radius)
            .text_sm()
    };
    let content = || div().flex().items_center().gap_1().w_full().min_w_0();
    let element = match row {
        ListRow::Folder {
            path,
            label,
            depth,
            included,
            expanded,
        } => {
            let included = *included;
            let toggle = path.clone();
            let workspace_for_click = workspace.clone();
            item(format!("folder-{path}"))
                .accessibility_label(path.clone())
                .on_click(move |_, _, cx| {
                    workspace_for_click
                        .update(cx, |this, cx| this.toggle_folder(&toggle, cx))
                        .ok();
                })
                .child(
                    content()
                        .children(guides(*depth))
                        .child(
                            super::icon_slot().text_color(muted).child(
                                Icon::new(if *expanded {
                                    IconName::ChevronDown
                                } else {
                                    IconName::ChevronRight
                                })
                                .xsmall(),
                            ),
                        )
                        .child(div().flex_1().min_w_0().truncate().child(label.clone()))
                        .child(stage_ring(
                            format!("stage-folder-{path}"),
                            included,
                            format!("everything in {path}"),
                            busy,
                            rem,
                            workspace,
                            {
                                let path = path.clone();
                                move |this, window, cx| {
                                    let stage = included != Included::All;
                                    this.stage_folder(&path, stage, window, cx);
                                }
                            },
                            cx,
                        )),
                )
        }
        ListRow::File {
            path,
            name,
            depth,
            included,
            mark,
        } => {
            let included = *included;
            let is_selected = selected == Some(path.as_ref());
            let (letter, meaning, color) = mark.appearance(cx);
            let select = path.to_string();
            let workspace_for_click = workspace.clone();
            item(format!("change-{path}"))
                .selected(is_selected)
                .accessibility_label(path.clone())
                .on_click(move |_, _, cx| {
                    let select = select.clone();
                    workspace_for_click
                        .update(cx, |this, cx| this.select_change(Some(select), cx))
                        .ok();
                })
                .child(
                    content()
                        .children(guides(*depth))
                        // Names line up with folder names beside them.
                        .child(super::icon_slot())
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .when(is_selected, |this| this.font_weight(FontWeight::MEDIUM))
                                .when(*mark == Mark::Deleted, |this| {
                                    this.line_through().text_color(muted)
                                })
                                .child(name.clone()),
                        )
                        // Git's own letter, in a fixed lane; the tooltip
                        // says it in words.
                        .child(
                            div()
                                .id(SharedString::from(format!("status-{path}")))
                                .flex_none()
                                .w(rem)
                                .text_center()
                                .text_xs()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(color)
                                .tooltip(move |window, cx| Tooltip::new(meaning).build(window, cx))
                                .child(letter),
                        )
                        .child(stage_ring(
                            format!("stage-{path}"),
                            included,
                            path.to_string(),
                            busy,
                            rem,
                            workspace,
                            {
                                let path = path.to_string();
                                move |this, window, cx| {
                                    // A partly staged file takes the rest.
                                    let stage = included != Included::All;
                                    this.set_staged(vec![path.clone()], stage, window, cx);
                                }
                            },
                            cx,
                        )),
                )
        }
    };
    // Each row is laid out at the list's width, so the inset is padding
    // around the row, not its margin.
    div().px_2().child(element).into_any_element()
}

/// The ring at a row's end that stages and unstages it: empty, filled with
/// a check, or filled with a dash for part. In the text's color, not the
/// accent, so a list of staged files stays calm beside the Commit button.
#[allow(clippy::too_many_arguments)]
fn stage_ring(
    id: String,
    included: Included,
    object: String,
    busy: bool,
    rem: gpui_kit::Pixels,
    workspace: &gpui_kit::WeakEntity<Workspace>,
    toggle: impl Fn(&mut Workspace, &mut Window, &mut Context<Workspace>) + 'static,
    cx: &gpui_kit::App,
) -> Button {
    let theme = cx.theme();
    let ring = div()
        .size(rem * 0.875)
        .flex()
        .items_center()
        .justify_center()
        .rounded_full()
        .map(|this| match included {
            Included::None => this.border_1().border_color(theme.muted_foreground),
            Included::Some => this.bg(theme.muted_foreground).child(
                Icon::new(IconName::Minus)
                    .size(rem * 0.625)
                    .text_color(theme.background),
            ),
            Included::All => this.bg(theme.sidebar_foreground).child(
                Icon::new(IconName::Check)
                    .size(rem * 0.625)
                    .text_color(theme.sidebar),
            ),
        });
    let (verb, tooltip) = match included {
        Included::None => ("Stage", "Stage"),
        Included::Some => ("Stage", "Partly staged. Stage the rest"),
        Included::All => ("Unstage", "Unstage"),
    };
    let workspace = workspace.clone();
    Button::new(SharedString::from(id))
        .xsmall()
        .ghost()
        .child(ring)
        .accessibility_label(format!("{verb} {object}"))
        .tooltip(tooltip)
        .disabled(busy)
        .on_click(move |_, window, cx| {
            // The row's own click selects or folds it; this one stages.
            cx.stop_propagation();
            workspace
                .update(cx, |this, cx| toggle(this, window, cx))
                .ok();
        })
}
