//! Lint issues in the editor, and fixes on request or on save; the rules
//! and their configuration live in the `lint` crate.
//!
//! Everything that reads the disk or the whole document runs off the main
//! thread: resolving a note's configuration costs a few `stat` calls, and
//! linting a ten-thousand-line note about two milliseconds. The main thread
//! only hands the editor the issues found, once typing pauses.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use gpui_kit::base::input::{Diagnostic, DiagnosticSeverity};
use gpui_kit::component::WindowExt as _;
use gpui_kit::component::input::{EditorState, Position, Rope, RopeExt as _};
use gpui_kit::component::notification::Notification;
use gpui_kit::{App, AppContext as _, Context, Entity, Global, SharedString, Window};
use lint::{Config, FixMode, FixOutcome, Level, Resolver};

use super::{FixLintIssues, Workspace};

/// How long typing pauses before the note is linted again. Longer than the
/// outline's wait: an issue that appears mid-word is noise.
const LINT_DELAY: Duration = Duration::from_millis(500);

/// At most this many issues are marked, so a note full of them stays as
/// quick to edit as any other. Fixing still fixes them all.
const MAX_ISSUES: usize = 1000;

/// Finds each note's configuration, shared by every window so files are
/// read once.
struct LintResolver(Arc<Mutex<Resolver>>);

impl Global for LintResolver {}

fn resolver(cx: &mut App) -> Arc<Mutex<Resolver>> {
    if !cx.has_global::<LintResolver>() {
        let global = crate::settings_file::lint_config_path(cx);
        cx.set_global(LintResolver(Arc::new(Mutex::new(Resolver::new(global)))));
    }
    cx.global::<LintResolver>().0.clone()
}

fn resolve(resolver: &Mutex<Resolver>, file: Option<&Path>, folder: Option<&Path>) -> Arc<Config> {
    resolver
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .resolve(file, folder)
}

/// An issue ready for the editor.
struct Issue {
    diagnostic: Diagnostic,
    /// See [`lint::Finding::transient`].
    transient: bool,
    line: u32,
}

/// The issues `config` finds in `source`, at most [`MAX_ISSUES`].
fn issues(source: &str, config: &Config) -> Vec<Issue> {
    let findings = lint::lint(source, config);
    if findings.is_empty() {
        return Vec::new();
    }
    let positions = lint::Positions::new(source);
    findings
        .into_iter()
        .take(MAX_ISSUES)
        .map(|finding| {
            let (line, column) = positions.at(finding.range.start);
            let (end_line, end_column) = positions.at(finding.range.end);
            let severity = match finding.level {
                Level::Info | Level::Off => DiagnosticSeverity::Info,
                Level::Warning => DiagnosticSeverity::Warning,
            };
            let diagnostic = Diagnostic::new(
                Position::new(line, column)..Position::new(end_line, end_column),
                SharedString::from(finding.message.into_owned()),
            )
            .with_severity(severity)
            .with_code(finding.rule.to_string())
            .with_source("neonlint");
            Issue {
                diagnostic,
                transient: finding.transient,
                line,
            }
        })
        .collect()
}

/// Mark `issues` in `editor`, replacing those marked before. Issues that go
/// stale as the writer types on are held back on the caret's line.
fn mark(editor: &Entity<EditorState>, issues: Vec<Issue>, cx: &mut App) {
    editor.update(cx, |editor, cx| {
        let caret_line = editor.text().offset_to_point(editor.cursor()).row as u32;
        let Some(marked) = editor.diagnostics_mut() else {
            return;
        };
        if marked.is_empty() && issues.is_empty() {
            return;
        }
        marked.clear();
        marked.extend(
            issues
                .into_iter()
                .filter(|issue| !(issue.transient && issue.line == caret_line))
                .map(|issue| issue.diagnostic),
        );
        cx.notify();
    });
}

/// Apply `outcome`, the fix of `source`, to `editor` as one change, so one
/// undo reverts it. The selection follows the text it was in.
fn apply_fix(
    editor: &Entity<EditorState>,
    source: &str,
    outcome: &FixOutcome,
    window: &mut Window,
    cx: &mut App,
) {
    let Some(edit) = lint::minimal_edit(source, &outcome.text) else {
        return;
    };
    editor.update(cx, |editor, cx| {
        let selection = editor.selected_range();
        let scroll = editor.scroll_offset();
        editor.set_selected_range(edit.range, cx);
        editor.replace(edit.text, window, cx);
        let selection = outcome.map_offset(selection.start)..outcome.map_offset(selection.end);
        editor.set_selected_range(selection, cx);
        // Selecting the edited range scrolled to it; the reader stays put.
        editor.set_scroll_offset(scroll, cx);
    });
}

impl Workspace {
    /// The configuration inputs for the tab at `ix`, to resolve off the main
    /// thread.
    fn lint_inputs(
        &self,
        ix: usize,
        cx: &mut App,
    ) -> (Arc<Mutex<Resolver>>, Option<PathBuf>, Option<PathBuf>) {
        let file = self.tabs[ix].document.path().map(Path::to_path_buf);
        (resolver(cx), file, self.folder.clone())
    }

    /// Record an edit to the tab at `ix` and lint it once typing pauses.
    pub(super) fn text_changed(&mut self, ix: usize, cx: &mut Context<Self>) {
        self.tabs[ix].text_version += 1;
        self.lint_tab(ix, true, cx);
    }

    /// Lint the documents on screen whose text changed since their last
    /// lint, as when a tab is shown.
    pub(super) fn lint_visible_tabs(&mut self, cx: &mut Context<Self>) {
        let other = self.other_pane_tab().map(|tab| tab.id);
        for ix in 0..self.tabs.len() {
            let tab = &self.tabs[ix];
            let visible = ix == self.active_tab || Some(tab.id) == other;
            if visible && tab.linted_version != Some(tab.text_version) {
                self.lint_tab(ix, false, cx);
            }
        }
    }

    /// Lint every open document again, as when the settings change.
    pub(super) fn relint(&mut self, cx: &mut Context<Self>) {
        for tab in &mut self.tabs {
            tab.linted_version = None;
        }
        self.lint_visible_tabs(cx);
    }

    /// Lint the tab at `ix`, after [`LINT_DELAY`] if `wait`, and mark its
    /// issues. A later edit replaces the task, so only the last lint lands.
    fn lint_tab(&mut self, ix: usize, wait: bool, cx: &mut Context<Self>) {
        let show_by_default = crate::settings_file::saved(cx).lint_issues;
        let (resolver, file, folder) = self.lint_inputs(ix, cx);
        let tab = &mut self.tabs[ix];
        let version = tab.text_version;
        tab.linted_version = Some(version);
        let editor = tab.editor.clone();
        let text: Rope = editor.read(cx).text().clone();
        tab.lint_task = Some(cx.spawn(async move |this, cx| {
            if wait {
                cx.background_executor().timer(LINT_DELAY).await;
            }
            let issues = cx
                .background_spawn(async move {
                    let config = resolve(&resolver, file.as_deref(), folder.as_deref());
                    if config.is_empty() || !config.show_issues().unwrap_or(show_by_default) {
                        return Vec::new();
                    }
                    issues(&text.to_string(), &config)
                })
                .await;
            this.update(cx, |this, cx| {
                let current = this
                    .tabs
                    .iter()
                    .any(|tab| tab.editor == editor && tab.text_version == version);
                if current {
                    mark(&editor, issues, cx);
                }
            })
            .ok();
        }));
    }

    /// Fix every issue the active document's rules can fix.
    pub(super) fn fix_lint_issues(
        &mut self,
        _: &FixLintIssues,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.is_busy() {
            return;
        }
        let ix = self.active_tab;
        let (resolver, file, folder) = self.lint_inputs(ix, cx);
        let tab = &self.tabs[ix];
        let (editor, version) = (tab.editor.clone(), tab.text_version);
        let text: Rope = editor.read(cx).text().clone();
        let run = cx.background_spawn(async move {
            let config = resolve(&resolver, file.as_deref(), folder.as_deref());
            let source = text.to_string();
            let outcome = lint::fix(&source, &config, FixMode::All);
            // What is left needs a person: a heading level, say.
            let left = lint::lint(
                outcome.as_ref().map_or(&source, |outcome| &outcome.text),
                &config,
            )
            .len();
            (config, source, outcome, left)
        });
        cx.spawn_in(window, async move |this, cx| {
            let (config, source, outcome, left) = run.await;
            this.update_in(cx, |this, window, cx| {
                let Some(tab) = this.tabs.iter().find(|tab| tab.editor == editor) else {
                    return;
                };
                if tab.text_version != version {
                    window.push_notification(
                        Notification::info("The note changed while it was being fixed. Try again."),
                        cx,
                    );
                    return;
                }
                if let Some(outcome) = &outcome {
                    apply_fix(&editor, &source, outcome, window, cx);
                }
                let notification = fix_summary(&config, outcome.as_ref(), left);
                window.push_notification(notification, cx);
            })
            .ok();
        })
        .detach();
    }

    /// Whether saving should first fix the document: the setting, unless the
    /// note's configuration says otherwise, which is only known once
    /// resolved, so this is checked again off the main thread.
    pub(super) fn fix_before_save(
        &mut self,
        tab_id: u64,
        cx: &mut Context<Self>,
    ) -> Option<gpui_kit::Task<Option<(String, FixOutcome)>>> {
        let ix = self.tabs.iter().position(|tab| tab.id == tab_id)?;
        let fix_by_default = crate::settings_file::saved(cx).lint_on_save;
        let (resolver, file, folder) = self.lint_inputs(ix, cx);
        let text: Rope = self.tabs[ix].editor.read(cx).text().clone();
        Some(cx.background_spawn(async move {
            let config = resolve(&resolver, file.as_deref(), folder.as_deref());
            if !config.fix_on_save().unwrap_or(fix_by_default) || !config.fixes_on_save() {
                return None;
            }
            let source = text.to_string();
            let outcome = lint::fix(&source, &config, FixMode::OnSave)?;
            Some((source, outcome))
        }))
    }

    /// Apply a fix made by [`Self::fix_before_save`] to the tab with
    /// `tab_id`, if its text is still the text fixed.
    pub(super) fn apply_save_fix(
        &mut self,
        tab_id: u64,
        version: u64,
        fixed: Option<(String, FixOutcome)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((source, outcome)) = fixed else {
            return;
        };
        let Some(tab) = self.tabs.iter().find(|tab| tab.id == tab_id) else {
            return;
        };
        if tab.text_version == version {
            let editor = tab.editor.clone();
            apply_fix(&editor, &source, &outcome, window, cx);
        }
    }
}

/// What a fix did, for a notification.
fn fix_summary(config: &Config, outcome: Option<&FixOutcome>, left: usize) -> Notification {
    if config.is_ignored() {
        return Notification::info("This note is ignored by its folder’s .neonlint.toml.");
    }
    if let Some(error) = config.errors().first() {
        let more = config.errors().len() - 1;
        let more = if more > 0 {
            format!(" ({more} more)")
        } else {
            String::new()
        };
        return Notification::warning(format!("Lint configuration: {error}{more}"));
    }
    let fixed = outcome.map_or(0, |outcome| outcome.fixed);
    let mut message = match fixed {
        0 => "No issues to fix.".to_string(),
        1 => "Fixed 1 issue.".to_string(),
        n => format!("Fixed {n} issues."),
    };
    match left {
        0 => {}
        1 => message.push_str(" 1 needs a manual fix."),
        n => message.push_str(&format!(" {n} need a manual fix.")),
    }
    if let Some(outcome) = outcome
        && !outcome.rejected.is_empty()
    {
        let names: Vec<String> = outcome.rejected.iter().map(ToString::to_string).collect();
        message.push_str(&format!(
            " Left {} alone: fixing would change how the note reads.",
            names.join(", ")
        ));
    }
    if fixed == 0 {
        Notification::info(message)
    } else {
        Notification::success(message)
    }
}
