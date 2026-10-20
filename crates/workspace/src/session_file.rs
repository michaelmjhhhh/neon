//! The folder and file the last active window showed, kept as JSON beside the
//! settings so the next launch can open them again.

use std::path::PathBuf;

use gpui_kit::{App, Global};
use serde::{Deserialize, Serialize};

use crate::settings_file;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Session {
    pub folder: Option<PathBuf>,
    pub file: Option<PathBuf>,
}

impl Session {
    /// The paths that still exist, in the order to open them: the folder
    /// first, so the sidebar can select the file once both are read.
    pub fn existing_paths(&self) -> Vec<PathBuf> {
        let folder = self.folder.clone().filter(|path| path.is_dir());
        let file = self.file.clone().filter(|path| path.is_file());
        folder.into_iter().chain(file).collect()
    }
}

/// The loaded session and where it is saved. Absent until [`load`] runs, so
/// tests and benchmarks never touch the user's file.
struct SessionFile {
    path: PathBuf,
    session: Session,
}

impl Global for SessionFile {}

/// Read the session the last launch left, and save each change from now on.
/// Call once at startup, after any benchmark has had its chance to run.
pub fn load(cx: &mut App) {
    let Some(path) = settings_file::config_dir().map(|dir| dir.join("session.json")) else {
        return;
    };
    let session = settings_file::read(&path);
    cx.set_global(SessionFile { path, session });
}

/// The folder and file to open in place of an empty window.
pub fn saved(cx: &App) -> Session {
    cx.try_global::<SessionFile>()
        .map(|file| file.session.clone())
        .unwrap_or_default()
}

/// Record what a window shows and save it, if the session was loaded.
pub(crate) fn remember(session: Session, cx: &mut App) {
    if !cx.has_global::<SessionFile>() {
        return;
    }
    let file = cx.global_mut::<SessionFile>();
    if file.session == session {
        return;
    }
    file.session = session;
    if let Err(error) = settings_file::write(&file.path, &file.session) {
        eprintln!(
            "Couldn’t save the session to {}: {error}",
            file.path.display()
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.json");
        let session = Session {
            folder: Some(dir.path().join("notes")),
            file: Some(dir.path().join("notes/today.md")),
        };
        settings_file::write(&path, &session).unwrap();
        assert_eq!(settings_file::read::<Session>(&path), session);
    }

    #[test]
    fn skips_paths_that_no_longer_exist() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("notes");
        std::fs::create_dir(&folder).unwrap();
        let file = folder.join("today.md");
        std::fs::write(&file, "# Today").unwrap();

        let session = Session {
            folder: Some(folder.clone()),
            file: Some(file.clone()),
        };
        assert_eq!(session.existing_paths(), [folder.clone(), file.clone()]);

        std::fs::remove_file(&file).unwrap();
        assert_eq!(session.existing_paths(), [folder.clone()]);

        std::fs::remove_dir(&folder).unwrap();
        assert!(session.existing_paths().is_empty());
    }
}
