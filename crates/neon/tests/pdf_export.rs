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
