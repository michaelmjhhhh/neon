//! Settings that outlast the app: the appearance, each pane's typography, the
//! editor's line numbers and the layout new windows open in, kept as JSON in
//! the user's settings directory.

use std::path::{Path, PathBuf};

use gpui_kit::{App, Global};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer, Serialize};

use crate::theme::{self, Appearance};
use crate::workspace::LayoutMode;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct Settings {
    #[serde(deserialize_with = "or_default")]
    pub(crate) appearance: Appearance,
    pub(crate) editor: PaneTypography,
    pub(crate) preview: PaneTypography,
    /// Whether new windows show line numbers beside the editor. Off by
    /// default: prose reads better without a gutter, as in Typora.
    pub(crate) line_numbers: bool,
    /// The layout new windows open in: Editor, Split or Preview.
    #[serde(deserialize_with = "or_default")]
    pub(crate) default_layout: LayoutMode,
}

/// A choice's default when the file names one this version does not know,
/// such as a layout a later version added, so the other settings survive.
fn or_default<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: DeserializeOwned + Default,
{
    let value = serde_json::Value::deserialize(deserializer)?;
    Ok(T::deserialize(value).unwrap_or_default())
}

/// A pane's chosen font and size; `None` keeps the default, so a default
/// that changes in a later version still applies.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct PaneTypography {
    pub(crate) font: Option<String>,
    pub(crate) size: Option<u32>,
}

/// The loaded settings and where they are saved. Absent until
/// [`load`] runs, so tests and benchmarks never touch the user's file.
struct SettingsFile {
    path: PathBuf,
    settings: Settings,
}

impl Global for SettingsFile {}

/// Read the saved settings and apply the appearance. Call once at startup,
/// before opening windows; from then on, changes are saved as they happen.
pub fn load(cx: &mut App) {
    let Some(path) = config_dir().map(|dir| dir.join("settings.json")) else {
        return;
    };
    let settings: Settings = read(&path);
    theme::restore_appearance(settings.appearance, cx);
    cx.set_global(SettingsFile { path, settings });
}

/// The settings new windows start from.
pub(crate) fn saved(cx: &App) -> Settings {
    cx.try_global::<SettingsFile>()
        .map(|file| file.settings.clone())
        .unwrap_or_default()
}

/// Change the settings and save them, if they were loaded.
pub(crate) fn update(cx: &mut App, change: impl FnOnce(&mut Settings)) {
    if !cx.has_global::<SettingsFile>() {
        return;
    }
    let file = cx.global_mut::<SettingsFile>();
    let before = file.settings.clone();
    change(&mut file.settings);
    if file.settings == before {
        return;
    }
    if let Err(error) = write(&file.path, &file.settings) {
        eprintln!("Couldn’t save settings to {}: {error}", file.path.display());
    }
}

/// `~/Library/Application Support/Neon` on macOS, else `neon` under
/// `$XDG_CONFIG_HOME` or `~/.config`.
pub(crate) fn config_dir() -> Option<PathBuf> {
    let home = crate::workspace::home_dir();
    let dir = if cfg!(target_os = "macos") {
        home?.join("Library/Application Support/Neon")
    } else {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| home.map(|home| home.join(".config")))?
            .join("neon")
    };
    Some(dir)
}

/// The JSON at `path`; defaults when the file is missing or unreadable, and
/// per field when one is missing or invalid.
pub(crate) fn read<T: DeserializeOwned + Default>(path: &Path) -> T {
    let Ok(text) = std::fs::read_to_string(path) else {
        return T::default();
    };
    serde_json::from_str(&text).unwrap_or_else(|error| {
        eprintln!("Ignoring {}: {error}", path.display());
        T::default()
    })
}

/// Write through a temporary file, so a failed write never leaves a partial
/// file behind.
pub(crate) fn write(path: &Path, value: &impl Serialize) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let json = serde_json::to_string_pretty(value).map_err(std::io::Error::other)?;
    let temp = path.with_extension("json.tmp");
    std::fs::write(&temp, json)?;
    std::fs::rename(&temp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested/settings.json");
        let settings = Settings {
            appearance: Appearance::Dark,
            editor: PaneTypography {
                font: Some("Menlo".into()),
                size: Some(18),
            },
            preview: PaneTypography::default(),
            line_numbers: true,
            default_layout: LayoutMode::Preview,
        };
        write(&path, &settings).unwrap();
        assert_eq!(read::<Settings>(&path), settings);
    }

    #[test]
    fn missing_or_invalid_files_use_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        assert_eq!(read::<Settings>(&path), Settings::default());
        std::fs::write(&path, "not json").unwrap();
        assert_eq!(read::<Settings>(&path), Settings::default());
        std::fs::write(&path, r#"{"preview": {"size": 20}}"#).unwrap();
        let settings: Settings = read(&path);
        assert_eq!(settings.preview.size, Some(20));
        assert!(!settings.line_numbers);
        assert_eq!(settings.default_layout, LayoutMode::Split);
        std::fs::write(&path, r#"{"default_layout": "editor"}"#).unwrap();
        let settings: Settings = read(&path);
        assert_eq!(settings.default_layout, LayoutMode::Editor);
    }

    #[test]
    fn unknown_choices_keep_the_other_settings() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(
            &path,
            r#"{"appearance": "sepia", "default_layout": "zen", "line_numbers": true}"#,
        )
        .unwrap();
        let settings: Settings = read(&path);
        assert_eq!(settings.appearance, Appearance::default());
        assert_eq!(settings.default_layout, LayoutMode::default());
        assert!(settings.line_numbers);
    }
}
