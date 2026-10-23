//! Export a Markdown document to HTML, PDF or Word, typeset the way the
//! preview shows it: the preview's font and size for text, the theme's
//! monospace font for code.
//!
//! HTML and Word are generated in Rust and are safe to build on any thread.
//! PDF is laid out by AppKit in a separate process; see [`pdf`].

mod docx;
mod html;
mod images;
#[cfg(target_os = "macos")]
pub mod pdf;

use std::io::Write as _;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};

pub use docx::render_docx;
pub use html::render_html;

/// A file format the document can be exported to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExportFormat {
    Html,
    Pdf,
    Docx,
}

impl ExportFormat {
    pub fn extension(self) -> &'static str {
        match self {
            Self::Html => "html",
            Self::Pdf => "pdf",
            Self::Docx => "docx",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Html => "HTML",
            Self::Pdf => "PDF",
            Self::Docx => "Word",
        }
    }
}

/// The typography and page an export is set in.
#[derive(Clone, Debug, PartialEq)]
pub struct ExportStyle {
    /// The text family, or `None` for the system font.
    pub text_font: Option<String>,
    /// The text size in CSS pixels, as the preview draws it at the default
    /// rem size. Print formats convert at 96 px to 72 pt, as browsers do.
    pub text_size_px: f32,
    /// The code family, or `None` for the system monospace font.
    pub code_font: Option<String>,
    /// Paper width and height in points.
    pub paper: (f32, f32),
}

impl Default for ExportStyle {
    fn default() -> Self {
        Self {
            text_font: None,
            text_size_px: 16.,
            code_font: None,
            paper: A4,
        }
    }
}

/// ISO A4 in points.
pub const A4: (f32, f32) = (595.28, 841.89);

/// Page margin on every side, in points.
pub(crate) const MARGIN_PT: f32 = 72.;

impl ExportStyle {
    /// The text size in points.
    pub(crate) fn text_size_pt(&self) -> f32 {
        self.text_size_px * 0.75
    }

    /// The width text runs across between the margins, in points.
    pub(crate) fn content_width_pt(&self) -> f32 {
        (self.paper.0 - 2. * MARGIN_PT).max(72.)
    }
}

/// What to export and where its relative links and images point.
pub struct ExportSource<'a> {
    pub markdown: &'a str,
    /// Used as the HTML document title.
    pub title: &'a str,
    /// The folder of the Markdown file, for resolving relative image paths.
    pub base_dir: Option<&'a Path>,
}

/// Export the document to `path` in `format`. Blocks for as long as the
/// export takes, so call it off the main thread. On failure no file is left
/// at `path`, and an existing file there is unchanged.
pub fn export(
    format: ExportFormat,
    source: &ExportSource,
    style: &ExportStyle,
    path: &Path,
) -> Result<()> {
    match format {
        ExportFormat::Html => write_atomically(path, render_html(source, style).as_bytes()),
        ExportFormat::Docx => write_atomically(path, &render_docx(source, style)?),
        #[cfg(target_os = "macos")]
        ExportFormat::Pdf => pdf::export(source, style, path, &pdf::current_executable()?),
        #[cfg(not(target_os = "macos"))]
        ExportFormat::Pdf => anyhow::bail!("PDF export is only available on macOS"),
    }
}

/// Write `bytes` to a temporary file beside `path`, then move it into place,
/// so a failed or interrupted export never leaves a partial file behind or
/// damages an existing one.
pub fn write_atomically(path: &Path, bytes: &[u8]) -> Result<()> {
    let temp = temp_path_beside(path)?;
    let result = (|| {
        let mut file = std::fs::File::create(&temp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        std::fs::rename(&temp, path)
    })();
    if result.is_err() {
        std::fs::remove_file(&temp).ok();
    }
    result.with_context(|| format!("couldn’t write {}", path.display()))
}

/// A hidden, unused file name in the folder of `path`. Renaming within one
/// folder stays on one volume, so the final move is atomic.
pub fn temp_path_beside(path: &Path) -> Result<PathBuf> {
    let dir = path
        .parent()
        .filter(|dir| !dir.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let name = path
        .file_name()
        .context("the export path has no file name")?
        .to_string_lossy();
    let pid = std::process::id();
    for attempt in 0..100u32 {
        let candidate = dir.join(format!(".{name}.{pid}-{attempt}.tmp"));
        if !candidate.exists() {
            return Ok(candidate);
        }
    }
    anyhow::bail!(
        "couldn’t find a free temporary file name in {}",
        dir.display()
    )
}

/// The accent of the preview's light theme, which highlights are washed in.
pub(crate) const ACCENT: u32 = 0xEB4B10;
/// How strong the preview's washes are: behind a highlight, behind a
/// callout, and behind a highlight inside a callout.
pub(crate) const HIGHLIGHT_OPACITY: f32 = 0.18;
pub(crate) const CALLOUT_OPACITY: f32 = 0.1;
pub(crate) const CALLOUT_HIGHLIGHT_OPACITY: f32 = 0.28;

/// A callout tone's color in the preview's light theme.
pub(crate) fn tone_color(tone: markdown::CalloutTone) -> u32 {
    use markdown::CalloutTone;
    match tone {
        CalloutTone::Note => 0x1F6FD1,
        CalloutTone::Tip | CalloutTone::Success => 0x1F8A5B,
        CalloutTone::Question | CalloutTone::Warning => 0xB7791F,
        CalloutTone::Danger => 0xD93036,
        CalloutTone::Quote => 0x5C5F68,
    }
}

/// `color` laid over white at `opacity`: the color a translucent wash shows
/// on the page, for formats without transparency.
pub(crate) fn tint(color: u32, opacity: f32) -> u32 {
    let channel = |shift: u32| {
        let value = ((color >> shift) & 0xFF) as f32;
        (255. - (255. - value) * opacity).round() as u32
    };
    channel(16) << 16 | channel(8) << 8 | channel(0)
}

/// Six uppercase hex digits, as Word writes colors.
pub(crate) fn hex(color: u32) -> String {
    format!("{color:06X}")
}

/// Characters XML 1.0 can't contain. Word refuses to open a document with
/// one in it, so they are dropped from text and attributes.
pub(crate) fn is_xml_char(c: char) -> bool {
    matches!(c, '\t' | '\n' | '\r') || (c >= ' ' && !matches!(c, '\u{FFFE}' | '\u{FFFF}'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_atomically_and_replaces_existing_files() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("out.html");
        write_atomically(&path, b"first").unwrap();
        write_atomically(&path, b"second").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"second");
        let leftovers: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(leftovers, vec![std::ffi::OsString::from("out.html")]);
    }

    #[test]
    fn a_failed_write_leaves_no_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("missing").join("out.html");
        assert!(write_atomically(&path, b"x").is_err());
        assert!(!path.exists());
    }

    #[test]
    fn tints_colors_as_washes_on_white() {
        assert_eq!(hex(tint(0x000000, 0.5)), "808080");
        assert_eq!(hex(tint(ACCENT, 1.)), "EB4B10");
        assert_eq!(hex(tint(ACCENT, 0.)), "FFFFFF");
    }

    #[test]
    fn rejects_characters_xml_cannot_hold() {
        assert!(is_xml_char('a'));
        assert!(is_xml_char('\n'));
        assert!(is_xml_char('中'));
        assert!(!is_xml_char('\u{0B}'));
        assert!(!is_xml_char('\u{FFFF}'));
    }
}
