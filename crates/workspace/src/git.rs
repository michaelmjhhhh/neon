//! Git through its command line: the repository a folder is in, what changed
//! in it, and staging, committing and pushing.
//!
//! Neon runs the `git` the user already has rather than a library, so a
//! commit reads their config, hooks, signing setup and credential helpers,
//! as one made in Terminal would. Every call blocks; run them off the main
//! thread.

use std::ffi::OsStr;
use std::fmt;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::OnceLock;

/// Lines of unchanged text a diff carries around each change: enough for
/// the whole of any note, so the viewer can unfold all of it.
const FULL_CONTEXT: &str = "--unified=100000";
/// A patch larger than this is not shown; it would take seconds to lay out.
const MAX_PATCH_BYTES: usize = 4 * 1024 * 1024;

/// What a failed Git command said, as a sentence to show.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GitError(String);

impl GitError {
    pub(crate) fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for GitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

pub(crate) type Result<T> = std::result::Result<T, GitError>;

/// How a file differs from the last commit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ChangeKind {
    Edited,
    New,
    Deleted,
    Renamed,
    /// Left with conflict markers by a merge.
    Conflicted,
}

impl ChangeKind {
    /// The word the sidebar shows, in the words the sheet header already uses
    /// for unsaved edits.
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Edited => "Edited",
            Self::New => "New",
            Self::Deleted => "Deleted",
            Self::Renamed => "Renamed",
            Self::Conflicted => "Conflict",
        }
    }
}

/// How much of a change the next commit takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Staged {
    No,
    /// Staged, then edited again: the commit takes the staged version.
    Partly,
    Yes,
}

/// One changed file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Change {
    /// Relative to the repository root, with `/` separators.
    path: String,
    /// The name before a staged rename.
    original: Option<String>,
    kind: ChangeKind,
    staged: Staged,
    /// Whether Git knows the file at all: an untracked file is in neither
    /// the last commit nor the index.
    tracked: bool,
}

impl Change {
    pub(crate) fn path(&self) -> &str {
        &self.path
    }

    pub(crate) fn original(&self) -> Option<&str> {
        self.original.as_deref()
    }

    pub(crate) fn kind(&self) -> ChangeKind {
        self.kind
    }

    pub(crate) fn staged(&self) -> Staged {
        self.staged
    }

    /// Mark the change staged or not, ahead of Git confirming it.
    pub(crate) fn set_staged(&mut self, staged: bool) {
        self.staged = if staged { Staged::Yes } else { Staged::No };
    }

    /// Every path the change touches: a rename stages its old name too.
    fn pathspecs(&self) -> impl Iterator<Item = &str> {
        std::iter::once(self.path.as_str()).chain(self.original.as_deref())
    }
}

/// The branch and what changed in the working tree, from one `git status`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Status {
    /// `None` when HEAD is detached.
    branch: Option<String>,
    /// False until the first commit.
    has_commits: bool,
    upstream: Option<String>,
    ahead: usize,
    behind: usize,
    has_remote: bool,
    changes: Vec<Change>,
}

impl Status {
    pub(crate) fn branch(&self) -> Option<&str> {
        self.branch.as_deref()
    }

    pub(crate) fn upstream(&self) -> Option<&str> {
        self.upstream.as_deref()
    }

    /// Commits on this branch that its upstream does not have.
    pub(crate) fn ahead(&self) -> usize {
        self.ahead
    }

    /// Commits on the upstream that this branch does not have.
    pub(crate) fn behind(&self) -> usize {
        self.behind
    }

    pub(crate) fn has_commits(&self) -> bool {
        self.has_commits
    }

    pub(crate) fn changes(&self) -> &[Change] {
        &self.changes
    }

    pub(crate) fn changes_mut(&mut self) -> &mut [Change] {
        &mut self.changes
    }

    pub(crate) fn change(&self, path: &str) -> Option<&Change> {
        self.changes.iter().find(|change| change.path == path)
    }

    /// Whether a push would send anything: commits the upstream lacks, or a
    /// branch with commits that has a remote but no upstream yet.
    pub(crate) fn can_push(&self) -> bool {
        self.branch.is_some()
            && self.has_commits
            && match self.upstream {
                Some(_) => self.ahead > 0,
                None => self.has_remote,
            }
    }
}

/// A Git working tree.
#[derive(Clone, Debug)]
pub(crate) struct Repo {
    root: PathBuf,
    git: PathBuf,
    /// Tests point Git at an empty home, so the user's config stays out.
    #[cfg(test)]
    home: Option<PathBuf>,
}

impl Repo {
    /// The working tree `folder` is in, if any. `None` too when Git is not
    /// installed: then the feature is simply absent.
    pub(crate) fn discover(folder: &Path) -> Option<Self> {
        let git = git_executable()?.to_path_buf();
        Self::at(git, folder.to_path_buf()).discover_root()
    }

    fn at(git: PathBuf, root: PathBuf) -> Self {
        Self {
            root,
            git,
            #[cfg(test)]
            home: None,
        }
    }

    /// This repository moved to the root of the working tree its folder is
    /// in, if the folder is in one.
    fn discover_root(mut self) -> Option<Self> {
        let output = self.run(["rev-parse", "--show-toplevel"], None).ok()?;
        let root = String::from_utf8(output.stdout).ok()?;
        self.root = PathBuf::from(root.trim_end_matches(['\n', '\r']));
        Some(self)
    }

    pub(crate) fn root(&self) -> &Path {
        &self.root
    }

    /// The absolute path of a change's file.
    pub(crate) fn absolute(&self, path: &str) -> PathBuf {
        self.root.join(path)
    }

    pub(crate) fn status(&self) -> Result<Status> {
        let output = self.run(
            [
                "status",
                "--porcelain=v2",
                "-z",
                "--branch",
                "--untracked-files=all",
            ],
            None,
        )?;
        let mut status = parse_status(&String::from_utf8_lossy(&output.stdout));
        let remotes = self.run(["remote"], None)?;
        status.has_remote = !remotes.stdout.trim_ascii().is_empty();
        Ok(status)
    }

    /// Stage every path of `changes`, deletions and renames included.
    pub(crate) fn stage<'a>(&self, changes: impl IntoIterator<Item = &'a Change>) -> Result<()> {
        let mut args = vec!["add", "--all", "--"];
        args.extend(changes.into_iter().flat_map(Change::pathspecs));
        self.run(args, None).map(drop)
    }

    /// Return `changes` to the last commit's version in the index, leaving
    /// the files themselves alone. Before the first commit this empties the
    /// index of them.
    pub(crate) fn unstage<'a>(&self, changes: impl IntoIterator<Item = &'a Change>) -> Result<()> {
        let mut args = vec!["reset", "--quiet", "--"];
        args.extend(changes.into_iter().flat_map(Change::pathspecs));
        // `reset` exits 1 when files stay modified; that is the point here.
        match self.run(args, None) {
            Ok(_) => Ok(()),
            Err(error) if error.0.is_empty() => Ok(()),
            Err(error) => Err(error),
        }
    }

    /// Commit what is staged with `message`, as typed.
    pub(crate) fn commit(&self, message: &str) -> Result<()> {
        self.run(["commit", "--quiet", "--file=-"], Some(message))
            .map(drop)
    }

    /// Push the current branch. A branch without an upstream is pushed to
    /// `origin`, or the only remote, and tracks it from then on.
    pub(crate) fn push(&self, status: &Status) -> Result<()> {
        if status.upstream.is_some() {
            return self.run(["push", "--quiet"], None).map(drop);
        }
        let output = self.run(["remote"], None)?;
        let remotes = String::from_utf8_lossy(&output.stdout);
        let remotes: Vec<&str> = remotes.lines().filter(|line| !line.is_empty()).collect();
        let remote = match remotes.as_slice() {
            [] => {
                return Err(GitError::new(
                    "This repository has no remote to push to. Add one with \
                     “git remote add” in Terminal, then push again.",
                ));
            }
            [only] => *only,
            several if several.contains(&"origin") => "origin",
            _ => {
                return Err(GitError::new(
                    "This branch has no upstream and the repository has several remotes. \
                     Push it once with “git push -u” in Terminal to choose one.",
                ));
            }
        };
        self.run(["push", "--quiet", "--set-upstream", remote, "HEAD"], None)
            .map(drop)
    }

    /// The change as a Git patch against the last commit, with the whole
    /// file as context.
    pub(crate) fn diff(&self, change: &Change, has_commits: bool) -> Result<String> {
        let output = if change.tracked && has_commits {
            let mut args = vec![
                "diff",
                "--no-color",
                "--no-ext-diff",
                "--no-textconv",
                "--find-renames",
                FULL_CONTEXT,
                "HEAD",
                "--",
            ];
            args.extend(change.pathspecs());
            self.run(args, None)?
        } else {
            // Untracked, or nothing committed yet: the file is all new.
            // `--no-index` exits 1 when the files differ, which they do.
            let output = self.output(
                [
                    "diff",
                    "--no-index",
                    "--no-color",
                    "--no-ext-diff",
                    FULL_CONTEXT,
                    "--",
                    "/dev/null",
                    change.path.as_str(),
                ],
                None,
            )?;
            if output.status.code() != Some(1) {
                return Err(failure(&output));
            }
            output
        };
        if output.stdout.len() > MAX_PATCH_BYTES {
            return Err(GitError::new("This change is too large to show."));
        }
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }

    fn command<I, S>(&self, args: I) -> Command
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let mut command = Command::new(&self.git);
        command
            .current_dir(&self.root)
            // Names as they are, not octal escapes.
            .args(["-c", "core.quotePath=false"])
            .args(args)
            // Never wait on a password prompt nobody can see; a credential
            // helper still answers.
            .env("GIT_TERMINAL_PROMPT", "0")
            // Status refreshes must not take the index lock from a commit
            // running in Terminal.
            .env("GIT_OPTIONAL_LOCKS", "0")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(test)]
        if let Some(home) = &self.home {
            command
                .env("HOME", home)
                .env("XDG_CONFIG_HOME", home)
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env("GIT_CONFIG_GLOBAL", home.join(".gitconfig"))
                .env("GIT_AUTHOR_NAME", "Neon Test")
                .env("GIT_AUTHOR_EMAIL", "test@example.com")
                .env("GIT_COMMITTER_NAME", "Neon Test")
                .env("GIT_COMMITTER_EMAIL", "test@example.com");
        }
        command
    }

    /// Run Git, feeding it `input`, whatever its exit status.
    fn output<I, S>(&self, args: I, input: Option<&str>) -> Result<Output>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let mut command = self.command(args);
        command.stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        });
        let mut child = command
            .spawn()
            .map_err(|error| GitError::new(format!("Couldn’t run Git. {error}.")))?;
        if let Some(input) = input
            && let Some(mut stdin) = child.stdin.take()
        {
            stdin
                .write_all(input.as_bytes())
                .map_err(|error| GitError::new(format!("Couldn’t run Git. {error}.")))?;
        }
        child
            .wait_with_output()
            .map_err(|error| GitError::new(format!("Couldn’t run Git. {error}.")))
    }

    /// Run Git, failing with what it said unless it succeeded.
    fn run<I, S>(&self, args: I, input: Option<&str>) -> Result<Output>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let output = self.output(args, input)?;
        if output.status.success() {
            Ok(output)
        } else {
            Err(failure(&output))
        }
    }
}

/// What Git printed about a failure, without its `error:` and `hint:`
/// prefixes and advice. Empty when it printed nothing.
fn failure(output: &Output) -> GitError {
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let text = if stderr.trim().is_empty() {
        &stdout
    } else {
        &stderr
    };
    let lines: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with("hint:"))
        .map(|line| {
            ["error: ", "fatal: ", "remote: ", "! "]
                .iter()
                .fold(line, |line, prefix| {
                    line.strip_prefix(prefix).unwrap_or(line)
                })
        })
        .take(3)
        .collect();
    GitError::new(lines.join(" "))
}

/// The `git` to run. Apps opened from Finder get a minimal `PATH`, so
/// Homebrew's install is looked for by name. `/usr/bin/git` on macOS is a
/// stub that offers to install the developer tools; it counts only once
/// they are installed, so a missing Git hides the feature instead of
/// raising an install dialog.
fn git_executable() -> Option<&'static Path> {
    static GIT: OnceLock<Option<PathBuf>> = OnceLock::new();
    GIT.get_or_init(|| {
        let from_path = std::env::var_os("PATH")
            .map(|path| {
                std::env::split_paths(&path)
                    .map(|dir| dir.join("git"))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let known = [
            "/opt/homebrew/bin/git",
            "/usr/local/bin/git",
            "/usr/bin/git",
        ]
        .into_iter()
        .map(PathBuf::from);
        from_path
            .into_iter()
            .chain(known)
            .find(|path| path.is_file() && (!is_macos_stub(path) || developer_tools_installed()))
    })
    .as_deref()
}

fn is_macos_stub(path: &Path) -> bool {
    cfg!(target_os = "macos") && path == Path::new("/usr/bin/git")
}

fn developer_tools_installed() -> bool {
    Command::new("/usr/bin/xcode-select")
        .arg("--print-path")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

/// Parse `git status --porcelain=v2 -z --branch`.
fn parse_status(output: &str) -> Status {
    let mut status = Status {
        has_commits: true,
        ..Status::default()
    };
    let mut records = output.split('\0');
    while let Some(record) = records.next() {
        if let Some(header) = record.strip_prefix("# ") {
            let (key, value) = header.split_once(' ').unwrap_or((header, ""));
            match key {
                "branch.oid" => status.has_commits = value != "(initial)",
                "branch.head" if value != "(detached)" => status.branch = Some(value.into()),
                "branch.upstream" => status.upstream = Some(value.into()),
                "branch.ab" => {
                    let mut counts = value
                        .split(' ')
                        .map(|count| count.trim_start_matches(['+', '-']).parse().unwrap_or(0));
                    status.ahead = counts.next().unwrap_or(0);
                    status.behind = counts.next().unwrap_or(0);
                }
                _ => {}
            }
            continue;
        }
        let (kind, rest) = record.split_at(record.len().min(2));
        let change = match kind {
            // XY sub mH mI mW hH hI path
            "1 " => rest
                .splitn(8, ' ')
                .nth(7)
                .map(|path| tracked_change(&rest[..2], path, None)),
            // XY sub mH mI mW hH hI score path, then the original path
            "2 " => {
                let original = records.next().map(String::from);
                rest.splitn(9, ' ')
                    .nth(8)
                    .map(|path| tracked_change(&rest[..2], path, original))
            }
            // XY sub m1 m2 m3 mW h1 h2 h3 path
            "u " => rest.splitn(10, ' ').nth(9).map(|path| Change {
                path: path.into(),
                original: None,
                kind: ChangeKind::Conflicted,
                staged: Staged::No,
                tracked: true,
            }),
            "? " => Some(Change {
                path: rest.into(),
                original: None,
                kind: ChangeKind::New,
                staged: Staged::No,
                tracked: false,
            }),
            _ => None,
        };
        status.changes.extend(change);
    }
    status.changes.sort_by(|a, b| a.path.cmp(&b.path));
    status
}

/// A change Git tracks, from its two-letter index and worktree state.
fn tracked_change(xy: &str, path: &str, original: Option<String>) -> Change {
    let mut letters = xy.chars();
    let index = letters.next().unwrap_or('.');
    let worktree = letters.next().unwrap_or('.');
    let kind = match (index, worktree) {
        ('R' | 'C', _) => ChangeKind::Renamed,
        ('A', _) => ChangeKind::New,
        ('D', _) | (_, 'D') => ChangeKind::Deleted,
        _ => ChangeKind::Edited,
    };
    let staged = match (index != '.', worktree != '.') {
        (true, true) => Staged::Partly,
        (true, false) => Staged::Yes,
        _ => Staged::No,
    };
    Change {
        path: path.into(),
        original,
        kind,
        staged,
        tracked: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn parses_branch_and_changes() {
        let output = "# branch.oid 1234\0# branch.head main\0# branch.upstream origin/main\0\
                      # branch.ab +2 -1\0\
                      1 MM N... 100644 100644 100644 aaa bbb notes/a b.md\0\
                      2 R. N... 100644 100644 100644 aaa aaa R100 new.md\0old.md\0\
                      1 .D N... 100644 100644 000000 aaa aaa gone.md\0\
                      1 A. N... 000000 100644 100644 000 aaa added.md\0\
                      u UU N... 100644 100644 100644 100644 a b c fight.md\0\
                      ? draft.md\0";
        let status = parse_status(output);
        assert_eq!(status.branch(), Some("main"));
        assert_eq!(status.upstream(), Some("origin/main"));
        assert_eq!((status.ahead(), status.behind()), (2, 1));
        assert!(status.has_commits());
        let summary: Vec<_> = status
            .changes()
            .iter()
            .map(|change| (change.path(), change.kind(), change.staged()))
            .collect();
        assert_eq!(
            summary,
            [
                ("added.md", ChangeKind::New, Staged::Yes),
                ("draft.md", ChangeKind::New, Staged::No),
                ("fight.md", ChangeKind::Conflicted, Staged::No),
                ("gone.md", ChangeKind::Deleted, Staged::No),
                ("new.md", ChangeKind::Renamed, Staged::Yes),
                ("notes/a b.md", ChangeKind::Edited, Staged::Partly),
            ]
        );
        assert_eq!(status.change("new.md").unwrap().original(), Some("old.md"));
        assert!(!status.change("draft.md").unwrap().tracked);
    }

    #[test]
    fn parses_a_repository_before_its_first_commit() {
        let status = parse_status("# branch.oid (initial)\0# branch.head main\0? a.md\0");
        assert!(!status.has_commits());
        assert!(!status.can_push());
        assert_eq!(status.changes().len(), 1);
    }

    #[test]
    fn parses_a_detached_head() {
        let status = parse_status("# branch.oid 1234\0# branch.head (detached)\0");
        assert_eq!(status.branch(), None);
        assert!(!status.can_push());
    }

    #[test]
    fn strips_git_prefixes_from_failures() {
        use std::os::unix::process::ExitStatusExt as _;
        let output = Output {
            status: std::process::ExitStatus::from_raw(1 << 8),
            stdout: Vec::new(),
            stderr: b"error: failed to push some refs to 'origin'\n\
                      hint: Updates were rejected because the remote contains work\n"
                .to_vec(),
        };
        assert_eq!(
            failure(&output).to_string(),
            "failed to push some refs to 'origin'"
        );
    }

    /// A repository in a temporary folder, with Git kept away from the
    /// user's own config.
    struct Fixture {
        _dir: tempfile::TempDir,
        repo: Repo,
    }

    impl Fixture {
        fn new() -> Option<Self> {
            let git = git_executable()?.to_path_buf();
            let dir = tempfile::tempdir().unwrap();
            let home = dir.path().join("home");
            let root = dir.path().join("notes");
            fs::create_dir_all(&home).unwrap();
            fs::create_dir_all(&root).unwrap();
            let repo = Repo {
                home: Some(home),
                ..Repo::at(git, root)
            };
            repo.run(["init", "--quiet", "--initial-branch=main"], None)
                .unwrap();
            Some(Self { _dir: dir, repo })
        }

        fn write(&self, path: &str, text: &str) {
            let path = self.repo.root.join(path);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, text).unwrap();
        }

        fn status(&self) -> Status {
            self.repo.status().unwrap()
        }
    }

    #[test]
    fn stages_commits_and_unstages() {
        let Some(fixture) = Fixture::new() else {
            return;
        };
        let repo = &fixture.repo;
        fixture.write("a.md", "one\n");
        fixture.write("ideas/b.md", "two\n");

        let status = fixture.status();
        assert!(!status.has_commits());
        let a = status.change("a.md").unwrap().clone();
        repo.stage([&a]).unwrap();
        assert_eq!(
            fixture.status().change("a.md").unwrap().staged(),
            Staged::Yes
        );

        // Unstaging works before the first commit too.
        repo.unstage([&a]).unwrap();
        assert_eq!(
            fixture.status().change("a.md").unwrap().staged(),
            Staged::No
        );

        repo.stage([&a]).unwrap();
        repo.commit("Add a\n\n# Heading lines stay\n").unwrap();
        let status = fixture.status();
        assert!(status.has_commits());
        assert_eq!(status.changes().len(), 1);
        assert_eq!(status.changes()[0].path(), "ideas/b.md");
        let log = repo.run(["log", "--format=%B"], None).unwrap();
        assert!(String::from_utf8_lossy(&log.stdout).contains("# Heading lines stay"));

        fixture.write("a.md", "one\nmore\n");
        let a = fixture.status().change("a.md").unwrap().clone();
        assert_eq!(a.kind(), ChangeKind::Edited);
        repo.stage([&a]).unwrap();
        fixture.write("a.md", "one\nmore\nagain\n");
        assert_eq!(
            fixture.status().change("a.md").unwrap().staged(),
            Staged::Partly
        );
    }

    #[test]
    fn diffs_against_the_last_commit() {
        let Some(fixture) = Fixture::new() else {
            return;
        };
        let repo = &fixture.repo;
        fixture.write("a.md", "keep\nold words here\n");
        let a = fixture.status().change("a.md").unwrap().clone();
        let patch = repo.diff(&a, false).unwrap();
        assert!(patch.contains("+old words here"));

        repo.stage([&a]).unwrap();
        repo.commit("Add a").unwrap();
        fixture.write("a.md", "keep\nnew words here\n");
        fixture.write("new note.md", "fresh\n");
        let status = fixture.status();

        let patch = repo.diff(status.change("a.md").unwrap(), true).unwrap();
        assert!(patch.contains("-old words here"));
        assert!(patch.contains("+new words here"));
        assert!(patch.contains(" keep"));

        let patch = repo
            .diff(status.change("new note.md").unwrap(), true)
            .unwrap();
        assert!(patch.contains("+fresh"));
    }

    #[test]
    fn diffs_a_staged_rename() {
        let Some(fixture) = Fixture::new() else {
            return;
        };
        let repo = &fixture.repo;
        fixture.write("old.md", "text\n");
        repo.stage(fixture.status().changes()).unwrap();
        repo.commit("Add").unwrap();
        repo.run(["mv", "old.md", "new.md"], None).unwrap();
        let status = fixture.status();
        let change = status.change("new.md").unwrap();
        assert_eq!(change.kind(), ChangeKind::Renamed);
        assert!(
            repo.diff(change, true)
                .unwrap()
                .contains("rename from old.md")
        );

        // Unstaging a rename restores both names in the index.
        repo.unstage([change]).unwrap();
        let kinds: Vec<_> = fixture
            .status()
            .changes()
            .iter()
            .map(|change| (change.path().to_string(), change.kind()))
            .collect();
        assert_eq!(
            kinds,
            [
                ("new.md".into(), ChangeKind::New),
                ("old.md".into(), ChangeKind::Deleted)
            ]
        );
    }

    #[test]
    fn pushes_and_tracks_a_new_branch() {
        let Some(fixture) = Fixture::new() else {
            return;
        };
        let repo = &fixture.repo;
        let remote = fixture.repo.root.parent().unwrap().join("remote.git");
        repo.run(
            [
                "init".as_ref(),
                "--quiet".as_ref(),
                "--bare".as_ref(),
                remote.as_os_str(),
            ],
            None,
        )
        .unwrap();

        fixture.write("a.md", "one\n");
        repo.stage(fixture.status().changes()).unwrap();
        repo.commit("Add a").unwrap();
        let status = fixture.status();
        assert!(!status.can_push(), "no remote to push to");
        assert!(
            repo.push(&status)
                .unwrap_err()
                .to_string()
                .contains("no remote")
        );

        repo.run(
            [
                "remote".as_ref(),
                "add".as_ref(),
                "origin".as_ref(),
                remote.as_os_str(),
            ],
            None,
        )
        .unwrap();
        let status = fixture.status();
        assert!(status.can_push());
        repo.push(&status).unwrap();
        let status = fixture.status();
        assert_eq!(status.upstream(), Some("origin/main"));
        assert_eq!(status.ahead(), 0);
        assert!(!status.can_push());

        fixture.write("a.md", "two\n");
        repo.stage(fixture.status().changes()).unwrap();
        repo.commit("Edit a").unwrap();
        let status = fixture.status();
        assert_eq!(status.ahead(), 1);
        repo.push(&status).unwrap();
        assert_eq!(fixture.status().ahead(), 0);
    }

    #[test]
    fn finds_the_repository_from_a_subfolder() {
        let Some(fixture) = Fixture::new() else {
            return;
        };
        fixture.write("deep/inside/a.md", "x\n");
        let within = |folder: PathBuf| {
            Repo {
                root: folder,
                ..fixture.repo.clone()
            }
            .discover_root()
        };
        let found = within(fixture.repo.root.join("deep/inside")).unwrap();
        assert_eq!(
            fs::canonicalize(found.root()).unwrap(),
            fs::canonicalize(fixture.repo.root()).unwrap()
        );
        let outside = tempfile::tempdir().unwrap();
        assert!(within(outside.path().to_path_buf()).is_none());
    }
}
