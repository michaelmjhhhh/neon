//! The sidebar's Search view: where text occurs in the open folder's
//! Markdown files, grouped by file. Picking a match opens its file with the
//! match selected; Return and the arrow keys in the field step through them.
//!
//! A search runs off the main thread as typing pauses, and shows its results
//! batch by batch while it reads; a newer query cancels it. How files are
//! read and matched is in [`crate::folder_search`].

use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use gpui_kit::assets::IconName as AssetIcon;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Escape, Input, InputEvent, InputState, RopeExt as _};
use gpui_kit::component::list::ListItem;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, IconName, Selectable as _, Sizable as _,
};
use gpui_kit::{
    AnyElement, AppContext as _, Context, Entity, Focusable as _, FontFeatures, FontWeight,
    HighlightStyle, InteractiveElement as _, IntoElement, ParentElement as _, ScrollStrategy,
    SharedString, Styled as _, StyledText, Subscription, Task, UniformListScrollHandle, Window,
    div, prelude::FluentBuilder as _, uniform_list,
};

use super::{
    NewTab, OpenFolder, SidebarView, Workspace, ZenView, file_name, format_count, icon_slot,
    sentence, sidebar_empty_state, split_extension,
};
use crate::KEY_CONTEXT;
use crate::folder::markdown_files;
use crate::folder_search::{
    FileMatches, LineMatch, MAX_MATCHES, Query, SearchOptions, locate, search_files,
};

gpui_kit::actions!(
    workspace,
    [ShowSearch, SelectNextMatch, SelectPreviousMatch]
);

/// The search field, whose Return and arrow keys step through the matches.
pub(crate) const FIELD_KEY_CONTEXT: &str = "SearchField";

/// How long typing pauses before the query runs, so each keystroke does
/// not start a search of its own.
const DEBOUNCE: Duration = Duration::from_millis(150);
/// How many files the first batch reads. Each batch reads twice as many as
/// the one before, up to [`MAX_BATCH`]: the first results show soon, and a
/// large folder still takes few trips to the main thread.
const FIRST_BATCH: usize = 32;
const MAX_BATCH: usize = 1024;

/// A match by what it is, not by its place in the list, so it stays picked
/// when the list is read again after a save.
#[derive(Clone, Debug, PartialEq, Eq)]
struct MatchId {
    path: PathBuf,
    line: usize,
    columns: Range<usize>,
}

impl MatchId {
    fn new(path: &Path, found: &LineMatch) -> Self {
        Self {
            path: path.to_path_buf(),
            line: found.line,
            columns: found.columns.clone(),
        }
    }
}

/// One row of the list: a file, or one of its matches, by index into the
/// results.
#[derive(Clone, Copy, Debug)]
enum Row {
    File { file_ix: usize, expanded: bool },
    Match(usize, usize),
}

/// The Search view's state.
pub(super) struct Search {
    query: Entity<InputState>,
    options: SearchOptions,
    /// The query the results are for. A match picked in a document edited
    /// since is found again on its line with it.
    ran: Option<Query>,
    /// Why the query cannot run, such as an invalid regex.
    problem: Option<SharedString>,
    results: Rc<Vec<FileMatches>>,
    match_count: usize,
    /// Whether matches beyond [`MAX_MATCHES`] were left out.
    truncated: bool,
    task: Option<Task<()>>,
    /// The list is for another query or other options: typed or toggled
    /// since, and the new results not in yet.
    stale: bool,
    /// The search waits for typing to pause.
    debouncing: bool,
    /// The folder changed while the view was hidden; it searches again
    /// once shown.
    outdated: bool,
    /// Set to stop the search reading in the background once another
    /// replaces it.
    cancelled: Arc<AtomicBool>,
    /// A step through the matches asked for before the query ran; taken
    /// once the first matches arrive.
    pending_step: Option<bool>,
    /// Files shown closed, by path.
    collapsed: HashSet<PathBuf>,
    rows: Rc<Vec<Row>>,
    selected: Option<MatchId>,
    /// A match picked in a file still being read; selected once it shows.
    pending_reveal: Option<MatchId>,
    list_scroll: UniformListScrollHandle,
    _subscription: Subscription,
}

impl Search {
    pub(super) fn new(window: &mut Window, cx: &mut Context<Workspace>) -> Self {
        let query = cx.new(|cx| InputState::new(window, cx).placeholder("Search"));
        // An IME composition, such as pinyin not yet turned into
        // characters, sends no Change, so only committed text is searched.
        let subscription =
            cx.subscribe_in(&query, window, |this, _, event: &InputEvent, window, cx| {
                if matches!(event, InputEvent::Change) {
                    this.search.stale = true;
                    this.run_search(Some(DEBOUNCE), window, cx);
                }
            });
        Self {
            query,
            options: SearchOptions::default(),
            ran: None,
            problem: None,
            results: Rc::default(),
            match_count: 0,
            truncated: false,
            task: None,
            stale: false,
            debouncing: false,
            outdated: false,
            cancelled: Arc::default(),
            pending_step: None,
            collapsed: HashSet::new(),
            rows: Rc::default(),
            selected: None,
            pending_reveal: None,
            list_scroll: UniformListScrollHandle::new(),
            _subscription: subscription,
        }
    }

    fn clear_results(&mut self) {
        self.ran = None;
        self.results = Rc::default();
        self.match_count = 0;
        self.truncated = false;
        self.pending_step = None;
        self.rows = Rc::default();
    }

    /// Add a batch of results, in place of the last search's for the first
    /// batch of a new one. The last search's stay until then, so the list
    /// does not flash empty as each key is typed.
    fn add_results(&mut self, batch: Vec<FileMatches>, replace: bool) {
        let results = Rc::make_mut(&mut self.results);
        if replace {
            results.clear();
            self.match_count = 0;
            self.truncated = false;
        }
        for mut file in batch {
            let room = MAX_MATCHES - self.match_count;
            if file.matches.len() > room {
                file.matches.truncate(room);
                self.truncated = true;
            }
            if file.matches.is_empty() {
                break;
            }
            self.match_count += file.matches.len();
            results.push(file);
        }
        self.rebuild_rows();
    }

    fn rebuild_rows(&mut self) {
        let mut rows = Vec::with_capacity(self.results.len() + self.match_count);
        for (file_ix, file) in self.results.iter().enumerate() {
            let expanded = !self.collapsed.contains(&file.path);
            rows.push(Row::File { file_ix, expanded });
            if expanded {
                rows.extend((0..file.matches.len()).map(|ix| Row::Match(file_ix, ix)));
            }
        }
        self.rows = Rc::new(rows);
    }

    /// The selected match's place in the results.
    fn selected_index(&self) -> Option<(usize, usize)> {
        let selected = self.selected.as_ref()?;
        let file_ix = self
            .results
            .iter()
            .position(|file| file.path == selected.path)?;
        let match_ix = self.results[file_ix]
            .matches
            .iter()
            .position(|found| found.line == selected.line && found.columns == selected.columns)?;
        Some((file_ix, match_ix))
    }

    /// The match `forward` or back from the selected one, wrapping around;
    /// the first or last when none is selected.
    fn step(&self, forward: bool) -> Option<(usize, usize)> {
        let total = self.results.len();
        if total == 0 {
            return None;
        }
        let Some((file_ix, match_ix)) = self.selected_index() else {
            return Some(if forward {
                (0, 0)
            } else {
                (total - 1, self.results[total - 1].matches.len() - 1)
            });
        };
        let count = self.results[file_ix].matches.len();
        Some(match (forward, match_ix) {
            (true, ix) if ix + 1 < count => (file_ix, ix + 1),
            (true, _) => ((file_ix + 1) % total, 0),
            (false, 0) => {
                let file_ix = (file_ix + total - 1) % total;
                (file_ix, self.results[file_ix].matches.len() - 1)
            }
            (false, ix) => (file_ix, ix - 1),
        })
    }

    fn row_of(&self, file_ix: usize, match_ix: usize) -> Option<usize> {
        self.rows
            .iter()
            .position(|row| matches!(row, Row::Match(f, m) if *f == file_ix && *m == match_ix))
    }
}

impl Workspace {
    /// Show the Search view with its field focused. The editor's selection,
    /// on one line, becomes the query, as in Find.
    pub(super) fn show_search(
        &mut self,
        _: &ShowSearch,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.set_sidebar_view(SidebarView::Search, window, cx);
        let query = self.search.query.clone();
        // Only a selection on screen: a hidden editor's is left over.
        let editor_shown = self
            .zen_view()
            .map_or(self.layout.shows_editor(), |view| view == ZenView::Editor);
        let selection = editor_shown
            .then(|| self.tab().editor.read(cx).selected_text().to_string())
            .filter(|text| !text.trim().is_empty() && !text.contains('\n'))
            .map(|text| {
                if self.search.options.regex {
                    regex::escape(&text)
                } else {
                    text
                }
            });
        if let Some(text) = selection
            && *query.read(cx).value() != *text
        {
            query.update(cx, |query, cx| query.set_value(text, window, cx));
            self.search.stale = true;
            self.run_search(None, window, cx);
        }
        query.update(cx, |query, cx| {
            query.focus(window, cx);
            query.select_all(window, cx);
        });
    }

    /// Search the folder again, as after a save or a refresh, unless nothing
    /// is being searched for.
    pub(super) fn refresh_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.search.query.read(cx).value().is_empty() {
            return;
        }
        // Hidden, the list waits: a save would otherwise read every file
        // for results nobody sees.
        if self.search_shown() {
            self.run_search(None, window, cx);
        } else {
            self.search.outdated = true;
        }
    }

    /// Search again if the folder changed while the view was hidden.
    pub(super) fn catch_up_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.search.outdated && self.search_shown() {
            self.run_search(None, window, cx);
        }
    }

    fn search_shown(&self) -> bool {
        self.sidebar_visible && self.sidebar_view == SidebarView::Search && self.zen.is_none()
    }

    /// Run the query, after `delay` if given; a newer run replaces it.
    fn run_search(&mut self, delay: Option<Duration>, window: &mut Window, cx: &mut Context<Self>) {
        let search = &mut self.search;
        search.cancelled.store(true, Ordering::Relaxed);
        search.cancelled = Arc::default();
        search.outdated = false;
        search.debouncing = delay.is_some();
        let text = search.query.read(cx).value().to_string();
        let query = match Query::new(&text, search.options) {
            Ok(query) => query,
            Err(problem) => {
                search.problem = Some(problem.into());
                search.clear_results();
                search.task = None;
                search.stale = false;
                cx.notify();
                return;
            }
        };
        search.problem = None;
        let (Some(query), Some(folder)) = (query, self.folder.clone()) else {
            search.clear_results();
            search.task = None;
            search.stale = false;
            cx.notify();
            return;
        };
        // Open documents with unsaved edits are searched as they read now.
        let unsaved: HashMap<PathBuf, String> = self
            .tabs
            .iter()
            .filter(|tab| tab.document.is_dirty())
            .filter_map(|tab| {
                let path = tab.document.path()?.to_path_buf();
                Some((path, tab.editor.read(cx).text().to_string()))
            })
            .collect();
        let unsaved = Arc::new(unsaved);
        let cancelled = self.search.cancelled.clone();
        self.search.task = Some(cx.spawn_in(window, async move |this, cx| {
            if let Some(delay) = delay {
                cx.background_executor().timer(delay).await;
                this.update(cx, |this, _| this.search.debouncing = false)
                    .ok();
            }
            let listing = cx
                .background_spawn(async move { markdown_files(&folder) })
                .await;
            let files = match listing {
                Ok(files) => Arc::new(files),
                Err(error) => {
                    this.update(cx, |this, cx| {
                        let message = sentence(&error.to_string());
                        this.search.problem =
                            Some(format!("Couldn’t read the folder. {message}").into());
                        this.search.clear_results();
                        this.search.task = None;
                        this.search.stale = false;
                        cx.notify();
                    })
                    .ok();
                    return;
                }
            };
            let query = Arc::new(query);
            let (mut start, mut batch, mut found) = (0, FIRST_BATCH, 0);
            loop {
                let end = (start + batch).min(files.len());
                let limit = MAX_MATCHES - found;
                let results = cx
                    .background_spawn({
                        let (query, files) = (query.clone(), files.clone());
                        let (unsaved, cancelled) = (unsaved.clone(), cancelled.clone());
                        async move {
                            search_files(&query, &files[start..end], &unsaved, limit, &cancelled)
                        }
                    })
                    .await;
                let landed = this.update_in(cx, |this, window, cx| {
                    this.land_search_results(results, start == 0, &query, window, cx)
                });
                let Ok((total, truncated)) = landed else {
                    return;
                };
                found = total;
                start = end;
                batch = (batch * 2).min(MAX_BATCH);
                if start >= files.len() || truncated {
                    break;
                }
            }
            this.update(cx, |this, cx| {
                let search = &mut this.search;
                search.task = None;
                search.pending_step = None;
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }
}

impl Workspace {
    /// Add a batch of results, in place of the last search's for the first
    /// batch, and take a step asked for before they came. Returns how many
    /// matches the list holds now, and whether it holds all there are.
    fn land_search_results(
        &mut self,
        results: Vec<FileMatches>,
        replace: bool,
        query: &Query,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> (usize, bool) {
        let search = &mut self.search;
        if replace {
            search.ran = Some(query.clone());
            search.stale = false;
        }
        let changed = replace || !results.is_empty();
        search.add_results(results, replace);
        if search.match_count > 0
            && let Some(forward) = search.pending_step.take()
            && let Some((file_ix, match_ix)) = search.step(forward)
        {
            self.pick_search_match(file_ix, match_ix, window, cx);
        }
        if changed {
            cx.notify();
        }
        (self.search.match_count, self.search.truncated)
    }

    /// Pick the next match, or the previous one, from the keyboard. Asked
    /// for while the query is still to run, the step waits for its results.
    fn step_search_match(&mut self, forward: bool, window: &mut Window, cx: &mut Context<Self>) {
        let search = &mut self.search;
        // The list is for an older query, or still empty: step once the
        // query's matches arrive. A query still waiting for typing to pause
        // runs at once.
        if search.stale || (search.task.is_some() && search.match_count == 0) {
            search.pending_step = Some(forward);
            if search.stale && (search.task.is_none() || search.debouncing) {
                self.run_search(None, window, cx);
            }
            return;
        }
        if let Some((file_ix, match_ix)) = self.search.step(forward) {
            self.pick_search_match(file_ix, match_ix, window, cx);
        }
    }

    /// Select a match in the list and show it in its document. A document
    /// with unsaved edits stays open in its tab and the match opens in a
    /// new one, as a linked note does.
    fn pick_search_match(
        &mut self,
        file_ix: usize,
        match_ix: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(found) = self
            .search
            .results
            .get(file_ix)
            .and_then(|file| Some(MatchId::new(&file.path, file.matches.get(match_ix)?)))
        else {
            return;
        };
        self.search.selected = Some(found.clone());
        if self.search.collapsed.remove(&found.path) {
            self.search.rebuild_rows();
        }
        if let Some(row) = self.search.row_of(file_ix, match_ix) {
            self.search
                .list_scroll
                .scroll_to_item(row, ScrollStrategy::Nearest);
        }
        cx.notify();
        self.open_search_match(found, window, cx);
    }

    fn open_search_match(&mut self, found: MatchId, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_busy() {
            return;
        }
        // Showing another tab focuses its editor; stepping from the field
        // keeps the field focused, so the next Return steps again instead
        // of typing into the document.
        let field = self.search.query.focus_handle(cx);
        let from_field = field.is_focused(window);
        self.search.pending_reveal = None;
        if self.tab().document.path() == Some(found.path.as_path()) {
            self.reveal_search_match(&found, window, cx);
        } else if let Some(ix) = self.tab_index_of_path(&found.path) {
            self.activate_tab(ix, window, cx);
            self.reveal_search_match(&found, window, cx);
        } else {
            self.search.pending_reveal = Some(found.clone());
            if self.tab().document.is_dirty() {
                self.new_tab(&NewTab, window, cx);
            }
            self.open_path(found.path, window, cx);
        }
        if from_field {
            field.focus(window, cx);
        }
    }

    /// Select a match picked while its document was being read, now that
    /// the document shows.
    pub(super) fn reveal_pending_search_match(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(found) = self.search.pending_reveal.take() else {
            return;
        };
        if self.tab().document.path() == Some(found.path.as_path()) {
            self.reveal_search_match(&found, window, cx);
        }
    }

    /// Select the match in the active document and scroll to it. If the
    /// document changed since the search, the match is looked for again on
    /// its line, nearest where it was.
    fn reveal_search_match(
        &mut self,
        found: &MatchId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let query = self.search.ran.clone();
        let field = self.search.query.focus_handle(cx);
        let editor = self.tab().editor.clone();
        let range = editor.update(cx, |editor, cx| {
            let text = editor.text();
            if found.line >= text.lines_len() {
                return None;
            }
            let start = text.line_start_offset(found.line);
            let line = text.slice_line(found.line).to_string();
            let line = line.strip_suffix('\r').unwrap_or(&line);
            let columns = query
                .and_then(|query| locate(&query, line, found.columns.start))
                .unwrap_or_else(|| {
                    found.columns.start.min(line.len())..found.columns.end.min(line.len())
                });
            let range = start + columns.start..start + columns.end;
            // Stepping through matches keeps the focus in the field; the
            // selection shows meanwhile all the same.
            editor.set_selection_focus(Some(field), cx);
            editor.set_selected_range(range.clone(), cx);
            Some(range)
        });
        let Some(range) = range else {
            return;
        };
        if !self.layout.shows_editor() {
            self.tab()
                .preview
                .update(cx, |preview, cx| preview.reveal_offset(range.start, cx));
        }
        // A document just read has no layout yet to scroll by, so the
        // selection is made again once it has one.
        cx.on_next_frame(window, move |_, _, cx| {
            editor.update(cx, |editor, cx| {
                if editor.selected_range() == range {
                    editor.set_selected_range(range, cx);
                }
            });
        });
    }

    fn toggle_search_file(&mut self, path: &Path, cx: &mut Context<Self>) {
        if !self.search.collapsed.remove(path) {
            self.search.collapsed.insert(path.to_path_buf());
        }
        self.search.rebuild_rows();
        cx.notify();
    }

    fn set_search_options(
        &mut self,
        options: SearchOptions,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.search.options = options;
        self.search.stale = true;
        self.run_search(None, window, cx);
    }

    /// The Search view below the window controls: the field and its
    /// options, what the search found, and the matches by file.
    pub(super) fn render_search(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(folder) = self.folder.as_deref() else {
            let can_open_folder = self.folder_task.is_none() && !self.is_busy();
            return sidebar_empty_state(
                IconName::Search,
                "No folder open",
                "Open a folder to search its Markdown files.",
            )
            .child(
                Button::new("open-folder-search")
                    .small()
                    .outline()
                    .icon(IconName::FolderOpen)
                    .label("Open Folder…")
                    .tooltip_with_action("Open folder…", &OpenFolder, Some(KEY_CONTEXT))
                    .disabled(!can_open_folder)
                    .on_click(
                        cx.listener(|this, _, window, cx| {
                            this.open_folder(&OpenFolder, window, cx)
                        }),
                    ),
            )
            .into_any_element();
        };
        let search = &self.search;
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        let options = search.options;
        let option = |id: &'static str,
                      icon: Icon,
                      label: &'static str,
                      on: bool,
                      toggled: SearchOptions| {
            Button::new(id)
                .xsmall()
                .ghost()
                .icon(icon)
                .selected(on)
                .accessibility_label(label)
                .tooltip(label)
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.set_search_options(toggled, window, cx)
                }))
        };
        let field = div()
            .id("search-field")
            .key_context(FIELD_KEY_CONTEXT)
            .flex_none()
            .mx_2()
            .on_action(cx.listener(|this, _: &SelectNextMatch, window, cx| {
                this.step_search_match(true, window, cx);
            }))
            .on_action(cx.listener(|this, _: &SelectPreviousMatch, window, cx| {
                this.step_search_match(false, window, cx);
            }))
            .on_action(cx.listener(|this, _: &Escape, window, cx| {
                this.focus_content(window, cx);
            }))
            .child(
                Input::new(&search.query)
                    .small()
                    .cleanable(true)
                    .aria_label("Search in folder")
                    .prefix(Icon::new(IconName::Search).xsmall().text_color(muted))
                    .suffix(
                        div()
                            .flex()
                            .items_center()
                            .child(option(
                                "search-match-case",
                                IconName::CaseSensitive.into(),
                                "Match case",
                                options.match_case,
                                SearchOptions {
                                    match_case: !options.match_case,
                                    ..options
                                },
                            ))
                            .child(option(
                                "search-whole-word",
                                AssetIcon::WholeWord.into(),
                                "Match whole word",
                                options.whole_word,
                                SearchOptions {
                                    whole_word: !options.whole_word,
                                    ..options
                                },
                            ))
                            .child(option(
                                "search-regex",
                                AssetIcon::Regex.into(),
                                "Use regular expression",
                                options.regex,
                                SearchOptions {
                                    regex: !options.regex,
                                    ..options
                                },
                            )),
                    ),
            );

        let searching = search.task.is_some() || search.stale;
        let text = search.query.read(cx).value();
        let empty_query = text.is_empty();
        let summary: SharedString = if let Some(problem) = &search.problem {
            problem.clone()
        } else if empty_query {
            "Find text in every Markdown file here.".into()
        } else if search.match_count == 0 {
            if searching {
                "Searching…".into()
            } else {
                format!("No results for “{text}”.").into()
            }
        } else {
            let results = match search.match_count {
                1 => "1 result".to_string(),
                count if search.truncated => format!("First {} results", format_count(count)),
                count => format!("{} results", format_count(count)),
            };
            let files = match search.results.len() {
                1 => "1 file".to_string(),
                count => format!("{} files", format_count(count)),
            };
            format!("{results} in {files}").into()
        };
        let theme = cx.theme();
        let tnum = FontFeatures(Arc::new(vec![("tnum".into(), 1)]));
        // Where the picked match is among them all, as Find counts.
        let position = search.selected_index().map(|(file_ix, match_ix)| {
            let before: usize = search.results[..file_ix]
                .iter()
                .map(|file| file.matches.len())
                .sum();
            format!(
                "{} of {}",
                format_count(before + match_ix + 1),
                format_count(search.match_count)
            )
        });
        let has_matches = search.match_count > 0;
        let step = |id: &'static str, icon: IconName, label: &'static str, forward: bool| {
            let action: &dyn gpui_kit::Action = if forward {
                &SelectNextMatch
            } else {
                &SelectPreviousMatch
            };
            Button::new(id)
                .xsmall()
                .ghost()
                .icon(icon)
                .accessibility_label(label)
                .tooltip_with_action(label, action, Some(FIELD_KEY_CONTEXT))
        };
        let status = div()
            .flex()
            .flex_none()
            .items_center()
            .gap_1()
            .h_7()
            .mx_2()
            .pl_1()
            .text_xs()
            .text_color(if search.problem.is_some() {
                theme.danger
            } else {
                muted
            })
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    // The count fits on the line beside the arrows; a hint
                    // or a problem wraps rather than lose its end.
                    .when(has_matches, |this| this.truncate())
                    .child(summary),
            )
            .when(searching && !empty_query, |this| {
                this.child(Spinner::new().xsmall().color(muted))
            })
            .when(has_matches, |this| {
                this.when_some(position, |this, position| {
                    this.child(div().flex_none().font_features(tnum).child(position))
                })
                .child(
                    step(
                        "previous-match",
                        IconName::ChevronUp,
                        "Previous match",
                        false,
                    )
                    .on_click(
                        cx.listener(|this, _, window, cx| {
                            this.step_search_match(false, window, cx)
                        }),
                    ),
                )
                .child(
                    step("next-match", IconName::ChevronDown, "Next match", true).on_click(
                        cx.listener(|this, _, window, cx| this.step_search_match(true, window, cx)),
                    ),
                )
            });

        let rem = window.rem_size();
        let rows = search.rows.clone();
        let results = search.results.clone();
        let selected = search.selected.clone();
        let folder = Arc::<Path>::from(folder);
        let workspace = cx.weak_entity();
        // Virtualized: a common word can match thousands of lines.
        let list = uniform_list("search-results", rows.len(), move |range, _, cx| {
            range
                .map(|ix| {
                    render_row(
                        rows[ix],
                        &results,
                        selected.as_ref(),
                        &folder,
                        rem,
                        &workspace,
                        cx,
                    )
                })
                .collect()
        })
        .track_scroll(&search.list_scroll)
        .flex_1()
        .min_h_0();

        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .pt_1()
            .child(field)
            .child(status)
            .child(list)
            .into_any_element()
    }
}

/// A row of the results: a file with its folder and how many matches it
/// has, or a match's line with the match marked.
fn render_row(
    row: Row,
    results: &[FileMatches],
    selected: Option<&MatchId>,
    folder: &Path,
    rem: gpui_kit::Pixels,
    workspace: &gpui_kit::WeakEntity<Workspace>,
    cx: &mut gpui_kit::App,
) -> AnyElement {
    let theme = cx.theme();
    let muted = theme.muted_foreground;
    let item = |id: String| {
        ListItem::new(SharedString::from(id))
            .h_7()
            .px_0()
            .py_0()
            .pl(rem * 0.25)
            .pr(rem * 0.5)
            .rounded(theme.radius)
            .text_sm()
    };
    let content = || div().flex().items_center().gap_1().w_full().min_w_0();
    let element = match row {
        Row::File { file_ix, expanded } => {
            let file = &results[file_ix];
            let path = file.path.clone();
            let id = path.to_string_lossy().into_owned();
            let (stem, extension) = split_extension(&file_name(&path));
            let directory = path
                .parent()
                .and_then(|parent| parent.strip_prefix(folder).ok())
                .filter(|relative| !relative.as_os_str().is_empty())
                .map(|relative| relative.to_string_lossy().into_owned());
            let count = format_count(file.matches.len());
            let workspace = workspace.clone();
            let tnum = FontFeatures(Arc::new(vec![("tnum".into(), 1)]));
            item(format!("search-file-{id}"))
                .accessibility_label(format!("{id}, {count}"))
                .on_click(move |_, _, cx| {
                    workspace
                        .update(cx, |this, cx| this.toggle_search_file(&path, cx))
                        .ok();
                })
                .child(
                    content()
                        .child(
                            icon_slot().text_color(muted).child(
                                Icon::new(if expanded {
                                    IconName::ChevronDown
                                } else {
                                    IconName::ChevronRight
                                })
                                .xsmall(),
                            ),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_1()
                                .items_baseline()
                                .gap_1p5()
                                .min_w_0()
                                .child(
                                    div()
                                        .flex()
                                        .flex_shrink_0()
                                        .max_w_full()
                                        .min_w_0()
                                        .child(
                                            div()
                                                .min_w_0()
                                                .truncate()
                                                .font_weight(FontWeight::MEDIUM)
                                                .child(stem),
                                        )
                                        .when_some(extension, |this, extension| {
                                            this.child(
                                                div()
                                                    .flex_none()
                                                    .text_color(muted)
                                                    .child(extension),
                                            )
                                        }),
                                )
                                .when_some(directory, |this, directory| {
                                    this.child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .truncate()
                                            .text_xs()
                                            .text_color(muted)
                                            .child(directory),
                                    )
                                }),
                        )
                        .child(
                            div()
                                .flex_none()
                                .text_xs()
                                .text_color(muted)
                                .font_features(tnum)
                                .child(count),
                        ),
                )
        }
        Row::Match(file_ix, match_ix) => {
            let file = &results[file_ix];
            let found = &file.matches[match_ix];
            let id = MatchId::new(&file.path, found);
            let is_selected = selected == Some(&id);
            let label = format!("Line {}: {}", found.line + 1, found.preview);
            // The one accent in the list: the match, lit in the tint the
            // editor selects it with, and set heavier so it reads at a glance.
            let highlight = HighlightStyle {
                background_color: Some(theme.selection),
                font_weight: Some(FontWeight::SEMIBOLD),
                ..Default::default()
            };
            let preview = StyledText::new(found.preview.clone())
                .with_highlights([(found.highlight.clone(), highlight)]);
            let workspace = workspace.clone();
            item(format!(
                "search-match-{}-{}-{}",
                file.path.display(),
                found.line,
                found.columns.start
            ))
            .selected(is_selected)
            .accessibility_label(label)
            .on_click(move |_, window, cx| {
                workspace
                    .update(cx, |this, cx| {
                        this.pick_search_match(file_ix, match_ix, window, cx)
                    })
                    .ok();
            })
            .child(
                content()
                    // The line starts under the file's name.
                    .child(icon_slot())
                    .child(div().flex_1().min_w_0().truncate().child(preview)),
            )
        }
    };
    div().px_2().child(element).into_any_element()
}
