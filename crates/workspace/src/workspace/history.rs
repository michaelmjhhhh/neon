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
    AnyElement, AppContext as _, BorderStyle, Bounds, ContentMask, Context, Entity, FontFeatures,
    FontWeight, Hsla, IntoElement, ParentElement as _, PathBuilder, Pixels, SharedString,
    Styled as _, Subscription, Task, UniformListScrollHandle, Window, canvas, div, point,
    prelude::FluentBuilder as _, quad, size, uniform_list,
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
/// A row's height, in rems: the subject, and its names and author below.
const ROW_REMS: f32 = 2.75;
/// How far down a row its commit's dot sits, in rems: on the subject's line,
/// not the row's middle, so the dot marks the line that names the commit.
const ROW_NODE_REMS: f32 = 1.0;

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
    _diff_subscription: Subscription,
}

impl History {
    pub(super) fn new(cx: &mut Context<Workspace>) -> Self {
        let diff = cx.new(|cx| DiffState::new([], cx));
        Self {
            commits: Rc::default(),
            graph: Rc::default(),
            loaded: false,
            error: None,
            task: None,
            reload_queued: false,
            selected: None,
            // The diff measures itself once painted and asks for another
            // frame through its state; the sheet that shows it must follow.
            _diff_subscription: cx.observe(&diff, |_, _, cx| cx.notify()),
            diff,
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
        let read = cx.background_spawn(async move {
            let commits = repo.log(HISTORY_LIMIT)?;
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
            node_y: rem * ROW_NODE_REMS,
            palette: [
                theme.chart_1,
                theme.chart_2,
                theme.chart_3,
                theme.chart_4,
                theme.chart_5,
            ],
        };
        let palette = style.palette;
        let muted = theme.muted_foreground;
        let neutral_label = theme.muted;
        let row_radius = theme.radius;
        let selected = self.history.selected.clone();
        let workspace = cx.weak_entity();
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |since| since.as_secs() as i64);
        let tnum = FontFeatures(std::sync::Arc::new(vec![("tnum".into(), 1)]));

        uniform_list("history", commits.len(), move |range, _, _| {
            range
                .map(|ix| {
                    let commit = &commits[ix];
                    let row = graph[ix].clone();
                    let lane_color = palette[row.color % palette.len()];
                    let style = style.clone();
                    let id = commit.id().to_string();
                    let is_selected = selected.as_deref() == Some(commit.id());
                    let is_head = commit.is_head();
                    let workspace = workspace.clone();
                    // Branches wear their lane's color, so a name finds its
                    // line in the graph; remotes and tags stay neutral.
                    let pills = labels(commit.refs()).into_iter().map(|label| {
                        div()
                            .flex_none()
                            .px_1p5()
                            .rounded_full()
                            .map(|this| match label.tone {
                                Tone::Lane => this
                                    .bg(lane_color.opacity(0.14))
                                    .text_color(lane_color)
                                    .when(label.current, |this| {
                                        this.font_weight(FontWeight::SEMIBOLD)
                                    }),
                                Tone::Neutral => this.bg(neutral_label).text_color(muted),
                            })
                            .child(label.name)
                    });
                    // Each row is laid out at the list's width, so the inset
                    // is padding around the row, not its margin.
                    let item = ListItem::new(SharedString::from(format!("commit-{id}")))
                        .selected(is_selected)
                        .h(rem * ROW_REMS)
                        .px_0()
                        .py_0()
                        .pl(rem * 0.25)
                        .pr_2()
                        .rounded(row_radius)
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
                                // The subject's line centers on the dot.
                                .child(
                                    div()
                                        .flex()
                                        .flex_col()
                                        .gap_0p5()
                                        .flex_1()
                                        .min_w_0()
                                        .pt(rem * (ROW_NODE_REMS - 0.625))
                                        .child(
                                            div()
                                                .flex()
                                                .items_baseline()
                                                .gap_2()
                                                .h(rem * 1.25)
                                                .min_w_0()
                                                .text_sm()
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
                                                        .font_features(tnum.clone())
                                                        .child(age(now, commit.time(), false)),
                                                ),
                                        )
                                        .child(
                                            div()
                                                .flex()
                                                .items_center()
                                                .gap_1()
                                                .min_w_0()
                                                .text_xs()
                                                .children(pills)
                                                .child(
                                                    div()
                                                        .min_w_0()
                                                        .truncate()
                                                        .text_color(muted)
                                                        .child(commit.author().to_string()),
                                                ),
                                        ),
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
                    .flex_1()
                    .min_w_0()
                    .pr_4()
                    .truncate()
                    .text_sm()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(commit.map_or_else(String::new, |c| c.subject().to_string())),
            )
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap_3()
                    // One file's header below already gives its counts.
                    .when_some(
                        files
                            .as_ref()
                            .zip(totals)
                            .filter(|(files, _)| files.len() > 1),
                        |this, (files, (added, removed))| {
                            this.child(
                                div()
                                    .flex()
                                    .gap_1p5()
                                    .text_xs()
                                    .font_features(tnum)
                                    .child(
                                        div()
                                            .text_color(muted)
                                            .child(format!("{} files", format_count(files.len()))),
                                    )
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
                    )
                    .child(
                        Button::new("close-commit")
                            .small()
                            .ghost()
                            .icon(IconName::Close)
                            .accessibility_label("Close commit")
                            .tooltip("Close commit (Esc)")
                            .on_click(
                                cx.listener(|this, _, window, cx| this.close_diff(window, cx)),
                            ),
                    ),
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
            // A faint tint under each changed line keeps a reworked
            // paragraph one block, with its changed words darker inside;
            // without it, the few unchanged words show as holes.
            .change_background(true)
            // The lines between hunks are not in the patch, so their count
            // only says something is skipped; a band says that quietly.
            .hunk_separator(DiffHunkSeparator::Simple)
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
        let commit = self
            .history
            .selected
            .as_deref()
            .and_then(|id| self.history.commit(id));
        let note = commit.map(|commit| {
            let mut note = format!("{}, {}", commit.author(), age(now, commit.time(), true));
            if commit.parents().len() > 1 {
                note.push_str(". A merge, compared with its first parent");
            }
            note
        });
        StatusBar::new()
            .bg(gpui_kit::transparent_black())
            .border_color(theme.border)
            .h_7()
            .py_0()
            .px_4()
            .left(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .when_some(commit, |this, commit| {
                        this.child(
                            div()
                                .font_family(theme.mono_font_family.clone())
                                .text_color(theme.muted_foreground)
                                .child(commit.short_id().to_string()),
                        )
                    })
                    .children(note),
            )
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
    /// The dot's distance from the row's top.
    node_y: Pixels,
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
        Height::Middle => bounds.top() + style.node_y,
        Height::Bottom => bounds.bottom(),
    };
    let color = |index: usize| style.palette[index % style.palette.len()];
    let node = point(x(row.lane), y(Height::Middle));
    // Lanes past the column's width would draw over the subject.
    window.with_content_mask(Some(ContentMask { bounds }), |window| {
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
    });
}

/// How a name is colored in the list.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Tone {
    /// A local branch, in its lane's color.
    Lane,
    /// A remote branch or a tag.
    Neutral,
}

struct Label {
    name: String,
    tone: Tone,
    current: bool,
}

/// The names a row shows: up to two, then how many more. A remote branch on
/// the same commit as its local one says nothing new, so only one that
/// differs shows: it marks where the remote is.
fn labels(refs: &[RefLabel]) -> Vec<Label> {
    let local: Vec<&str> = refs
        .iter()
        .filter_map(|label| match label {
            RefLabel::Branch { name, .. } => Some(name.as_str()),
            _ => None,
        })
        .collect();
    let mut names: Vec<Label> = refs
        .iter()
        .filter_map(|label| match label {
            RefLabel::Branch { name, current } => Some(Label {
                name: name.clone(),
                tone: Tone::Lane,
                current: *current,
            }),
            RefLabel::Remote(name) => {
                let short = name
                    .split_once('/')
                    .map_or(name.as_str(), |(_, short)| short);
                (!local.contains(&short)).then(|| Label {
                    name: name.clone(),
                    tone: Tone::Neutral,
                    current: false,
                })
            }
            RefLabel::Tag(name) => Some(Label {
                name: name.clone(),
                tone: Tone::Neutral,
                current: false,
            }),
        })
        .collect();
    if names.len() > 2 {
        let more = names.len() - 1;
        names.truncate(1);
        names.push(Label {
            name: format!("+{more}"),
            tone: Tone::Neutral,
            current: false,
        });
    }
    names
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
