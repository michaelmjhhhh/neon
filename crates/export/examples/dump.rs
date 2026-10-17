//! Write a Markdown file as HTML, Word and PDF, for checking exports by eye:
//! `cargo run -p export --example dump -- notes.md out/notes [path/to/neon]`.

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let markdown = std::fs::read_to_string(&args[1])?;
    let source = export::ExportSource {
        markdown: &markdown,
        title: "Dump",
        base_dir: std::path::Path::new(&args[1]).parent(),
    };
    let style = export::ExportStyle {
        text_font: std::env::var("TEXT_FONT").ok(),
        code_font: std::env::var("CODE_FONT").ok(),
        ..export::ExportStyle::default()
    };
    let out = &args[2];
    export::write_atomically(
        format!("{out}.html").as_ref(),
        export::render_html(&source, &style).as_bytes(),
    )?;
    export::write_atomically(
        format!("{out}.docx").as_ref(),
        &export::render_docx(&source, &style)?,
    )?;
    #[cfg(target_os = "macos")]
    if let Some(neon) = args.get(3) {
        export::pdf::export(
            &source,
            &style,
            format!("{out}.pdf").as_ref(),
            neon.as_ref(),
        )?;
    }
    Ok(())
}
