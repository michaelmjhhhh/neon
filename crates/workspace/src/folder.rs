use gpui_kit::SharedString;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

use gpui_kit::component::tree::TreeItem;

/// The name of the row an empty folder shows. Hidden names are never listed,
/// so it cannot collide with a real entry, and its parent is the folder.
const EMPTY_PLACEHOLDER: &str = ".empty";

/// Filesystem snapshots cross the background/UI boundary without GPUI state.
pub(super) struct FolderEntry {
    path: PathBuf,
    is_dir: bool,
    children: Vec<FolderEntry>,
}

impl FolderEntry {
    /// The tree item for this entry. An empty folder gets a disabled "Empty"
    /// child: the tree treats only items with children as folders.
    pub(super) fn into_tree_item(self, expanded: &HashSet<SharedString>) -> TreeItem {
        let label = self
            .path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        let id: SharedString = self.path.to_string_lossy().into_owned().into();
        let is_expanded = expanded.contains(&id);
        let placeholder = (self.is_dir && self.children.is_empty()).then(|| {
            let id = self
                .path
                .join(EMPTY_PLACEHOLDER)
                .to_string_lossy()
                .into_owned();
            TreeItem::new(id, "Empty").disabled(true)
        });
        TreeItem::new(id, label)
            .expanded(is_expanded)
            .children(
                self.children
                    .into_iter()
                    .map(|entry| entry.into_tree_item(expanded)),
            )
            .children(placeholder)
    }
}

/// Skip hidden directories and symlinks; never follow cycles or leave the folder.
///
/// A directory is listed when it holds Markdown files or nothing at all, so a
/// new folder shows up while one of only images or code stays out of the way.
pub(super) fn scan_folder(path: &Path) -> std::io::Result<Vec<FolderEntry>> {
    scan_dir(path).map(|(entries, _)| entries)
}

/// The directory's listed entries, and whether it has any visible entry.
fn scan_dir(path: &Path) -> std::io::Result<(Vec<FolderEntry>, bool)> {
    let mut entries = Vec::new();
    let mut has_visible = false;
    for entry in std::fs::read_dir(path)? {
        let entry = entry?;
        if entry.file_name().to_string_lossy().starts_with('.') {
            continue;
        }
        has_visible = true;
        let kind = entry.file_type()?;
        let path = entry.path();
        if kind.is_dir() {
            let (children, dir_has_visible) = scan_dir(&path)?;
            if !children.is_empty() || !dir_has_visible {
                entries.push(FolderEntry {
                    path,
                    is_dir: true,
                    children,
                });
            }
        } else if kind.is_file() && is_markdown(&path) {
            entries.push(FolderEntry {
                path,
                is_dir: false,
                children: Vec::new(),
            });
        }
    }
    entries.sort_by(|a, b| {
        b.is_dir.cmp(&a.is_dir).then_with(|| {
            a.path
                .file_name()
                .unwrap_or_default()
                .to_ascii_lowercase()
                .cmp(&b.path.file_name().unwrap_or_default().to_ascii_lowercase())
        })
    });
    Ok((entries, has_visible))
}

pub(super) fn is_markdown(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("md") || ext.eq_ignore_ascii_case("markdown"))
}

/// Why `name` cannot name a new folder, if it cannot.
pub(super) fn folder_name_error(name: &str) -> Option<&'static str> {
    if name.is_empty() {
        Some("Enter a name for the folder.")
    } else if name == "." || name == ".." || name.contains(['/', '\\', '\0']) {
        Some("Folder names can’t contain “/” or be “.” or “..”.")
    } else if name.starts_with('.') {
        Some("Names starting with “.” are hidden, so the folder wouldn’t appear.")
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_empty_folders_and_hides_folders_without_markdown() {
        let root = std::env::temp_dir().join(format!("neon-scan-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("empty")).unwrap();
        std::fs::create_dir_all(root.join("images")).unwrap();
        std::fs::write(root.join("images/photo.png"), b"").unwrap();
        std::fs::create_dir_all(root.join("hidden-only")).unwrap();
        std::fs::write(root.join("hidden-only/.DS_Store"), b"").unwrap();
        std::fs::create_dir_all(root.join("notes")).unwrap();
        std::fs::write(root.join("notes/a.md"), b"").unwrap();
        std::fs::write(root.join("b.md"), b"").unwrap();

        let entries = scan_folder(&root).unwrap();
        let names: Vec<_> = entries
            .iter()
            .map(|entry| {
                entry
                    .path
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        std::fs::remove_dir_all(&root).unwrap();

        assert_eq!(names, ["empty", "hidden-only", "notes", "b.md"]);
    }

    #[test]
    fn rejects_unusable_folder_names() {
        assert!(folder_name_error("Drafts").is_none());
        assert!(folder_name_error("草稿").is_none());
        assert!(folder_name_error("").is_some());
        assert!(folder_name_error("..").is_some());
        assert!(folder_name_error("a/b").is_some());
        assert!(folder_name_error(".git").is_some());
    }
}
