use gpui_kit::SharedString;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io;
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

    /// Record how many Markdown files each folder holds, at any depth, and
    /// return this entry's count.
    pub(super) fn count_files(&self, counts: &mut HashMap<SharedString, usize>) -> usize {
        if !self.is_dir {
            return 1;
        }
        let count = self
            .children
            .iter()
            .map(|child| child.count_files(counts))
            .sum();
        counts.insert(self.path.to_string_lossy().into_owned().into(), count);
        count
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

/// The Markdown file in `folder` that a link such as `Note.md` or
/// `sub/Note.md` names, the way Obsidian resolves `[[Note]]`: a file whose
/// path ends with the link, compared without case, and of several, the one
/// nearest the top of the folder.
pub(super) fn find_note(folder: &Path, link: &Path) -> Option<PathBuf> {
    fn lowercase(path: &Path) -> Vec<String> {
        path.components()
            .map(|part| part.as_os_str().to_string_lossy().to_lowercase())
            .collect()
    }
    fn walk(entries: Vec<FolderEntry>, files: &mut Vec<PathBuf>) {
        for entry in entries {
            if entry.is_dir {
                walk(entry.children, files);
            } else {
                files.push(entry.path);
            }
        }
    }
    let wanted = lowercase(link);
    if wanted.is_empty() {
        return None;
    }
    let mut files = Vec::new();
    walk(scan_folder(folder).ok()?, &mut files);
    files
        .into_iter()
        .filter(|path| {
            let relative = path.strip_prefix(folder).unwrap_or(path);
            lowercase(relative).ends_with(&wanted)
        })
        .min_by_key(|path| path.components().count())
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

/// Copy `source`, a file or a folder, into `directory` and return the copy's
/// path. The copy keeps its name where that is free, else takes the first
/// free one of `name copy`, `name copy 2`, and so on, as in Finder. Nothing
/// is ever overwritten.
pub(super) fn copy_entry(source: &Path, directory: &Path) -> io::Result<PathBuf> {
    let metadata = fs::symlink_metadata(source).map_err(|error| match error.kind() {
        io::ErrorKind::NotFound => io::Error::new(error.kind(), "it no longer exists"),
        _ => error,
    })?;
    let name = source
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "it has no name"))?
        .to_string_lossy()
        .into_owned();
    if metadata.is_dir() {
        // Copying `notes` into `notes/daily` would copy its own copy over
        // and over, a level deeper each time.
        if fs::canonicalize(directory)?.starts_with(fs::canonicalize(source)?) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "a folder can’t be pasted into itself",
            ));
        }
        copy_folder(source, directory, &name)
    } else {
        copy_file(source, directory, &name)
    }
}

fn copy_file(source: &Path, directory: &Path, name: &str) -> io::Result<PathBuf> {
    for candidate in copy_names(name, false) {
        let target = directory.join(candidate);
        // Creating the file claims its name, so a file that appeared there
        // since the name was chosen is never written over.
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&target)
        {
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
        return match fs::copy(source, &target) {
            Ok(_) => Ok(target),
            Err(error) => {
                let _ = fs::remove_file(&target);
                Err(error)
            }
        };
    }
    unreachable!("copy names never run out")
}

/// Build the copy under a hidden name, which the sidebar never lists, then
/// rename it into place: a half-made copy never shows, and a failed one
/// leaves nothing behind.
fn copy_folder(source: &Path, directory: &Path, name: &str) -> io::Result<PathBuf> {
    let staging = (1..)
        .map(|n| directory.join(format!(".{name}.neon-copy-{n}")))
        .find_map(|path| match fs::create_dir(&path) {
            Ok(()) => Some(Ok(path)),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => None,
            Err(error) => Some(Err(error)),
        })
        .expect("staging names never run out")?;
    let result = copy_dir_contents(source, &staging).and_then(|()| {
        for candidate in copy_names(name, true) {
            let target = directory.join(candidate);
            // A rename replaces an empty folder, so check first. One made in
            // the moment between is empty, and losing it loses nothing.
            if fs::symlink_metadata(&target).is_ok() {
                continue;
            }
            match fs::rename(&staging, &target) {
                Ok(()) => return Ok(target),
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::AlreadyExists
                            | io::ErrorKind::DirectoryNotEmpty
                            | io::ErrorKind::NotADirectory
                    ) =>
                {
                    continue;
                }
                Err(error) => return Err(error),
            }
        }
        unreachable!("copy names never run out")
    });
    if result.is_err() {
        let _ = fs::remove_dir_all(&staging);
    }
    result
}

/// Copy everything in `from` into the existing folder `to`: hidden files and
/// files the sidebar does not list too, since notes link to their images.
/// Links are copied as links, not followed, so a link to an enclosing folder
/// cannot loop.
fn copy_dir_contents(from: &Path, to: &Path) -> io::Result<()> {
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        let target = to.join(entry.file_name());
        if kind.is_dir() {
            fs::create_dir(&target)?;
            copy_dir_contents(&entry.path(), &target)?;
        } else if kind.is_symlink() {
            #[cfg(unix)]
            std::os::unix::fs::symlink(fs::read_link(entry.path())?, &target)?;
        } else if kind.is_file() {
            fs::copy(entry.path(), &target)?;
        }
        // Sockets and pipes have no contents to copy.
    }
    Ok(())
}

/// The names a copy of `name` may take, best first: `name` itself, then
/// `name copy`, `name copy 2`, and so on, before a file's extension. A copy
/// of a copy counts on from its base name rather than reading `copy copy`.
fn copy_names(name: &str, is_dir: bool) -> impl Iterator<Item = String> {
    let (stem, extension) = match name.rsplit_once('.') {
        Some((stem, extension)) if !is_dir && !stem.is_empty() => (stem, format!(".{extension}")),
        _ => (name, String::new()),
    };
    let base = without_copy_suffix(stem).to_string();
    std::iter::once(name.to_string()).chain((1u64..).map(move |n| match n {
        1 => format!("{base} copy{extension}"),
        n => format!("{base} copy {n}{extension}"),
    }))
}

/// `plan` from `plan copy` or `plan copy 3`; any other stem as it is.
fn without_copy_suffix(stem: &str) -> &str {
    if let Some(base) = stem.strip_suffix(" copy") {
        return base;
    }
    match stem.rsplit_once(" copy ") {
        Some((base, n)) if !n.is_empty() && n.bytes().all(|byte| byte.is_ascii_digit()) => base,
        _ => stem,
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
        let mut counts = HashMap::new();
        let total: usize = entries.iter().map(|e| e.count_files(&mut counts)).sum();
        assert_eq!(total, 2);
    }

    #[test]
    fn finds_linked_notes_anywhere_in_the_folder() {
        let root = tempfile::tempdir().unwrap();
        let root = root.path();
        std::fs::create_dir_all(root.join("projects/old")).unwrap();
        std::fs::write(root.join("projects/Plan.md"), b"").unwrap();
        std::fs::write(root.join("projects/old/plan.md"), b"").unwrap();
        std::fs::write(root.join("projects/old/Notes.md"), b"").unwrap();

        // By name, the shallowest match wins, whatever its case.
        assert_eq!(
            find_note(root, Path::new("plan.md")),
            Some(root.join("projects/Plan.md"))
        );
        // A path narrows the match to that folder.
        assert_eq!(
            find_note(root, Path::new("old/Plan.md")),
            Some(root.join("projects/old/plan.md"))
        );
        assert_eq!(
            find_note(root, Path::new("notes.md")),
            Some(root.join("projects/old/Notes.md"))
        );
        assert_eq!(find_note(root, Path::new("Missing.md")), None);
    }

    #[test]
    fn copies_keep_their_name_where_it_is_free() {
        let root = tempfile::tempdir().unwrap();
        let root = root.path();
        fs::create_dir(root.join("archive")).unwrap();
        fs::write(root.join("plan.md"), b"# Plan").unwrap();

        let copy = copy_entry(&root.join("plan.md"), &root.join("archive")).unwrap();

        assert_eq!(copy, root.join("archive/plan.md"));
        assert_eq!(fs::read(&copy).unwrap(), b"# Plan");
        assert_eq!(fs::read(root.join("plan.md")).unwrap(), b"# Plan");
    }

    #[test]
    fn copies_beside_a_taken_name_count_up_and_overwrite_nothing() {
        let root = tempfile::tempdir().unwrap();
        let root = root.path();
        fs::write(root.join("plan.md"), b"original").unwrap();
        fs::write(root.join("plan copy 2.md"), b"keep").unwrap();

        let source = root.join("plan.md");
        assert_eq!(
            copy_entry(&source, root).unwrap(),
            root.join("plan copy.md")
        );
        // `plan copy 2.md` is taken, and is left as it was.
        assert_eq!(
            copy_entry(&source, root).unwrap(),
            root.join("plan copy 3.md")
        );
        assert_eq!(fs::read(root.join("plan copy 2.md")).unwrap(), b"keep");
        // A copy of a copy counts on instead of reading `copy copy`.
        assert_eq!(
            copy_entry(&root.join("plan copy.md"), root).unwrap(),
            root.join("plan copy 4.md")
        );
        assert_eq!(fs::read(root.join("plan copy 4.md")).unwrap(), b"original");
    }

    #[test]
    fn copies_whole_folders_without_following_links() {
        let root = tempfile::tempdir().unwrap();
        let root = root.path();
        let notes = root.join("notes");
        fs::create_dir_all(notes.join("images")).unwrap();
        fs::write(notes.join("a.md"), b"![](images/photo.png)").unwrap();
        fs::write(notes.join("images/photo.png"), b"png").unwrap();
        fs::write(notes.join(".hidden"), b"").unwrap();
        // A link to the folder it is in would loop if followed.
        std::os::unix::fs::symlink("..", notes.join("images/up")).unwrap();

        let copy = copy_entry(&notes, root).unwrap();

        assert_eq!(copy, root.join("notes copy"));
        assert_eq!(
            fs::read(copy.join("a.md")).unwrap(),
            b"![](images/photo.png)"
        );
        assert_eq!(fs::read(copy.join("images/photo.png")).unwrap(), b"png");
        assert!(copy.join(".hidden").is_file());
        assert_eq!(
            fs::read_link(copy.join("images/up")).unwrap(),
            Path::new("..")
        );
        // No staging folder is left behind.
        let mut names: Vec<_> = fs::read_dir(root)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        assert_eq!(names, ["notes", "notes copy"]);
    }

    #[test]
    fn refuses_to_paste_a_folder_into_itself() {
        let root = tempfile::tempdir().unwrap();
        let notes = root.path().join("notes");
        fs::create_dir_all(notes.join("daily")).unwrap();

        for directory in [notes.clone(), notes.join("daily")] {
            let error = copy_entry(&notes, &directory).unwrap_err();
            assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        }
        assert_eq!(fs::read_dir(notes.join("daily")).unwrap().count(), 0);
        assert_eq!(fs::read_dir(&notes).unwrap().count(), 1);
    }

    #[test]
    fn reports_a_source_that_is_gone() {
        let root = tempfile::tempdir().unwrap();
        let error = copy_entry(&root.path().join("gone.md"), root.path()).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::NotFound);
        assert_eq!(error.to_string(), "it no longer exists");
    }

    #[test]
    fn names_copies_before_the_extension() {
        let names = |name, is_dir| copy_names(name, is_dir).take(3).collect::<Vec<_>>();
        assert_eq!(names("a.md", false), ["a.md", "a copy.md", "a copy 2.md"]);
        assert_eq!(names("a copy 7.md", false)[1], "a copy.md");
        // Folders and dotfiles have no extension to keep last.
        assert_eq!(names("v1.2", true)[1], "v1.2 copy");
        assert_eq!(names(".env", false)[1], ".env copy");
        // `copy` alone or with a non-number after it is part of the name.
        assert_eq!(names("hard copy x", true)[1], "hard copy x copy");
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
