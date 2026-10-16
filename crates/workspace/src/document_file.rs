use std::path::{Path, PathBuf};

/// Where the open document lives and whether it has unsaved edits.
///
/// Edits and saves are counted rather than flagged, so a save that finishes
/// after further typing still leaves the document marked as edited.
#[derive(Debug, Default)]
pub struct DocumentFile {
    path: Option<PathBuf>,
    edit_revision: u64,
    saved_revision: u64,
}

impl DocumentFile {
    /// A document freshly read from, or written to, `path`.
    pub fn saved(path: PathBuf) -> Self {
        Self {
            path: Some(path),
            ..Self::default()
        }
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// The file name, or `Untitled` for a document never saved.
    pub fn display_name(&self) -> String {
        self.path
            .as_deref()
            .and_then(Path::file_name)
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Untitled".to_string())
    }

    pub fn is_dirty(&self) -> bool {
        self.edit_revision != self.saved_revision
    }

    /// Record an edit and return the revision it produced.
    pub fn record_edit(&mut self) -> u64 {
        self.edit_revision += 1;
        self.edit_revision
    }

    /// The revision a save started now would write.
    pub fn edit_revision(&self) -> u64 {
        self.edit_revision
    }

    /// Record that the text as of `revision` was written to `path`.
    pub fn record_save(&mut self, path: PathBuf, revision: u64) {
        self.path = Some(path);
        self.saved_revision = revision;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn untitled_until_saved() {
        let mut document = DocumentFile::default();
        assert_eq!(document.display_name(), "Untitled");
        assert!(!document.is_dirty());

        let revision = document.record_edit();
        assert!(document.is_dirty());
        document.record_save(PathBuf::from("/notes/today.md"), revision);
        assert_eq!(document.display_name(), "today.md");
        assert!(!document.is_dirty());
    }

    #[test]
    fn edits_during_a_save_keep_the_document_dirty() {
        let mut document = DocumentFile::saved(PathBuf::from("a.md"));
        document.record_edit();
        let saving = document.edit_revision();
        document.record_edit();
        document.record_save(PathBuf::from("a.md"), saving);
        assert!(document.is_dirty());
    }
}
