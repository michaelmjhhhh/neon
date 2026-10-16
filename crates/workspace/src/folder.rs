use gpui_kit::SharedString;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

use gpui_kit::component::tree::TreeItem;

/// Filesystem snapshots cross the background/UI boundary without GPUI state.
pub(super) struct FolderEntry {
    path: PathBuf,
    children: Vec<FolderEntry>,
}

impl FolderEntry {
    pub(super) fn into_tree_item(self, expanded: &HashSet<SharedString>) -> TreeItem {
        let label = self
            .path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        let id: SharedString = self.path.to_string_lossy().into_owned().into();
        let is_expanded = expanded.contains(&id);
        TreeItem::new(id, label).expanded(is_expanded).children(
            self.children
                .into_iter()
                .map(|entry| entry.into_tree_item(expanded)),
        )
    }
}

/// Skip hidden directories and symlinks; never follow cycles or leave the folder.
pub(super) fn scan_folder(path: &Path) -> std::io::Result<Vec<FolderEntry>> {
    let mut entries = Vec::new();
    for entry in std::fs::read_dir(path)? {
        let entry = entry?;
        if entry.file_name().to_string_lossy().starts_with('.') {
            continue;
        }
        let kind = entry.file_type()?;
        let path = entry.path();
        if kind.is_dir() {
            let children = scan_folder(&path)?;
            if !children.is_empty() {
                entries.push(FolderEntry { path, children });
            }
        } else if kind.is_file() && is_markdown(&path) {
            entries.push(FolderEntry {
                path,
                children: Vec::new(),
            });
        }
    }
    entries.sort_by(|a, b| {
        a.children
            .is_empty()
            .cmp(&b.children.is_empty())
            .then_with(|| {
                a.path
                    .file_name()
                    .unwrap_or_default()
                    .to_ascii_lowercase()
                    .cmp(&b.path.file_name().unwrap_or_default().to_ascii_lowercase())
            })
    });
    Ok(entries)
}

pub(super) fn is_markdown(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("md") || ext.eq_ignore_ascii_case("markdown"))
}
