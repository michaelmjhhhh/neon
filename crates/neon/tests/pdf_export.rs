//! PDF export end to end: the app's own executable lays out the pages.
#![cfg(target_os = "macos")]

use std::path::Path;

use export::{ExportSource, ExportStyle};

fn neon() -> &'static Path {
    Path::new(env!("CARGO_BIN_EXE_neon"))
}

fn source(markdown: &str) -> ExportSource<'_> {
    ExportSource {
        markdown,
        title: "Test",
        base_dir: None,
    }
}

#[test]
fn writes_a_paginated_pdf_in_the_configured_fonts() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("out.pdf");
    let paragraph = "A line of text that fills the page. ".repeat(40);
    let markdown = format!(
        "# Title\n\n{}\n\n```\ncode\n```\n",
        vec![paragraph; 30].join("\n\n")
    );
    let style = ExportStyle {
        text_font: Some("Georgia".into()),
        code_font: Some("Courier New".into()),
        ..ExportStyle::default()
    };
    export::pdf::export(&source(&markdown), &style, &path, neon()).unwrap();

    let pdf = std::fs::read(&path).unwrap();
    assert!(pdf.starts_with(b"%PDF-"));
    let text = String::from_utf8_lossy(&pdf);
    // Quartz writes each page as `/Type /Page /Parent …`.
    let pages = text.matches("/Type /Page /").count();
    assert!(pages >= 3, "expected several pages, found {pages}");
    assert!(text.contains("Georgia"));
    assert!(text.contains("CourierNew"));
    // Only the PDF is left in the folder.
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
}

#[test]
fn a_failed_export_leaves_an_existing_file_untouched() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("missing-folder").join("out.pdf");
    assert!(export::pdf::export(&source("# Hi"), &ExportStyle::default(), &path, neon()).is_err());
    assert!(!path.exists());

    // A worker that fails reports why, and the previous export survives.
    let existing = dir.path().join("keep.pdf");
    std::fs::write(&existing, b"previous").unwrap();
    let style = ExportStyle {
        paper: (10., 10.),
        ..ExportStyle::default()
    };
    let error = export::pdf::export(&source("# Hi"), &style, &existing, neon()).unwrap_err();
    assert!(error.to_string().contains("paper size"), "{error:#}");
    assert_eq!(std::fs::read(&existing).unwrap(), b"previous");
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
}

#[test]
fn prints_callouts_highlights_and_properties_like_the_preview() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("obsidian.pdf");
    let markdown = "---\ntitle: Weekly notes\ntags: [a, b]\n---\n\n> [!tip] Read this\n> Body with [[Weekly Review]] and [the web](https://example.com/x).\n\nSome ==marked== text.\n";
    export::pdf::export(&source(markdown), &ExportStyle::default(), &path, neon()).unwrap();

    let pdf = std::fs::read(&path).unwrap();
    let text = String::from_utf8_lossy(&pdf);
    assert!(text.contains("(https://example.com/x)"));
    let fills = fill_colors(&pdf);
    // The tip's green and the accent, each as a wash on white.
    let tip_panel = [233., 243., 239.];
    let highlight = [251., 223., 212.];
    for expected in [tip_panel, highlight] {
        assert!(
            fills
                .iter()
                .any(|fill| (0..3).all(|ix| (fill[ix] * 255. - expected[ix]).abs() < 1.)),
            "no fill of {expected:?} among {fills:?}"
        );
    }
}

/// The colors the pages' content sets with `r g b sc`, from 0 to 1.
fn fill_colors(pdf: &[u8]) -> Vec<[f32; 3]> {
    use std::io::Read as _;
    let mut colors = Vec::new();
    let mut rest = pdf;
    while let Some(start) = find(rest, b"stream\n") {
        let body = &rest[start + 7..];
        let Some(end) = find(body, b"endstream") else {
            break;
        };
        let mut content = String::new();
        if flate2::read::ZlibDecoder::new(&body[..end])
            .read_to_string(&mut content)
            .is_ok()
        {
            let words: Vec<&str> = content.split_whitespace().collect();
            for (ix, word) in words.iter().enumerate() {
                if *word == "sc" && ix >= 3 {
                    let channels: Vec<f32> = words[ix - 3..ix]
                        .iter()
                        .filter_map(|word| word.parse().ok())
                        .collect();
                    if let [r, g, b] = channels[..] {
                        colors.push([r, g, b]);
                    }
                }
            }
        }
        rest = &body[end..];
    }
    colors
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}
