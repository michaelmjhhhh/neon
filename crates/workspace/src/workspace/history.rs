//! The sidebar's History view: the commits of every branch as a graph, the
//! newest first. Each branch runs down its own lane; merges and forks bend
//! between lanes. A commit picked in the list shows what it changed in the
//! sheet, as a change picked in the Changes view does.

use std::rc::Rc;
use std::time::{SystemTime, UNIX_EPOCH};

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::diff::{
    Diff, DiffChangeIndicator, DiffFile, DiffHunkSeparator, DiffState,
};
use gpui_kit::component::empty::{Empty, EmptyHeader, EmptyMedia, EmptyTitle};
use gpui_kit::component::list::ListItem;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::status_bar::StatusBar;
use gpui_kit::component::{ActiveTheme as _, IconName, Sizable as _, TitleBar};
use gpui_kit::{
    AnyElement, AppContext as _, BorderStyle, Bounds, Context, Entity, FontFeatures, FontWeight,
    Hsla, IntoElement, ParentElement as _, PathBuilder, Pixels, SharedString, Styled as _, Task,
    UniformListScrollHandle, Window, canvas, div, point, prelude::FluentBuilder as _, quad, size,
    uniform_list,
};

use super::changes::SidebarView;
use super::{SHEET_HEADER_REMS, Workspace, format_count, sentence, sidebar_empty_state};
use crate::commit_graph::{self, Edge, GraphRow, Height};
use crate::git::{CommitInfo, GitError, RefLabel};

/// The newest commits the history reads. Older ones are a Terminal task.
const HISTORY_LIMIT: usize = 500;
/// The most lanes drawn; lines in lanes past it are clipped.
const MAX_LANES: usize = 6;
/// A lane's width, in rems.
const LANE_REMS: f32 = 0.75;

/// The history and the commit shown in the sheet.
pub(super) struct History {
    commits: Rc<Vec<CommitInfo>>,
    graph: Rc<Vec<GraphRow>>,
    loaded: bool,
    error: Option<SharedString>,
    task: Option<Task<()>>,
    /// A reload asked for while one ran.
    reload_queued: bool,
    /// The commit whose changes the sheet shows, by id.
    selected: Option<String>,
    diff: Entity<DiffState>,
    /// The commit whose patch the diff holds.
    shown: Option<String>,
    diff_error: Option<SharedString>,
    diff_task: Option<Task<()>>,
    scroll: UniformListScrollHandle,
}

impl History {
    pub(super) fn new(cx: &mut Context<Workspace>) -> Self {
        Self {
            commits: Rc::default(),
            graph: Rc::default(),
            loaded: false,
            error: None,
            task: None,
            reload_queued: false,
            selected: None,
            diff: cx.new(|cx| DiffState::new([], cx)),
            shown: None,
            diff_error: None,
            diff_task: None,
            scroll: UniformListScrollHandle::new(),
        }
    }

    pub(super) fn is_reading(&self) -> bool {
        self.task.is_some()
    }

    pub(super) fn selected(&self) -> Option<&str> {
        self.selected.as_deref()
    }

    fn commit(&self, id: &str) -> Option<&CommitInfo> {
        self.commits.iter().find(|commit| commit.id() == id)
    }
}

impl Workspace {
    /// Forget the history of a repository the window no longer shows.
    pub(super) fn reset_history(&mut self) {
        self.history.commits = Rc::default();
        self.history.graph = Rc::default();
        self.history.loaded = false;
        self.history.error = None;
        self.history.selected = None;
        self.history.shown = None;
    }

    /// Read the history again, and lay out its graph, off the main thread.
    pub(super) fn load_history(&mut self, cx: &mut Context<Self>) {
        let Some(repo) = self.changes.repo().cloned() else {
            return;
        };
        if self.history.task.is_some() {
            self.history.reload_queued = true;
            return;
        }
        let has_commits = self
            .changes
            .status()
            .is_none_or(|status| status.has_commits());
        let read = cx.background_spawn(async move {
            // `git log HEAD` fails before the first commit; there is no
            // history to show, not an error.
            let commits = if has_commits {
                repo.log(HISTORY_LIMIT)?
            } else {
                Vec::new()
            };
            let graph =
                commit_graph::layout(commits.iter().map(|commit| (commit.id(), commit.parents())));
            Ok::<_, GitError>((commits, graph))
        });
        self.history.task = Some(cx.spawn(async move |this, cx| {
            let result = read.await;
            this.update(cx, |this, cx| {
                this.history.task = None;
                if std::mem::take(&mut this.history.reload_queued) {
                    this.load_history(cx);
                }
                this.history.loaded = true;
                match result {
                    Ok((commits, graph)) => {
                        this.history.error = None;
                        if *this.history.commits != commits {
                            if let Some(id) = &this.history.selected
                                && !commits.iter().any(|commit| commit.id() == id)
                            {
                                this.history.selected = None;
                            }
                            this.history.commits = Rc::new(commits);
                            this.history.graph = Rc::new(graph);
                        }
                    }
                    Err(error) => {
                        this.history.error = Some(sentence(&error.to_string()).into());
                    }
                }
                cx.notify();
            })
            .ok();
        }));
    }

    pub(super) fn select_commit(&mut self, id: Option<String>, cx: &mut Context<Self>) {
        if self.history.selected == id {
            return;
        }
        self.history.selected = id;
        self.history.diff_error = None;
        self.load_commit_diff(cx);
        cx.notify();
    }

    fn load_commit_diff(&mut self, cx: &mut Context<Self>) {
        let (Some(repo), Some(id)) = (self.changes.repo().cloned(), self.history.selected.clone())
        else {
            self.history.diff_task = None;
            return;
        };
        if self.history.shown.as_ref() == Some(&id) {
            return;
        }
        let read = cx.background_spawn(async move {
            let patch = repo.show(&id)?;
            let files = DiffFile::parse(&patch)
                .map_err(|error| GitError::new(error.message().to_string()))?;
            Ok::<_, GitError>((id, files))
        });
        self.history.diff_task = Some(cx.spawn(async move |this, cx| {
            let result = read.await;
            this.update(cx, |this, cx| {
                this.history.diff_task = None;
                match result {
                    Ok((id, files)) => {
                        if this.history.selected.as_ref() != Some(&id) {
                            return;
                        }
                        this.history.diff_error = None;
                        this.history.shown = Some(id);
                        this.history
                            .diff
                            .update(cx, |diff, cx| diff.set_files(files, cx));
                    }
                    Err(error) => {
                        this.history.diff_error = Some(sentence(&error.to_string()).into());
                    }
                }
                cx.notify();
            })
            .ok();
        }));
    }

    /// The History view below the window controls: the branch, then the
    /// graph.
    pub(super) fn render_history(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let header = self.render_branch_header(cx);
        let body = if let Some(error) = &self.history.error {
            sidebar_empty_state(
                IconName::TriangleAlert,
                "Couldn’t read the history",
                "Git reported a problem with this repository.",
            )
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(error.clone()),
            )
            .into_any_element()
        } else if !self.history.loaded {
            Empty::new()
                .flex_none()
                .justify_start()
                .px_4()
                .pt_10()
                .header(
                    EmptyHeader::new()
                        .media(EmptyMedia::new().child(Spinner::new().small()))
                        .title(EmptyTitle::new().child("Reading history…")),
                )
                .into_any_element()
        } else if self.history.commits.is_empty() {
            sidebar_empty_state(
                IconName::Inbox,
                "No commits yet",
                "Commit from the Changes view to start the history.",
            )
            .into_any_element()
        } else {
            self.render_graph(window, cx)
        };
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .children(header)
            .child(body)
            .into_any_element()
    }

    fn render_graph(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let rem = window.rem_size();
        let commits = self.history.commits.clone();
        let graph = self.history.graph.clone();
        let lanes = graph
            .iter()
            .map(GraphRow::width)
            .max()
            .unwrap_or(1)
            .min(MAX_LANES);
        let style = GraphStyle {
            lane_width: rem * LANE_REMS,
            stroke: rem * 0.1,
            dot: rem * 0.22,
            palette: [
                theme.chart_1,
                theme.chart_2,
                theme.chart_3,
                theme.chart_4,
                theme.chart_5,
            ],
        };
        let muted = theme.muted_foreground;
        let foreground = theme.sidebar_foreground;
        let border = theme.border;
        let row_radius = theme.radius;
        let chip_radius = theme.radius / 1.5;
        let selected = self.history.selected.clone();
        let workspace = cx.weak_entity();
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |since| since.as_secs() as i64);

        uniform_list("history", commits.len(), move |range, _, _| {
            range
                .map(|ix| {
                    let commit = &commits[ix];
                    let row = graph[ix].clone();
                    let style = style.clone();
                    let id = commit.id().to_string();
                    let is_selected = selected.as_deref() == Some(commit.id());
                    let is_head = commit.is_head();
                    let workspace = workspace.clone();
                    // Each row is laid out at the list's width, so the inset
                    // is padding around the row, not its margin.
                    let item = ListItem::new(SharedString::from(format!("commit-{id}")))
                        .selected(is_selected)
                        .h_7()
                        .px_0()
                        .py_0()
                        .pl(rem * 0.25)
                        .pr_2()
                        .rounded(row_radius)
                        .text_sm()
                        .accessibility_label(SharedString::from(commit.subject().to_string()))
                        .on_click(move |_, _, cx| {
                            let id = id.clone();
                            workspace
                                .update(cx, |this, cx| this.select_commit(Some(id), cx))
                                .ok();
                        })
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_1p5()
                                .w_full()
                                .h_full()
                                .min_w_0()
                                .child(
                                    canvas(
                                        |_, _, _| {},
                                        move |bounds, _, window, _| {
                                            paint_row(&row, is_head, &style, bounds, window)
                                        },
                                    )
                                    .flex_none()
                                    .w(rem * LANE_REMS * lanes as f32)
                                    .h_full(),
                                )
                                .children(labels(commit.refs()).into_iter().map(
                                    |(label, current)| {
                                        div()
                                            .flex_none()
                                            .px_1()
                                            .rounded(chip_radius)
                                            .border_1()
                                            .border_color(border)
                                            .text_xs()
                                            .map(|this| {
                                                if current {
                                                    this.font_weight(FontWeight::MEDIUM)
                                                        .text_color(foreground)
                                                } else {
                                                    this.text_color(muted)
                                                }
                                            })
                                            .child(label)
                                    },
                                ))
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .truncate()
                                        .when(is_selected, |this| {
                                            this.font_weight(FontWeight::MEDIUM)
                                        })
                                        .child(commit.subject().to_string()),
                                )
                                .child(
                                    div()
                                        .flex_none()
                                        .text_xs()
                                        .text_color(muted)
                                        .font_features(FontFeatures(std::sync::Arc::new(vec![(
                                            "tnum".into(),
                                            1,
                                        )])))
                                        .child(age(now, commit.time(), false)),
                                ),
                        );
                    div().px_2().child(item)
                })
                .collect()
        })
        .track_scroll(&self.history.scroll)
        .flex_1()
        .min_h_0()
        .pt_1()
        .into_any_element()
    }

    /// The sheet's header while it shows a commit: its message, size, and
    /// the way back to writing.
    pub(super) fn render_commit_header(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        let id = self.history.selected.clone().unwrap_or_default();
        let commit = self.history.commit(&id);
        let shown = self.history.shown.as_ref() == Some(&id);
        let files = shown.then(|| self.history.diff.read(cx).files().to_vec());
        let totals = files.as_ref().map(|files| {
            files.iter().fold((0, 0), |(added, removed), file| {
                (added + file.additions(), removed + file.deletions())
            })
        });
        let tnum = FontFeatures(std::sync::Arc::new(vec![("tnum".into(), 1)]));

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
                            .min_w_0()
                            .truncate()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(commit.map_or_else(String::new, |c| c.subject().to_string())),
                    )
                    .when_some(commit, |this, commit| {
                        this.child(
                            div()
                                .flex_none()
                                .text_xs()
                                .text_color(muted)
                                .font_family(theme.mono_font_family.clone())
                                .child(commit.short_id().to_string()),
                        )
                    })
                    .when_some(
                        files.as_ref().zip(totals),
                        |this, (files, (added, removed))| {
                            this.child(
                                div()
                                    .flex()
                                    .flex_none()
                                    .gap_1p5()
                                    .text_xs()
                                    .font_features(tnum)
                                    .child(div().text_color(muted).child(match files.len() {
                                        1 => "1 file".to_string(),
                                        count => format!("{} files", format_count(count)),
                                    }))
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
                        },
                    ),
            )
            .child(
                Button::new("close-commit")
                    .small()
                    .ghost()
                    .icon(IconName::Close)
                    .accessibility_label("Close commit")
                    .tooltip("Close commit (Esc)")
                    .on_click(cx.listener(|this, _, window, cx| this.close_diff(window, cx))),
            )
            .into_any_element()
    }

    /// The commit's changes, file by file.
    pub(super) fn render_commit_body(&self, cx: &mut Context<Self>) -> AnyElement {
        let id = self.history.selected.as_deref().unwrap_or_default();
        if let Some(error) = &self.history.diff_error {
            return sidebar_empty_state(
                IconName::TriangleAlert,
                "Couldn’t show this commit",
                "Git reported a problem reading it.",
            )
            .justify_center()
            .size_full()
            .child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(error.clone()),
            )
            .into_any_element();
        }
        if self.history.shown.as_deref() != Some(id) {
            return div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .child(Spinner::new().small().color(cx.theme().muted_foreground))
                .into_any_element();
        }
        if self.history.diff.read(cx).files().is_empty() {
            return sidebar_empty_state(
                IconName::Check,
                "No changes to files",
                "This commit records history without changing any file.",
            )
            .justify_center()
            .size_full()
            .into_any_element();
        }
        Diff::new(&self.history.diff)
            .soft_wrap(true)
            .hunk_separator(DiffHunkSeparator::LineInfo)
            .change_indicator(DiffChangeIndicator::Signs)
            .size_full()
            .into_any_element()
    }

    /// The sheet's footer while it shows a commit: who made it, and when.
    pub(super) fn render_commit_footer(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |since| since.as_secs() as i64);
        let note = self
            .history
            .selected
            .as_deref()
            .and_then(|id| self.history.commit(id))
            .map(|commit| {
                let mut note = format!("{}, {}", commit.author(), age(now, commit.time(), true));
                if commit.parents().len() > 1 {
                    note.push_str(". A merge, compared with its first parent");
                }
                note
            })
            .unwrap_or_default();
        StatusBar::new()
            .bg(gpui_kit::transparent_black())
            .border_color(theme.border)
            .h_7()
            .py_0()
            .px_4()
            .left(div().child(note))
            .into_any_element()
    }

    pub(super) fn history_visible(&self) -> bool {
        self.sidebar_visible && self.sidebar_view == SidebarView::History
    }
}

/// How the graph is drawn, resolved once per frame.
#[derive(Clone)]
struct GraphStyle {
    lane_width: Pixels,
    stroke: Pixels,
    dot: Pixels,
    palette: [Hsla; 5],
}

/// Draw one row's lines and its commit's dot. Lines stop at the dot's edge,
/// so HEAD's ring stays hollow on any row background.
fn paint_row(
    row: &GraphRow,
    is_head: bool,
    style: &GraphStyle,
    bounds: Bounds<Pixels>,
    window: &mut Window,
) {
    let x = |lane: usize| bounds.left() + style.lane_width * (lane as f32 + 0.5);
    let y = |height: Height| match height {
        Height::Top => bounds.top(),
        Height::Middle => bounds.center().y,
        Height::Bottom => bounds.bottom(),
    };
    let color = |index: usize| style.palette[index % style.palette.len()];
    let node = point(x(row.lane), y(Height::Middle));

    for Edge {
        from,
        to,
        color: edge_color,
    } in &row.edges
    {
        let mut start = point(x(from.0), y(from.1));
        let mut end = point(x(to.0), y(to.1));
        // Leave the dot's area clear.
        if *from == (row.lane, Height::Middle) {
            start.y += style.dot;
        }
        if *to == (row.lane, Height::Middle) {
            end.y -= style.dot;
        }
        let mut path = PathBuilder::stroke(style.stroke);
        path.move_to(start);
        if start.x == end.x {
            path.line_to(end);
        } else {
            // Leave and arrive vertically, so lanes read as continuous.
            let middle = (start.y + end.y) / 2.;
            path.cubic_bezier_to(end, point(start.x, middle), point(end.x, middle));
        }
        if let Ok(path) = path.build() {
            window.paint_path(path, color(*edge_color));
        }
    }

    let dot = Bounds::centered_at(node, size(style.dot * 2., style.dot * 2.));
    let color = color(row.color);
    if is_head {
        window.paint_quad(quad(
            dot,
            style.dot,
            gpui_kit::transparent_black(),
            style.stroke * 1.2,
            color,
            BorderStyle::Solid,
        ));
    } else {
        window.paint_quad(quad(
            dot,
            style.dot,
            color,
            Pixels::ZERO,
            gpui_kit::transparent_black(),
            BorderStyle::Solid,
        ));
    }
}

/// The names a row shows, with whether each is the current branch: the
/// first, then how many more, as the sidebar has room for one. A remote
/// branch on the same commit as its local one says nothing new, so only one
/// that differs counts: it marks where the remote is.
fn labels(refs: &[RefLabel]) -> Vec<(String, bool)> {
    let local: Vec<&str> = refs
        .iter()
        .filter_map(|label| match label {
            RefLabel::Branch { name, .. } => Some(name.as_str()),
            _ => None,
        })
        .collect();
    let names: Vec<(String, bool)> = refs
        .iter()
        .filter_map(|label| match label {
            RefLabel::Branch { name, current } => Some((name.clone(), *current)),
            RefLabel::Remote(name) => {
                let short = name
                    .split_once('/')
                    .map_or(name.as_str(), |(_, short)| short);
                (!local.contains(&short)).then(|| (name.clone(), false))
            }
            RefLabel::Tag(name) => Some((name.clone(), false)),
        })
        .collect();
    let more = names.len().saturating_sub(1);
    names
        .into_iter()
        .take(1)
        .chain((more > 0).then(|| (format!("+{more}"), false)))
        .collect()
}

/// How long ago `then` was: `3d` in the list, `3 days ago` in the sheet.
fn age(now: i64, then: i64, long: bool) -> String {
    let seconds = (now - then).max(0);
    let (count, short, unit) = match seconds {
        0..60 => {
            return if long {
                "just now".into()
            } else {
                "now".into()
            };
        }
        60..3_600 => (seconds / 60, "m", "minute"),
        3_600..86_400 => (seconds / 3_600, "h", "hour"),
        86_400..604_800 => (seconds / 86_400, "d", "day"),
        604_800..2_629_800 => (seconds / 604_800, "w", "week"),
        2_629_800..31_557_600 => (seconds / 2_629_800, "mo", "month"),
        _ => (seconds / 31_557_600, "y", "year"),
    };
    if long {
        let plural = if count == 1 { "" } else { "s" };
        format!("{count} {unit}{plural} ago")
    } else {
        format!("{count}{short}")
    }
}
