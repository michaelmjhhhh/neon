//! Changed files grouped into the folders they are in, for the Changes
//! view: one row per folder or file, in the order the list shows them.
//!
//! A folder holding only one folder joins it in a single row, `notes/2026`,
//! so a deep path does not cost a row per level.

use std::collections::{BTreeMap, HashSet};

use crate::git::{Change, Staged};

/// One row of the grouped list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ChangeRow {
    Folder {
        /// The folder's path from the repository root, without a trailing
        /// slash; the key for collapsing it.
        path: String,
        /// What the row shows: the folder's name, or several joined ones.
        label: String,
        depth: usize,
        /// Changes inside it, at any depth.
        count: usize,
        /// How many changes inside are staged, at least in part.
        staged: usize,
        expanded: bool,
    },
    File {
        /// Index into the changes the rows were built from.
        index: usize,
        depth: usize,
    },
}

#[derive(Default)]
struct Folder {
    folders: BTreeMap<String, Folder>,
    /// Indexes of the changes directly inside, with their names.
    files: Vec<(String, usize)>,
}

impl Folder {
    fn changes(&self) -> Vec<usize> {
        let mut changes: Vec<usize> = self.files.iter().map(|(_, ix)| *ix).collect();
        for folder in self.folders.values() {
            changes.extend(folder.changes());
        }
        changes
    }
}

/// The rows for `changes`, folders first and by name, as in the Files
/// view. A folder in `collapsed` shows without its contents.
pub(crate) fn change_rows(changes: &[Change], collapsed: &HashSet<String>) -> Vec<ChangeRow> {
    let mut root = Folder::default();
    for (ix, change) in changes.iter().enumerate() {
        let mut parts: Vec<&str> = change.path().split('/').collect();
        let name = parts.pop().unwrap_or_default();
        let folder = parts.into_iter().fold(&mut root, |folder, part| {
            folder.folders.entry(part.to_string()).or_default()
        });
        folder.files.push((name.to_lowercase(), ix));
    }
    let mut rows = Vec::new();
    push_rows(&root, "", 0, changes, collapsed, &mut rows);
    rows
}

fn push_rows(
    folder: &Folder,
    prefix: &str,
    depth: usize,
    changes: &[Change],
    collapsed: &HashSet<String>,
    rows: &mut Vec<ChangeRow>,
) {
    for (name, child) in &folder.folders {
        let mut label = name.clone();
        let mut child = child;
        while child.files.is_empty() && child.folders.len() == 1 {
            let (name, only) = child.folders.iter().next().expect("one folder");
            label = format!("{label}/{name}");
            child = only;
        }
        let path = format!("{prefix}{label}");
        let inside = child.changes();
        let expanded = !collapsed.contains(&path);
        rows.push(ChangeRow::Folder {
            path: path.clone(),
            label,
            depth,
            count: inside.len(),
            staged: inside
                .iter()
                .filter(|ix| changes[**ix].staged() != Staged::No)
                .count(),
            expanded,
        });
        if expanded {
            push_rows(
                child,
                &format!("{path}/"),
                depth + 1,
                changes,
                collapsed,
                rows,
            );
        }
    }
    let mut files = folder.files.clone();
    files.sort();
    rows.extend(
        files
            .into_iter()
            .map(|(_, index)| ChangeRow::File { index, depth }),
    );
}

/// The paths of every change inside the folder at `path`.
pub(crate) fn paths_inside(changes: &[Change], path: &str) -> Vec<String> {
    let prefix = format!("{path}/");
    changes
        .iter()
        .filter(|change| change.path().starts_with(&prefix))
        .map(|change| change.path().to_string())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::Status;

    fn changes(paths: &[&str]) -> Vec<Change> {
        // Built through the status parser, the only way changes are made.
        let output: String = paths.iter().map(|path| format!("? {path}\0")).collect();
        Status::parse_for_tests(&output).changes().to_vec()
    }

    fn describe(rows: &[ChangeRow], changes: &[Change]) -> Vec<String> {
        rows.iter()
            .map(|row| match row {
                ChangeRow::Folder {
                    label,
                    depth,
                    count,
                    ..
                } => format!("{}{label}/ {count}", "  ".repeat(*depth)),
                ChangeRow::File { index, depth } => {
                    let name = changes[*index].path().rsplit('/').next().unwrap();
                    format!("{}{name}", "  ".repeat(*depth))
                }
            })
            .collect()
    }

    #[test]
    fn groups_files_under_their_folders() {
        let changes = changes(&[
            "b.md",
            "journal/tuesday.md",
            "A.md",
            "journal/monday.md",
            "notes/2026/october/plan.md",
            "journal/old/sunday.md",
        ]);
        let rows = change_rows(&changes, &HashSet::new());
        assert_eq!(
            describe(&rows, &changes),
            [
                "journal/ 3",
                "  old/ 1",
                "    sunday.md",
                "  monday.md",
                "  tuesday.md",
                "notes/2026/october/ 1",
                "  plan.md",
                "A.md",
                "b.md",
            ]
        );
    }

    #[test]
    fn hides_a_collapsed_folders_contents() {
        let changes = changes(&["journal/monday.md", "a.md"]);
        let collapsed = HashSet::from(["journal".to_string()]);
        let rows = change_rows(&changes, &collapsed);
        assert_eq!(describe(&rows, &changes), ["journal/ 1", "a.md"]);
        assert!(matches!(
            rows[0],
            ChangeRow::Folder {
                expanded: false,
                ..
            }
        ));
        assert_eq!(paths_inside(&changes, "journal"), ["journal/monday.md"]);
    }

    #[test]
    fn counts_what_is_staged_inside_a_folder() {
        let mut changes = changes(&["d/a.md", "d/b.md"]);
        let staged = |changes: &[Change]| match &change_rows(changes, &HashSet::new())[0] {
            ChangeRow::Folder { staged, count, .. } => (*staged, *count),
            ChangeRow::File { .. } => unreachable!(),
        };
        assert_eq!(staged(&changes), (0, 2));
        changes[0].set_staged(true);
        assert_eq!(staged(&changes), (1, 2));
        changes[1].set_staged(true);
        assert_eq!(staged(&changes), (2, 2));
    }
}
