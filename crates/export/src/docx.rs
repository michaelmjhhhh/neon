//! A Word document built from the Markdown: headings as Word's heading
//! styles, so they show in the navigation pane, real numbered and bulleted
//! lists, tables, and embedded local images, all in the preview's fonts.

use anyhow::{Context as _, Result};
use docx_rs as w;
use pulldown_cmark::{Alignment, Event, Parser, Tag, TagEnd, TextMergeStream};

use crate::html::collect_alt;
use crate::{ExportSource, ExportStyle, MARGIN_PT, images, is_xml_char, parser_options};

/// Word's stand-ins for the system fonts, which documents can't name.
const SYSTEM_TEXT_FONT: &str = "Helvetica Neue";
const SYSTEM_CODE_FONT: &str = "Menlo";

const TEXT_COLOR: &str = "1F2023";
const HEADING_COLOR: &str = "111216";
const MUTED_COLOR: &str = "8A8D97";
const SECONDARY_COLOR: &str = "5C5F68";
const LINK_COLOR: &str = "2F62B8";
const BORDER_COLOR: &str = "E3E3E8";
const QUOTE_BORDER_COLOR: &str = "C9CAD0";
const CODE_FILL: &str = "F7F7F9";
const INLINE_CODE_FILL: &str = "F1F1F4";
const TABLE_HEAD_FILL: &str = "F4F4F6";

const NORMAL: &str = "Normal";
const CODE_BLOCK: &str = "SourceCode";
const HTML_SOURCE: &str = "HtmlSource";
const INLINE_CODE: &str = "VerbatimChar";
const QUOTE: &str = "BlockQuote";
const HYPERLINK: &str = "Hyperlink";
const TABLE_TEXT: &str = "TableText";
const TABLE_HEADER: &str = "TableHeader";
const FOOTNOTE: &str = "FootnoteText";
const RULE: &str = "HorizontalRule";
const LIST: &str = "ListParagraph";

/// Heading sizes relative to the text, as in the preview.
const HEADING_SCALES: [f32; 6] = [2., 1.5, 1.25, 1.0625, 1., 0.875];
const BULLETS: [&str; 3] = ["\u{2022}", "\u{25E6}", "\u{25AA}"];
/// List indent per level, in twentieths of a point.
const LIST_INDENT: i32 = 360;

/// Render the document as a Word (.docx) file.
pub fn render_docx(source: &ExportSource, style: &ExportStyle) -> Result<Vec<u8>> {
    let mut writer = Writer::new(source, style);
    let mut events = TextMergeStream::new(Parser::new_ext(source.markdown, parser_options()));
    while let Some(event) = events.next() {
        match event {
            Event::Start(Tag::Image { dest_url, .. }) => {
                let alt = collect_alt(&mut events);
                writer.image(&dest_url, &alt);
            }
            event => writer.event(event),
        }
    }
    writer.finish()
}

struct Writer<'a> {
    source: &'a ExportSource<'a>,
    style: &'a ExportStyle,
    docx: w::Docx,
    paragraph: Option<w::Paragraph>,
    link: Option<w::Hyperlink>,
    bold: usize,
    italic: usize,
    strike: usize,
    heading: Option<usize>,
    /// The text of the code, front matter or HTML block being read.
    verbatim: Option<(&'static str, String)>,
    quote_depth: usize,
    /// The numbering id of each open list, outermost first.
    lists: Vec<usize>,
    /// Whether each open list item is a completed task.
    items: Vec<bool>,
    /// The next paragraph starts a list item, so it carries the number.
    item_starts: bool,
    /// A list just ended; the next block keeps its distance from it.
    after_list: bool,
    footnote: bool,
    /// The label a footnote's first paragraph opens with.
    footnote_label: Option<String>,
    table: Option<TableState>,
    numberings: Vec<(w::AbstractNumbering, w::Numbering)>,
}

struct TableState {
    alignments: Vec<Alignment>,
    rows: Vec<w::TableRow>,
    cells: Vec<w::TableCell>,
    paragraphs: Vec<w::Paragraph>,
    in_head: bool,
    column: usize,
}

impl<'a> Writer<'a> {
    fn new(source: &'a ExportSource<'a>, style: &'a ExportStyle) -> Self {
        let text_font = style.text_font.as_deref().unwrap_or(SYSTEM_TEXT_FONT);
        let size = half_points(style.text_size_pt());
        let (width, height) = style.paper;
        let margin = twips(MARGIN_PT);
        let now = w3c_timestamp(std::time::SystemTime::now());
        let mut docx = w::Docx::new()
            .created_at(&now)
            .updated_at(&now)
            .default_fonts(fonts(text_font))
            .default_size(size)
            .page_size(twips(width) as u32, twips(height) as u32)
            .page_margin(
                w::PageMargin::new()
                    .top(margin)
                    .bottom(margin)
                    .left(margin)
                    .right(margin),
            );
        for definition in styles(style) {
            docx = docx.add_style(definition);
        }
        Self {
            source,
            style,
            docx,
            paragraph: None,
            link: None,
            bold: 0,
            italic: 0,
            strike: 0,
            heading: None,
            verbatim: None,
            quote_depth: 0,
            lists: Vec::new(),
            items: Vec::new(),
            item_starts: false,
            after_list: false,
            footnote: false,
            footnote_label: None,
            table: None,
            numberings: Vec::new(),
        }
    }

    fn event(&mut self, event: Event) {
        match event {
            Event::Start(tag) => self.start(tag),
            Event::End(tag) => self.end(tag),
            Event::Text(text) => match &mut self.verbatim {
                Some((_, verbatim)) => verbatim.push_str(&text),
                None => self.push_run(self.run(&text)),
            },
            Event::Code(code) | Event::InlineHtml(code) => {
                let run = self.run(&code).style(INLINE_CODE).shading(
                    w::Shading::new()
                        .shd_type(w::ShdType::Clear)
                        .fill(INLINE_CODE_FILL),
                );
                self.push_run(run);
            }
            Event::Html(html) => {
                if let Some((_, verbatim)) = &mut self.verbatim {
                    verbatim.push_str(&html);
                }
            }
            Event::SoftBreak => self.push_run(self.run(" ")),
            Event::HardBreak => {
                self.push_run(w::Run::new().add_break(w::BreakType::TextWrapping));
            }
            Event::FootnoteReference(label) => {
                let mut run = self.run(&format!("[{label}]")).color(LINK_COLOR);
                run.run_property = run.run_property.vert_align(w::VertAlignType::SuperScript);
                self.push_run(run);
            }
            Event::TaskListMarker(done) => {
                if let Some(item) = self.items.last_mut() {
                    *item = done;
                }
                self.push_run(self.run(if done { "\u{2611} " } else { "\u{2610} " }));
            }
            Event::Rule => {
                self.finish_paragraph();
                let paragraph = self.new_paragraph().style(RULE);
                self.push_paragraph(paragraph);
            }
            _ => {}
        }
    }

    fn start(&mut self, tag: Tag) {
        match tag {
            Tag::Paragraph => self.finish_paragraph(),
            Tag::Heading { level, .. } => {
                self.finish_paragraph();
                self.heading = Some(level as usize);
            }
            Tag::BlockQuote(_) => {
                self.finish_paragraph();
                self.quote_depth += 1;
            }
            Tag::CodeBlock(_) | Tag::MetadataBlock(_) => {
                self.finish_paragraph();
                self.verbatim = Some((CODE_BLOCK, String::new()));
            }
            Tag::HtmlBlock => {
                self.finish_paragraph();
                self.verbatim = Some((HTML_SOURCE, String::new()));
            }
            Tag::List(start) => {
                self.finish_paragraph();
                // An item that opens straight into a nested list still
                // shows its own number.
                if self.item_starts {
                    self.paragraph();
                    self.finish_paragraph();
                }
                let id = self.add_numbering(start);
                self.lists.push(id);
            }
            Tag::Item => {
                self.finish_paragraph();
                self.items.push(false);
                self.item_starts = true;
            }
            Tag::FootnoteDefinition(label) => {
                self.finish_paragraph();
                self.footnote = true;
                self.footnote_label = Some(format!("[{label}] "));
            }
            Tag::Table(alignments) => {
                self.finish_paragraph();
                self.table = Some(TableState {
                    alignments,
                    rows: Vec::new(),
                    cells: Vec::new(),
                    paragraphs: Vec::new(),
                    in_head: false,
                    column: 0,
                });
            }
            Tag::TableHead => {
                if let Some(table) = &mut self.table {
                    table.in_head = true;
                }
            }
            Tag::TableCell => self.finish_paragraph(),
            Tag::Emphasis => self.italic += 1,
            Tag::Strong => self.bold += 1,
            Tag::Strikethrough => self.strike += 1,
            Tag::Link { dest_url, .. } => {
                self.paragraph();
                // In-page anchors have no target in the Word document.
                if !dest_url.starts_with('#') {
                    self.link = Some(w::Hyperlink::new(
                        encode_uri(&dest_url),
                        w::HyperlinkType::External,
                    ));
                }
            }
            _ => {}
        }
    }

    fn end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Paragraph => self.finish_paragraph(),
            TagEnd::Heading(_) => {
                self.finish_paragraph();
                self.heading = None;
            }
            TagEnd::BlockQuote(_) => {
                self.finish_paragraph();
                self.quote_depth = self.quote_depth.saturating_sub(1);
            }
            TagEnd::CodeBlock | TagEnd::MetadataBlock(_) | TagEnd::HtmlBlock => {
                if let Some((style, text)) = self.verbatim.take() {
                    self.verbatim_paragraph(style, &text);
                }
            }
            TagEnd::List(_) => {
                self.finish_paragraph();
                self.lists.pop();
                self.after_list = self.lists.is_empty();
            }
            TagEnd::Item => {
                if self.item_starts {
                    self.paragraph();
                }
                self.finish_paragraph();
                self.items.pop();
                self.item_starts = false;
            }
            TagEnd::FootnoteDefinition => {
                self.finish_paragraph();
                self.footnote = false;
            }
            TagEnd::TableCell => {
                self.finish_paragraph();
                if let Some(table) = &mut self.table {
                    let mut paragraphs = std::mem::take(&mut table.paragraphs);
                    // Word requires every cell to hold a paragraph.
                    if paragraphs.is_empty() {
                        paragraphs.push(w::Paragraph::new().style(TABLE_TEXT));
                    }
                    let mut cell = w::TableCell::new();
                    for paragraph in paragraphs {
                        cell = cell.add_paragraph(paragraph);
                    }
                    if table.in_head {
                        cell = cell.shading(
                            w::Shading::new()
                                .shd_type(w::ShdType::Clear)
                                .fill(TABLE_HEAD_FILL),
                        );
                    }
                    table.cells.push(cell);
                    table.column += 1;
                }
            }
            TagEnd::TableHead | TagEnd::TableRow => {
                if let Some(table) = &mut self.table {
                    let cells = std::mem::take(&mut table.cells);
                    table.rows.push(w::TableRow::new(cells));
                    table.in_head = false;
                    table.column = 0;
                }
            }
            TagEnd::Table => {
                if let Some(table) = self.table.take() {
                    self.push_table(table);
                }
            }
            TagEnd::Emphasis => self.italic = self.italic.saturating_sub(1),
            TagEnd::Strong => self.bold = self.bold.saturating_sub(1),
            TagEnd::Strikethrough => self.strike = self.strike.saturating_sub(1),
            TagEnd::Link => {
                if let Some(link) = self.link.take() {
                    let paragraph = self
                        .paragraph
                        .take()
                        .unwrap_or_else(|| self.new_paragraph());
                    self.paragraph = Some(paragraph.add_hyperlink(link));
                }
            }
            _ => {}
        }
    }

    /// A local image embedded at its size, or shrunk to the text width; any
    /// other image as its description, linked when it is on the web.
    fn image(&mut self, url: &str, alt: &str) {
        let loaded = images::load(url, self.source.base_dir)
            .and_then(|image| image.to_png().map(|png| (image, png)));
        if let Some((image, png)) = loaded {
            // Word measures images at 96 pixels per inch.
            let (width, height) = image.fitted(self.style.content_width_pt() * 96. / 72.);
            let pic = w::Pic::new_with_dimensions(png, image.size.0, image.size.1)
                .size(emu(width), emu(height));
            self.push_run(w::Run::new().add_image(pic));
            return;
        }
        let description = if alt.trim().is_empty() { "Image" } else { alt };
        let run = self.run(description).italic();
        if images::is_remote(url) && self.link.is_none() {
            self.paragraph();
            let link = w::Hyperlink::new(encode_uri(url), w::HyperlinkType::External)
                .add_run(run.style(HYPERLINK));
            let paragraph = self
                .paragraph
                .take()
                .unwrap_or_else(|| self.new_paragraph());
            self.paragraph = Some(paragraph.add_hyperlink(link));
        } else {
            self.push_run(run);
        }
    }

    /// A run of text in the current inline styles.
    fn run(&self, text: &str) -> w::Run {
        let text: String = text.chars().filter(|c| is_xml_char(*c)).collect();
        let mut run = w::Run::new().add_text(text);
        if self.bold > 0 {
            run = run.bold();
        }
        if self.italic > 0 {
            run = run.italic();
        }
        if self.strike > 0 {
            run = run.strike();
        }
        if self.link.is_some() {
            run = run.style(HYPERLINK);
        }
        if self.items.last() == Some(&true) {
            run = run.color(MUTED_COLOR);
        }
        run
    }

    fn push_run(&mut self, run: w::Run) {
        if let Some(link) = self.link.take() {
            self.paragraph();
            self.link = Some(link.add_run(run));
            return;
        }
        let paragraph = self
            .paragraph
            .take()
            .unwrap_or_else(|| self.new_paragraph());
        self.paragraph = Some(paragraph.add_run(run));
    }

    /// Start the paragraph inline content goes into, if none is open.
    fn paragraph(&mut self) {
        if self.paragraph.is_none() {
            self.paragraph = Some(self.new_paragraph());
        }
    }

    /// An empty paragraph styled for where it falls in the document.
    fn new_paragraph(&mut self) -> w::Paragraph {
        let mut paragraph = w::Paragraph::new();
        if let Some(table) = &self.table {
            paragraph = paragraph.style(if table.in_head {
                TABLE_HEADER
            } else {
                TABLE_TEXT
            });
            let alignment = match table.alignments.get(table.column) {
                Some(Alignment::Center) => Some(w::AlignmentType::Center),
                Some(Alignment::Right) => Some(w::AlignmentType::Right),
                _ => None,
            };
            if let Some(alignment) = alignment {
                paragraph = paragraph.align(alignment);
            }
            return paragraph;
        }
        if let Some(level) = self.heading {
            paragraph = paragraph.style(&format!("Heading{level}"));
        } else if self.footnote {
            paragraph = paragraph.style(FOOTNOTE);
        } else if self.quote_depth > 0 {
            paragraph = paragraph.style(QUOTE);
        } else if !self.lists.is_empty() {
            paragraph = paragraph.style(LIST);
        }
        let depth = self.lists.len();
        if let Some(&id) = self.lists.last() {
            if self.item_starts {
                paragraph =
                    paragraph.numbering(w::NumberingId::new(id), w::IndentLevel::new(depth - 1));
                self.item_starts = false;
            } else {
                // A later paragraph of an item lines up with its text.
                paragraph = paragraph.indent(Some(list_indent(depth - 1)), None, None, None);
            }
        } else if self.quote_depth > 1 {
            paragraph = paragraph.indent(
                Some(LIST_INDENT * self.quote_depth as i32),
                None,
                None,
                None,
            );
        }
        if let Some(label) = self.footnote_label.take() {
            paragraph = paragraph.add_run(self.run(&label).color(LINK_COLOR));
        }
        if std::mem::take(&mut self.after_list) {
            let gap = twips(self.style.text_size_pt() * 0.75) as u32;
            paragraph = paragraph.line_spacing(w::LineSpacing::new().before(gap));
        }
        paragraph
    }

    fn finish_paragraph(&mut self) {
        if let Some(link) = self.link.take() {
            let paragraph = self
                .paragraph
                .take()
                .unwrap_or_else(|| self.new_paragraph());
            self.paragraph = Some(paragraph.add_hyperlink(link));
        }
        if let Some(paragraph) = self.paragraph.take() {
            self.push_paragraph(paragraph);
        }
    }

    fn push_paragraph(&mut self, paragraph: w::Paragraph) {
        match &mut self.table {
            Some(table) => table.paragraphs.push(paragraph),
            None => self.docx = std::mem::take(&mut self.docx).add_paragraph(paragraph),
        }
    }

    /// Code keeps its line breaks and spaces in one shaded paragraph.
    fn verbatim_paragraph(&mut self, style: &'static str, text: &str) {
        let text = text.strip_suffix('\n').unwrap_or(text);
        let mut run = w::Run::new();
        for (ix, line) in text.split('\n').enumerate() {
            if ix > 0 {
                run = run.add_break(w::BreakType::TextWrapping);
            }
            let line: String = line
                .trim_end_matches('\r')
                .replace('\t', "    ")
                .chars()
                .filter(|c| is_xml_char(*c))
                .collect();
            if !line.is_empty() {
                run = run.add_text(line);
            }
        }
        let paragraph = self.new_paragraph().style(style).add_run(run);
        self.push_paragraph(paragraph);
    }

    fn push_table(&mut self, table: TableState) {
        let columns = table
            .rows
            .iter()
            .map(|row| row.cells.len())
            .chain([table.alignments.len()])
            .max()
            .unwrap_or(1)
            .max(1);
        let width = twips(self.style.content_width_pt()) as usize;
        let table = w::Table::new(table.rows)
            .set_grid(vec![width / columns; columns])
            .width(width, w::WidthType::Dxa);
        self.docx = std::mem::take(&mut self.docx).add_table(table);
        // Word runs a paragraph straight after a table into it; a small gap
        // keeps the following text apart.
        self.after_list = true;
    }

    /// A numbering definition of its own for each list, so every ordered
    /// list counts from its own start.
    fn add_numbering(&mut self, start: Option<u64>) -> usize {
        let id = self.numberings.len() + 1;
        let mut abstract_numbering = w::AbstractNumbering::new(id);
        for level in 0..9 {
            let (format, text) = match start {
                Some(_) => ("decimal", format!("%{}.", level + 1)),
                None => ("bullet", BULLETS[level % BULLETS.len()].to_string()),
            };
            let first = match (level, start) {
                (0, Some(start)) => start.min(u32::MAX as u64) as usize,
                _ => 1,
            };
            abstract_numbering = abstract_numbering.add_level(
                w::Level::new(
                    level,
                    w::Start::new(first),
                    w::NumberFormat::new(format),
                    w::LevelText::new(text),
                    w::LevelJc::new("left"),
                )
                .indent(
                    Some(list_indent(level)),
                    Some(w::SpecialIndentType::Hanging(LIST_INDENT)),
                    None,
                    None,
                ),
            );
        }
        self.numberings
            .push((abstract_numbering, w::Numbering::new(id, id)));
        id
    }

    fn finish(mut self) -> Result<Vec<u8>> {
        self.finish_paragraph();
        if let Some(table) = self.table.take() {
            self.push_table(table);
        }
        let mut docx = self.docx;
        for (abstract_numbering, numbering) in self.numberings {
            docx = docx
                .add_abstract_numbering(abstract_numbering)
                .add_numbering(numbering);
        }
        let mut bytes = std::io::Cursor::new(Vec::new());
        docx.build()
            .pack(&mut bytes)
            .context("couldn’t assemble the Word document")?;
        Ok(bytes.into_inner())
    }
}

/// The document's styles, named the way Word names its own where one exists
/// so they behave like Word's: headings appear in the navigation pane.
fn styles(style: &ExportStyle) -> Vec<w::Style> {
    let text_font = style.text_font.as_deref().unwrap_or(SYSTEM_TEXT_FONT);
    let code_font = style.code_font.as_deref().unwrap_or(SYSTEM_CODE_FONT);
    let size = style.text_size_pt();
    let em = |scale: f32| twips(size * scale) as u32;
    // Word's line spacing multiplies the font's own line height, about 1.2
    // of its size, so these match the preview's CSS line heights.
    let lines = |css: f32| w::LineSpacing::new().line((240. * css / 1.2).round() as i32);

    let mut styles = vec![
        w::Style::new(NORMAL, w::StyleType::Paragraph)
            .name("Normal")
            .q_format(true)
            .fonts(fonts(text_font))
            .size(half_points(size))
            .color(TEXT_COLOR)
            .line_spacing(lines(1.6).after(em(0.8))),
        w::Style::new(LIST, w::StyleType::Paragraph)
            .name("List Paragraph")
            .based_on(NORMAL)
            .line_spacing(lines(1.6).after(em(0.25))),
        with_paragraph(
            w::Style::new(CODE_BLOCK, w::StyleType::Paragraph)
                .name("Source Code")
                .based_on(NORMAL)
                .fonts(fonts(code_font))
                .size(half_points(size * 0.875))
                .line_spacing(lines(1.45).after(em(1.))),
            |property| {
                property
                    .shading(
                        w::Shading::new()
                            .shd_type(w::ShdType::Clear)
                            .fill(CODE_FILL),
                    )
                    .set_borders(box_borders(BORDER_COLOR))
            },
        ),
        w::Style::new(HTML_SOURCE, w::StyleType::Paragraph)
            .name("HTML Source")
            .based_on(NORMAL)
            .fonts(fonts(code_font))
            .size(half_points(size * 0.875))
            .color(MUTED_COLOR)
            .line_spacing(lines(1.45).after(em(1.))),
        w::Style::new(INLINE_CODE, w::StyleType::Character)
            .name("Verbatim Char")
            .fonts(fonts(code_font))
            .size(half_points(size * 0.875)),
        w::Style::new(HYPERLINK, w::StyleType::Character)
            .name("Hyperlink")
            .color(LINK_COLOR)
            .underline("single"),
        with_paragraph(
            w::Style::new(QUOTE, w::StyleType::Paragraph)
                .name("Block Text")
                .based_on(NORMAL)
                .color(SECONDARY_COLOR)
                .indent(Some(LIST_INDENT), None, None, None),
            |property| {
                property.set_border(
                    w::ParagraphBorder::new(w::ParagraphBorderPosition::Left)
                        .val(w::BorderType::Single)
                        .size(18)
                        .space(8)
                        .color(QUOTE_BORDER_COLOR),
                )
            },
        ),
        w::Style::new(TABLE_TEXT, w::StyleType::Paragraph)
            .name("Table Text")
            .based_on(NORMAL)
            .line_spacing(lines(1.4).before(em(0.2)).after(em(0.2))),
        w::Style::new(TABLE_HEADER, w::StyleType::Paragraph)
            .name("Table Header")
            .based_on(TABLE_TEXT)
            .bold(),
        w::Style::new(FOOTNOTE, w::StyleType::Paragraph)
            .name("Footnote Text")
            .based_on(NORMAL)
            .size(half_points(size * 0.875))
            .color(SECONDARY_COLOR),
        with_paragraph(
            w::Style::new(RULE, w::StyleType::Paragraph)
                .name("Horizontal Rule")
                .based_on(NORMAL)
                .size(4)
                .line_spacing(w::LineSpacing::new().before(em(1.)).after(em(1.5))),
            |property| {
                property.set_border(
                    w::ParagraphBorder::new(w::ParagraphBorderPosition::Bottom)
                        .val(w::BorderType::Single)
                        .size(6)
                        .space(1)
                        .color(BORDER_COLOR),
                )
            },
        ),
    ];
    for (ix, scale) in HEADING_SCALES.into_iter().enumerate() {
        let level = ix + 1;
        let heading_size = size * scale;
        styles.push(with_paragraph(
            w::Style::new(format!("Heading{level}"), w::StyleType::Paragraph)
                .name(format!("heading {level}"))
                .based_on(NORMAL)
                .next(NORMAL)
                .q_format(true)
                .bold()
                .size(half_points(heading_size))
                .color(if level == 6 {
                    SECONDARY_COLOR
                } else {
                    HEADING_COLOR
                })
                .outline_lvl(ix)
                .line_spacing(
                    lines(1.25)
                        .before(twips(heading_size * 1.2) as u32)
                        .after(twips(heading_size * 0.4) as u32),
                ),
            |property| property.keep_next(true).keep_lines(true),
        ));
    }
    styles
}

fn with_paragraph(
    mut style: w::Style,
    update: impl FnOnce(w::ParagraphProperty) -> w::ParagraphProperty,
) -> w::Style {
    style.paragraph_property = update(style.paragraph_property);
    style
}

fn box_borders(color: &str) -> w::ParagraphBorders {
    let mut borders = w::ParagraphBorders::with_empty();
    for position in [
        w::ParagraphBorderPosition::Top,
        w::ParagraphBorderPosition::Bottom,
        w::ParagraphBorderPosition::Left,
        w::ParagraphBorderPosition::Right,
    ] {
        borders = borders.set(
            w::ParagraphBorder::new(position)
                .val(w::BorderType::Single)
                .size(4)
                .space(6)
                .color(color),
        );
    }
    borders
}

fn fonts(family: &str) -> w::RunFonts {
    w::RunFonts::new()
        .ascii(family)
        .hi_ansi(family)
        .east_asia(family)
        .cs(family)
}

fn list_indent(level: usize) -> i32 {
    LIST_INDENT * (level as i32 + 1) + LIST_INDENT
}

fn twips(points: f32) -> i32 {
    (points * 20.).round() as i32
}

fn half_points(points: f32) -> usize {
    (points * 2.).round().max(2.) as usize
}

/// `2026-10-17T09:30:00Z`: the date format Word's document properties use.
fn w3c_timestamp(time: std::time::SystemTime) -> String {
    let seconds = time
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_secs() as i64);
    let (days, rest) = (seconds.div_euclid(86_400), seconds.rem_euclid(86_400));
    // Civil date from days since 1970-01-01, after Howard Hinnant's
    // `civil_from_days`.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rest / 3600,
        rest % 3600 / 60,
        rest % 60
    )
}

/// English Metric Units for a length in 96-dpi pixels.
fn emu(pixels: f32) -> u32 {
    (pixels.max(1.) * 9525.).round() as u32
}

/// A link target Word accepts: it rejects a document whose relationships
/// hold characters a URI can't, such as spaces or raw non-ASCII text.
fn encode_uri(url: &str) -> String {
    let mut encoded = String::with_capacity(url.len());
    for byte in url.bytes() {
        let keep = byte.is_ascii_alphanumeric() || b"-._~:/?#[]@!$&'()*+,;=%".contains(&byte);
        if keep {
            encoded.push(byte as char);
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

#[cfg(test)]
mod tests {
    use std::io::Read as _;

    use super::*;

    fn export(markdown: &str, style: &ExportStyle) -> Vec<u8> {
        render_docx(
            &ExportSource {
                markdown,
                title: "Notes",
                base_dir: None,
            },
            style,
        )
        .unwrap()
    }

    fn part(docx: &[u8], name: &str) -> String {
        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(docx)).unwrap();
        let mut file = archive.by_name(name).unwrap();
        let mut xml = String::new();
        file.read_to_string(&mut xml).unwrap();
        xml
    }

    #[test]
    fn sets_the_configured_fonts_and_size() {
        let style = ExportStyle {
            text_font: Some("Iowan Old Style".into()),
            text_size_px: 18.,
            code_font: Some("JetBrains Mono".into()),
            ..ExportStyle::default()
        };
        let styles = part(&export("Text and `code`", &style), "word/styles.xml");
        assert!(styles.contains("w:ascii=\"Iowan Old Style\""));
        assert!(styles.contains("w:ascii=\"JetBrains Mono\""));
        // 18 px is 13.5 pt, 27 half-points.
        assert!(styles.contains("<w:sz w:val=\"27\" />"));
    }

    #[test]
    fn names_word_fonts_for_the_system_fonts() {
        let styles = part(&export("Text", &ExportStyle::default()), "word/styles.xml");
        assert!(styles.contains("w:ascii=\"Helvetica Neue\""));
        assert!(styles.contains("w:ascii=\"Menlo\""));
    }

    #[test]
    fn maps_headings_to_word_heading_styles() {
        let docx = export("# One\n\n### Three", &ExportStyle::default());
        let document = part(&docx, "word/document.xml");
        assert!(document.contains("<w:pStyle w:val=\"Heading1\" />"));
        assert!(document.contains("<w:pStyle w:val=\"Heading3\" />"));
        let styles = part(&docx, "word/styles.xml");
        assert!(styles.contains("<w:name w:val=\"heading 1\" />"));
    }

    #[test]
    fn numbers_lists_from_their_start() {
        let docx = export(
            "3. three\n4. four\n\n- bullet\n  1. nested\n",
            &ExportStyle::default(),
        );
        let numbering = part(&docx, "word/numbering.xml");
        assert!(numbering.contains("<w:start w:val=\"3\" />"));
        assert!(numbering.contains("w:val=\"decimal\""));
        assert!(numbering.contains("w:val=\"bullet\""));
        let document = part(&docx, "word/document.xml");
        assert!(document.contains("<w:ilvl w:val=\"1\" />"));
    }

    #[test]
    fn keeps_code_lines_and_spaces() {
        let document = part(
            &export(
                "```\nfn main() {\n    run();\n}\n```",
                &ExportStyle::default(),
            ),
            "word/document.xml",
        );
        assert!(document.contains("<w:pStyle w:val=\"SourceCode\" />"));
        assert!(document.contains("xml:space=\"preserve\">    run();</w:t>"));
        assert_eq!(
            document.matches("<w:br w:type=\"textWrapping\" />").count(),
            2
        );
    }

    #[test]
    fn escapes_text_and_drops_characters_xml_cannot_hold() {
        let document = part(
            &export("a < b & \"c\" \u{0B}done", &ExportStyle::default()),
            "word/document.xml",
        );
        assert!(document.contains("a &lt; b &amp; &quot;c&quot; done"));
        assert!(!document.contains('\u{0B}'));
    }

    #[test]
    fn links_and_tables() {
        let docx = export(
            "[a link](<https://example.com/a b?q=ü>)\n\n| a | b |\n|---|--:|\n| 1 |  |\n",
            &ExportStyle::default(),
        );
        let rels = part(&docx, "word/_rels/document.xml.rels");
        assert!(rels.contains("https://example.com/a%20b?q=%C3%BC"));
        let document = part(&docx, "word/document.xml");
        assert!(document.contains("<w:tbl>"));
        assert!(document.contains("<w:jc w:val=\"right\" />"));
        // Every cell, the empty one too, holds a paragraph.
        let cells: Vec<&str> = document.split("<w:tc>").skip(1).collect();
        assert_eq!(cells.len(), 4);
        assert!(cells.iter().all(|cell| cell.contains("<w:p ")));
    }

    #[test]
    fn embeds_local_images_and_describes_others() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("dot.png"), images::tests::png()).unwrap();
        let docx = render_docx(
            &ExportSource {
                markdown: "![dot](dot.png)\n\n![missing image](gone.png)",
                title: "Images",
                base_dir: Some(dir.path()),
            },
            &ExportStyle::default(),
        )
        .unwrap();
        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(&docx)).unwrap();
        let media = (0..archive.len())
            .filter(|ix| {
                let name = archive.by_index(*ix).unwrap().name().to_string();
                name.starts_with("word/media/") && name.ends_with(".png")
            })
            .count();
        assert_eq!(media, 1);
        let document = part(&docx, "word/document.xml");
        assert!(document.contains("missing image"));
    }

    #[test]
    fn formats_document_dates() {
        let at = |seconds| std::time::UNIX_EPOCH + std::time::Duration::from_secs(seconds);
        assert_eq!(w3c_timestamp(at(0)), "1970-01-01T00:00:00Z");
        assert_eq!(w3c_timestamp(at(951_782_400)), "2000-02-29T00:00:00Z");
        assert_eq!(w3c_timestamp(at(1_792_222_245)), "2026-10-17T07:30:45Z");
    }

    #[test]
    fn keeps_a_footnote_label_with_its_text() {
        let document = part(
            &export("Note[^a].\n\n[^a]: The note.", &ExportStyle::default()),
            "word/document.xml",
        );
        let definition = document
            .split("<w:p ")
            .find(|paragraph| paragraph.contains("The note."))
            .unwrap();
        assert!(definition.contains("[a] "));
    }

    #[test]
    fn handles_every_construct_without_panicking() {
        let markdown = "---\na: 1\n---\n\n# H\n\n> quote\n> > nested\n\n- [x] done\n- [ ] todo\n-\n- - deep\n\n1. a\n\n   para\n\n***bold italic*** ~~s~~ line  \nbreak <br> [^n]\n\n[^n]: note\n\n<div>html</div>\n\n---\n\n| x |\n|---|\n";
        let docx = export(markdown, &ExportStyle::default());
        let document = part(&docx, "word/document.xml");
        assert!(document.contains("\u{2611}"));
        assert!(document.contains("&lt;div&gt;html&lt;/div&gt;"));
    }
}
