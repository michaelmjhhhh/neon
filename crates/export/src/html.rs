//! A standalone HTML page: the document's markup, the preview's typography
//! as CSS, and local images embedded so the file can be moved or shared.

use base64::Engine as _;
use pulldown_cmark::{CodeBlockKind, CowStr, Event, Parser, Tag, TagEnd, TextMergeStream};

use crate::{ExportSource, ExportStyle, images, parser_options};

const TEXT_FALLBACKS: &str = "-apple-system, BlinkMacSystemFont, \"Helvetica Neue\", sans-serif";
const CODE_FALLBACKS: &str = "ui-monospace, Menlo, monospace";

/// Render the document as a self-contained HTML page.
///
/// On screen the text sits in a centered reading column; printed, it fills
/// the page between the margins. PDF export prints this same page.
pub fn render_html(source: &ExportSource, style: &ExportStyle) -> String {
    let mut body = String::with_capacity(source.markdown.len() * 3 / 2);
    let events = Events {
        inner: TextMergeStream::new(Parser::new_ext(source.markdown, parser_options())),
        source,
    };
    pulldown_cmark::html::push_html(&mut body, events);

    let mut page = String::with_capacity(body.len() + 4096);
    page.push_str("<!DOCTYPE html>\n<html>\n<head>\n<meta charset=\"utf-8\">\n");
    page.push_str("<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n");
    page.push_str("<meta name=\"generator\" content=\"Neon\">\n<title>");
    page.push_str(&escape_html(source.title));
    page.push_str("</title>\n<style>\n");
    page.push_str(&stylesheet(style));
    page.push_str("</style>\n</head>\n<body>\n<main>\n");
    page.push_str(&body);
    page.push_str("</main>\n</body>\n</html>\n");
    page
}

/// The parser's events, adjusted to read like the preview.
struct Events<'a, 'b> {
    inner: TextMergeStream<'a, Parser<'a>>,
    source: &'b ExportSource<'b>,
}

impl<'a> Iterator for Events<'a, '_> {
    type Item = Event<'a>;

    fn next(&mut self) -> Option<Event<'a>> {
        Some(match self.inner.next()? {
            // The preview shows front matter as a YAML code block.
            Event::Start(Tag::MetadataBlock(_)) => {
                Event::Start(Tag::CodeBlock(CodeBlockKind::Fenced("yaml".into())))
            }
            Event::End(TagEnd::MetadataBlock(_)) => Event::End(TagEnd::CodeBlock),
            // The preview shows HTML as its source rather than rendering it.
            Event::Start(Tag::HtmlBlock) => Event::Html("<pre class=\"html-source\">".into()),
            Event::End(TagEnd::HtmlBlock) => Event::Html("</pre>\n".into()),
            Event::Html(html) => Event::Text(html),
            Event::InlineHtml(html) => Event::Code(html),
            // The preview breaks the line at a single newline.
            Event::SoftBreak => Event::HardBreak,
            // A glyph rather than a form control, so every format, and
            // print, shows the same mark.
            Event::TaskListMarker(done) => Event::InlineHtml(CowStr::Borrowed(if done {
                "<span class=\"task done\">\u{2611}</span> "
            } else {
                "<span class=\"task\">\u{2610}</span> "
            })),
            Event::Start(Tag::Image {
                dest_url, title, ..
            }) => {
                let alt = collect_alt(&mut self.inner);
                Event::InlineHtml(self.image_tag(&dest_url, &title, &alt).into())
            }
            event => event,
        })
    }
}

impl Events<'_, '_> {
    fn image_tag(&self, url: &str, title: &str, alt: &str) -> String {
        let mut tag = String::from("<img src=\"");
        match images::load(url, self.source.base_dir) {
            Some(image) => {
                tag.push_str("data:");
                tag.push_str(image.mime);
                tag.push_str(";base64,");
                base64::engine::general_purpose::STANDARD.encode_string(&image.bytes, &mut tag);
                tag.push('"');
            }
            None => {
                tag.push_str(&escape_html(url));
                tag.push('"');
            }
        }
        tag.push_str(" alt=\"");
        tag.push_str(&escape_html(alt));
        tag.push('"');
        if !title.is_empty() {
            tag.push_str(" title=\"");
            tag.push_str(&escape_html(title));
            tag.push('"');
        }
        tag.push('>');
        tag
    }
}

/// The plain text of an image's description, consuming events through the
/// image's end.
pub(crate) fn collect_alt<'a>(events: &mut impl Iterator<Item = Event<'a>>) -> String {
    let mut alt = String::new();
    let mut depth = 1usize;
    for event in events {
        match event {
            Event::Start(_) => depth += 1,
            Event::End(_) => {
                depth -= 1;
                if depth == 0 {
                    break;
                }
            }
            Event::Text(text) | Event::Code(text) => alt.push_str(&text),
            Event::SoftBreak | Event::HardBreak => alt.push(' '),
            _ => {}
        }
    }
    alt
}

fn stylesheet(style: &ExportStyle) -> String {
    let text_font = font_stack(style.text_font.as_deref(), TEXT_FALLBACKS);
    let code_font = font_stack(style.code_font.as_deref(), CODE_FALLBACKS);
    let size = style.text_size_pt();
    format!(
        r#":root {{ color-scheme: light; }}
html {{ -webkit-text-size-adjust: 100%; text-size-adjust: 100%; }}
body {{ margin: 0; background: #ffffff; color: #1f2023; font-family: {text_font}; font-size: {size}pt; line-height: 1.6; overflow-wrap: break-word; -webkit-print-color-adjust: exact; print-color-adjust: exact; }}
main {{ max-width: 40em; margin: 0 auto; padding: 3em 2em 4em; }}
h1, h2, h3, h4, h5, h6 {{ margin: 1.5em 0 0.5em; line-height: 1.25; font-weight: 700; color: #111216; }}
h1 {{ font-size: 2em; }}
h2 {{ font-size: 1.5em; }}
h3 {{ font-size: 1.25em; }}
h4 {{ font-size: 1.0625em; }}
h5 {{ font-size: 1em; }}
h6 {{ font-size: 0.875em; color: #5c5f68; }}
main > :first-child {{ margin-top: 0; }}
p, ul, ol, blockquote, pre, table, dl {{ margin: 0 0 1em; }}
ul, ol {{ padding-left: 1.6em; }}
li + li {{ margin-top: 0.25em; }}
li > ul, li > ol {{ margin: 0.25em 0 0; }}
li:has(> .task), li:has(> p > .task) {{ list-style: none; }}
li:has(> .task.done), li:has(> p > .task.done) {{ color: #8a8d97; }}
.task {{ display: inline-block; width: 1.2em; margin-left: -1.2em; }}
a {{ color: #2f62b8; text-decoration: underline; text-underline-offset: 0.15em; }}
code, kbd, samp, pre {{ font-family: {code_font}; font-size: 0.875em; }}
:not(pre) > code {{ padding: 0.1em 0.3em; border-radius: 4px; background: #f1f1f4; }}
pre {{ padding: 0.75em 1em; border: 1px solid #e3e3e8; border-radius: 6px; background: #f7f7f9; line-height: 1.45; overflow-x: auto; white-space: pre-wrap; }}
pre code {{ font-size: 1em; padding: 0; background: none; }}
blockquote {{ margin-left: 0; margin-right: 0; padding-left: 1em; border-left: 3px solid #c9cad0; color: #5c5f68; }}
table {{ border-collapse: collapse; }}
th, td {{ padding: 0.4em 0.75em; border: 1px solid #e3e3e8; }}
th {{ background: #f4f4f6; font-weight: 600; }}
img {{ max-width: 100%; height: auto; }}
hr {{ margin: 2em 0; border: 0; border-top: 1px solid #e3e3e8; }}
pre.html-source {{ padding: 0; border: 0; background: none; color: #8a8d97; }}
.footnote-definition {{ font-size: 0.875em; color: #5c5f68; }}
.footnote-definition p {{ display: inline; }}
@media print {{
  main {{ max-width: none; padding: 0; }}
  pre, blockquote, tr, img {{ break-inside: avoid; }}
  h1, h2, h3, h4, h5, h6 {{ break-after: avoid; break-inside: avoid; }}
}}
"#
    )
}

/// A CSS `font-family` list: the chosen family, quoted, then the fallbacks.
fn font_stack(family: Option<&str>, fallbacks: &str) -> String {
    match family {
        Some(family) => {
            let mut quoted = String::from("\"");
            for c in family.chars() {
                match c {
                    '"' | '\\' => {
                        quoted.push('\\');
                        quoted.push(c);
                    }
                    // Keeps a family name from closing the style element.
                    '<' => quoted.push_str("\\3C "),
                    c if c.is_control() => {}
                    c => quoted.push(c),
                }
            }
            quoted.push_str("\", ");
            quoted.push_str(fallbacks);
            quoted
        }
        None => fallbacks.to_string(),
    }
}

pub(crate) fn escape_html(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&#39;"),
            c => escaped.push(c),
        }
    }
    escaped
}

#[cfg(test)]
mod tests {
    use super::*;

    fn export(markdown: &str, style: &ExportStyle) -> String {
        render_html(
            &ExportSource {
                markdown,
                title: "Notes",
                base_dir: None,
            },
            style,
        )
    }

    #[test]
    fn sets_the_configured_fonts_and_size() {
        let style = ExportStyle {
            text_font: Some("Iowan Old Style".into()),
            text_size_px: 18.,
            code_font: Some("JetBrains Mono".into()),
            ..ExportStyle::default()
        };
        let html = export("Hello `code`", &style);
        assert!(html.contains("font-family: \"Iowan Old Style\", -apple-system"));
        assert!(html.contains("font-size: 13.5pt"));
        assert!(html.contains("font-family: \"JetBrains Mono\", ui-monospace"));
    }

    #[test]
    fn uses_the_system_fonts_when_none_is_chosen() {
        let html = export("Hello", &ExportStyle::default());
        assert!(html.contains("font-family: -apple-system"));
        assert!(html.contains("font-family: ui-monospace"));
        assert!(html.contains("font-size: 12pt"));
    }

    #[test]
    fn font_names_cannot_break_out_of_the_stylesheet() {
        let style = ExportStyle {
            text_font: Some("Evil\"</style><script>".into()),
            ..ExportStyle::default()
        };
        let html = export("Hello", &style);
        assert!(!html.contains("</style><script>"));
        assert!(html.contains("\"Evil\\\"\\3C /style>"));
    }

    #[test]
    fn renders_the_markdown_the_preview_supports() {
        let markdown = "---\ntitle: x\n---\n\n# Title\n\n- [x] done\n- [ ] todo\n\n| a | b |\n|---|--:|\n| 1 | 2 |\n\n~~gone~~ and a note[^1].\n\n[^1]: The note.\n";
        let html = export(markdown, &ExportStyle::default());
        assert!(html.contains("<pre><code class=\"language-yaml\">title: x"));
        assert!(html.contains("<h1>Title</h1>"));
        assert!(html.contains("<span class=\"task done\">\u{2611}</span> done"));
        assert!(html.contains("<span class=\"task\">\u{2610}</span> todo"));
        assert!(html.contains("<td style=\"text-align: right\">2</td>"));
        assert!(html.contains("<del>gone</del>"));
        assert!(html.contains("class=\"footnote-definition\""));
    }

    #[test]
    fn breaks_the_line_at_a_single_newline_like_the_preview() {
        let html = export("first\nsecond", &ExportStyle::default());
        assert!(html.contains("<p>first<br />\nsecond</p>"));
    }

    #[test]
    fn shows_html_as_source_like_the_preview() {
        let html = export(
            "<div onclick=\"x()\">hi</div>\n\nA <b>bold</b> tag.",
            &ExportStyle::default(),
        );
        assert!(
            html.contains("<pre class=\"html-source\">&lt;div onclick=\"x()\"&gt;hi&lt;/div&gt;")
        );
        assert!(html.contains("A <code>&lt;b&gt;</code>bold<code>&lt;/b&gt;</code> tag."));
        assert!(!html.contains("<div onclick"));
    }

    #[test]
    fn escapes_the_title() {
        let html = render_html(
            &ExportSource {
                markdown: "",
                title: "<b>&",
                base_dir: None,
            },
            &ExportStyle::default(),
        );
        assert!(html.contains("<title>&lt;b&gt;&amp;</title>"));
    }

    #[test]
    fn embeds_local_images_and_keeps_web_images() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("dot.png"), images::tests::png()).unwrap();
        let source = ExportSource {
            markdown: "![A *red* dot](dot.png \"Dot\")\n\n![web](https://example.com/a.png)\n\n![gone](gone.png)",
            title: "Images",
            base_dir: Some(dir.path()),
        };
        let html = render_html(&source, &ExportStyle::default());
        assert!(html.contains("<img src=\"data:image/png;base64,"));
        assert!(html.contains("alt=\"A red dot\" title=\"Dot\">"));
        assert!(html.contains("<img src=\"https://example.com/a.png\" alt=\"web\">"));
        assert!(html.contains("<img src=\"gone.png\" alt=\"gone\">"));
    }
}
