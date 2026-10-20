use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::iter::Peekable;
use std::ops::Range;
use std::sync::Arc;

use pulldown_cmark::{
    Alignment, CodeBlockKind, Event, HeadingLevel, OffsetIter, Options, Parser, Tag, TagEnd,
};

use crate::inline::{InlineBuilder, InlineStyle, InlineText};

/// A parsed Markdown document.
#[derive(Clone, Debug, Default)]
pub struct MarkdownDocument {
    blocks: Vec<Block>,
    stats: DocumentStats,
}

impl MarkdownDocument {
    /// Parse `source` as CommonMark with the GitHub extensions people expect
    /// from a Markdown editor: tables, task lists, strikethrough, footnotes
    /// and YAML front matter.
    pub fn parse(source: &str) -> Self {
        Self::parse_with_cancellation(source, || false)
            .expect("an uncancelled Markdown parse always returns a document")
    }

    /// Parse while periodically checking whether the caller still needs the
    /// result. Cancellation is checked between top-level blocks.
    pub fn parse_with_cancellation(source: &str, cancelled: impl Fn() -> bool) -> Option<Self> {
        if cancelled() {
            return None;
        }
        let options = Options::ENABLE_TABLES
            | Options::ENABLE_FOOTNOTES
            | Options::ENABLE_STRIKETHROUGH
            | Options::ENABLE_TASKLISTS
            | Options::ENABLE_YAML_STYLE_METADATA_BLOCKS
            | Options::ENABLE_GFM;
        let mut parser = BlockParser {
            events: Parser::new_ext(source, options)
                .into_offset_iter()
                .peekable(),
            pending_task: None,
        };

        let mut blocks = Vec::new();
        let mut occurrences = HashMap::<u64, u64>::new();
        while let Some((event, range)) = parser.events.next() {
            if cancelled() {
                return None;
            }
            let Some(kind) = parser.block_from_event(event) else {
                continue;
            };
            let fingerprint = fingerprint(&kind);
            let occurrence = occurrences.entry(fingerprint).or_default();
            let id = BlockId(fingerprint_of(&(fingerprint, *occurrence)));
            *occurrence += 1;
            blocks.push(Block {
                id,
                fingerprint,
                source_range: range,
                kind,
            });
        }

        if cancelled() {
            return None;
        }
        Some(Self {
            blocks,
            stats: DocumentStats::measure(source),
        })
    }

    pub fn blocks(&self) -> &[Block] {
        &self.blocks
    }

    pub fn stats(&self) -> DocumentStats {
        self.stats
    }

    /// The index of the block containing, or else preceding, a byte offset
    /// in the source.
    pub fn block_ix_for_offset(&self, offset: usize) -> Option<usize> {
        if self.blocks.is_empty() {
            return None;
        }
        let following = self
            .blocks
            .partition_point(|block| block.source_range.start <= offset);
        Some(following.saturating_sub(1))
    }
}

/// Counts over the source text.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DocumentStats {
    words: usize,
    lines: usize,
}

impl DocumentStats {
    fn measure(source: &str) -> Self {
        Self {
            words: crate::count_words(source),
            lines: source.lines().count().max(1),
        }
    }

    pub fn words(&self) -> usize {
        self.words
    }

    pub fn lines(&self) -> usize {
        self.lines
    }
}

/// Stable identity for a top-level block: its content plus how many earlier
/// blocks had the same content. Edits elsewhere in the document keep it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct BlockId(u64);

impl BlockId {
    pub fn as_u64(self) -> u64 {
        self.0
    }
}

/// One top-level block of a document.
#[derive(Clone, Debug)]
pub struct Block {
    id: BlockId,
    fingerprint: u64,
    source_range: Range<usize>,
    kind: BlockKind,
}

impl Block {
    pub fn id(&self) -> BlockId {
        self.id
    }

    /// A hash of the rendered content; equal fingerprints render identically.
    pub fn fingerprint(&self) -> u64 {
        self.fingerprint
    }

    /// The byte range of the source this block was parsed from.
    pub fn source_range(&self) -> Range<usize> {
        self.source_range.clone()
    }

    pub fn kind(&self) -> &BlockKind {
        &self.kind
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum BlockKind {
    Heading(Heading),
    Paragraph(InlineText),
    /// A paragraph that consists of a single image.
    Image(Image),
    Code(CodeBlock),
    Quote(Vec<BlockKind>),
    List(MarkdownList),
    Table(Table),
    Footnote {
        label: Arc<str>,
        children: Vec<BlockKind>,
    },
    Html(Arc<str>),
    Rule,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Heading {
    level: u8,
    text: InlineText,
}

impl Heading {
    /// 1 through 6.
    pub fn level(&self) -> u8 {
        self.level
    }

    pub fn text(&self) -> &InlineText {
        &self.text
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Image {
    url: Arc<str>,
    alt: Arc<str>,
}

impl Image {
    pub fn url(&self) -> &Arc<str> {
        &self.url
    }

    pub fn alt(&self) -> &Arc<str> {
        &self.alt
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct CodeBlock {
    language: Option<Arc<str>>,
    code: Arc<str>,
}

impl CodeBlock {
    pub fn language(&self) -> Option<&Arc<str>> {
        self.language.as_ref()
    }

    pub fn code(&self) -> &Arc<str> {
        &self.code
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct MarkdownList {
    start: Option<u64>,
    items: Vec<ListItem>,
}

impl MarkdownList {
    /// The first number of an ordered list; `None` for a bulleted list.
    pub fn start(&self) -> Option<u64> {
        self.start
    }

    pub fn items(&self) -> &[ListItem] {
        &self.items
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ListItem {
    task: Option<bool>,
    children: Vec<BlockKind>,
    source_range: Range<usize>,
}

impl Hash for ListItem {
    fn hash<H: Hasher>(&self, state: &mut H) {
        // Moving a list in the source does not change its rendered identity.
        self.task.hash(state);
        self.children.hash(state);
    }
}

impl ListItem {
    /// `Some(checked)` for a task list item.
    pub fn task(&self) -> Option<bool> {
        self.task
    }

    pub fn children(&self) -> &[BlockKind] {
        &self.children
    }

    /// Source bytes occupied by this item, including its marker and children.
    pub fn source_range(&self) -> Range<usize> {
        self.source_range.clone()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ColumnAlignment {
    None,
    Left,
    Center,
    Right,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Table {
    alignments: Vec<ColumnAlignment>,
    header: Vec<InlineText>,
    rows: Vec<Vec<InlineText>>,
}

impl Table {
    pub fn alignments(&self) -> &[ColumnAlignment] {
        &self.alignments
    }

    pub fn header(&self) -> &[InlineText] {
        &self.header
    }

    pub fn rows(&self) -> &[Vec<InlineText>] {
        &self.rows
    }

    pub fn column_count(&self) -> usize {
        self.alignments.len()
    }
}

fn fingerprint(kind: &BlockKind) -> u64 {
    fingerprint_of(kind)
}

fn fingerprint_of(value: &impl Hash) -> u64 {
    let mut hasher = DefaultHasher::new();
    value.hash(&mut hasher);
    hasher.finish()
}

/// Recursive descent over pulldown-cmark's event stream.
struct BlockParser<'a> {
    events: Peekable<OffsetIter<'a>>,
    /// A task marker seen inside the list item being parsed.
    pending_task: Option<bool>,
}

impl<'a> BlockParser<'a> {
    /// Build the block that `event` opens, consuming events through its end.
    fn block_from_event(&mut self, event: Event<'a>) -> Option<BlockKind> {
        match event {
            Event::Start(tag) => self.block_from_tag(tag),
            Event::Rule => Some(BlockKind::Rule),
            Event::Html(html) => Some(BlockKind::Html(html.trim_end().into())),
            Event::TaskListMarker(checked) => {
                self.pending_task = Some(checked);
                None
            }
            _ => None,
        }
    }

    fn block_from_tag(&mut self, tag: Tag<'a>) -> Option<BlockKind> {
        match tag {
            Tag::Paragraph => {
                let inline = self.collect_inline_until(TagEnd::Paragraph);
                Some(paragraph_or_image(inline))
            }
            Tag::Heading { level, .. } => Some(BlockKind::Heading(Heading {
                level: heading_level(level),
                text: self.collect_inline_until(TagEnd::Heading(level)).finish(),
            })),
            Tag::CodeBlock(kind) => {
                let language = match kind {
                    CodeBlockKind::Fenced(info) => info
                        .split(|c: char| c.is_whitespace() || c == ',' || c == '{')
                        .next()
                        .filter(|lang| !lang.is_empty())
                        .map(Arc::from),
                    CodeBlockKind::Indented => None,
                };
                Some(BlockKind::Code(CodeBlock {
                    language,
                    code: self.collect_text_until(TagEnd::CodeBlock),
                }))
            }
            Tag::MetadataBlock(kind) => Some(BlockKind::Code(CodeBlock {
                language: Some("yaml".into()),
                code: self.collect_text_until(TagEnd::MetadataBlock(kind)),
            })),
            Tag::HtmlBlock => Some(BlockKind::Html(self.collect_text_until(TagEnd::HtmlBlock))),
            Tag::BlockQuote(kind) => Some(BlockKind::Quote(
                self.collect_blocks_until(TagEnd::BlockQuote(kind)),
            )),
            Tag::List(start) => Some(BlockKind::List(MarkdownList {
                start,
                items: self.collect_list_items(TagEnd::List(start.is_some())),
            })),
            Tag::Table(alignments) => Some(BlockKind::Table(self.collect_table(alignments))),
            Tag::FootnoteDefinition(label) => Some(BlockKind::Footnote {
                label: label.as_ref().into(),
                children: self.collect_blocks_until(TagEnd::FootnoteDefinition),
            }),
            other => {
                // A tag this renderer has no block for: skip its contents.
                self.skip_until(other.to_end());
                None
            }
        }
    }

    /// Child blocks of a container. Loose inline content directly inside the
    /// container, as in a tight list item, becomes an implicit paragraph.
    fn collect_blocks_until(&mut self, end: TagEnd) -> Vec<BlockKind> {
        let mut children = Vec::new();
        loop {
            if self.next_is_inline() {
                let inline = self.collect_loose_inline();
                if !inline.is_empty() {
                    children.push(paragraph_or_image(inline));
                }
                continue;
            }
            let Some((event, _)) = self.events.next() else {
                break;
            };
            if matches!(&event, Event::End(tag_end) if *tag_end == end) {
                break;
            }
            if let Some(block) = self.block_from_event(event) {
                children.push(block);
            }
        }
        children
    }

    fn collect_list_items(&mut self, end: TagEnd) -> Vec<ListItem> {
        let mut items = Vec::new();
        while let Some((event, source_range)) = self.events.next() {
            match event {
                Event::Start(Tag::Item) => {
                    // A marker already seen belongs to the enclosing item.
                    let enclosing_task = self.pending_task.take();
                    let children = self.collect_blocks_until(TagEnd::Item);
                    let task = self.pending_task.take();
                    self.pending_task = enclosing_task;
                    items.push(ListItem {
                        task,
                        children,
                        source_range,
                    });
                }
                Event::End(tag_end) if tag_end == end => break,
                _ => {}
            }
        }
        items
    }

    fn collect_table(&mut self, alignments: Vec<Alignment>) -> Table {
        let mut table = Table {
            alignments: alignments
                .into_iter()
                .map(|alignment| match alignment {
                    Alignment::None => ColumnAlignment::None,
                    Alignment::Left => ColumnAlignment::Left,
                    Alignment::Center => ColumnAlignment::Center,
                    Alignment::Right => ColumnAlignment::Right,
                })
                .collect(),
            header: Vec::new(),
            rows: Vec::new(),
        };
        let mut in_head = false;
        while let Some((event, _)) = self.events.next() {
            match event {
                Event::Start(Tag::TableHead) => in_head = true,
                Event::End(TagEnd::TableHead) => in_head = false,
                Event::Start(Tag::TableRow) => table.rows.push(Vec::new()),
                Event::Start(Tag::TableCell) => {
                    let cell = self.collect_inline_until(TagEnd::TableCell).finish();
                    if in_head {
                        table.header.push(cell);
                    } else if let Some(row) = table.rows.last_mut() {
                        row.push(cell);
                    }
                }
                Event::End(TagEnd::Table) => break,
                _ => {}
            }
        }
        table
    }

    fn collect_inline_until(&mut self, end: TagEnd) -> InlineBuilder {
        let mut inline = InlineBuilder::default();
        while let Some((event, _)) = self.events.next() {
            if matches!(&event, Event::End(tag_end) if *tag_end == end) {
                break;
            }
            self.push_inline(&mut inline, event);
        }
        inline
    }

    /// Inline events up to, but not including, the next block-level event.
    fn collect_loose_inline(&mut self) -> InlineBuilder {
        let mut inline = InlineBuilder::default();
        while self.next_is_inline() {
            let Some((event, _)) = self.events.next() else {
                break;
            };
            self.push_inline(&mut inline, event);
        }
        inline
    }

    fn next_is_inline(&mut self) -> bool {
        let Some((event, _)) = self.events.peek() else {
            return false;
        };
        match event {
            Event::Text(_)
            | Event::Code(_)
            | Event::InlineMath(_)
            | Event::InlineHtml(_)
            | Event::FootnoteReference(_)
            | Event::SoftBreak
            | Event::HardBreak
            | Event::TaskListMarker(_) => true,
            Event::Start(tag) => is_inline_tag(tag),
            Event::End(tag_end) => is_inline_tag_end(tag_end),
            _ => false,
        }
    }

    fn push_inline(&mut self, inline: &mut InlineBuilder, event: Event<'a>) {
        match event {
            Event::Text(text) => inline.push_text(&text),
            Event::Code(code) | Event::InlineMath(code) => inline.push_code(&code),
            Event::InlineHtml(html) => inline.push_code(&html),
            Event::FootnoteReference(label) => {
                inline.push_style(InlineStyle::LINK);
                inline.push_text(&format!("[{label}]"));
                inline.pop_style(InlineStyle::LINK);
            }
            // A single newline breaks the line, as in Obsidian and Typora,
            // rather than joining the lines as CommonMark does.
            Event::SoftBreak | Event::HardBreak => inline.push_hard_break(),
            Event::TaskListMarker(checked) => self.pending_task = Some(checked),
            Event::Start(Tag::Emphasis) => inline.push_style(InlineStyle::EMPHASIS),
            Event::End(TagEnd::Emphasis) => inline.pop_style(InlineStyle::EMPHASIS),
            Event::Start(Tag::Strong) => inline.push_style(InlineStyle::STRONG),
            Event::End(TagEnd::Strong) => inline.pop_style(InlineStyle::STRONG),
            Event::Start(Tag::Strikethrough) => inline.push_style(InlineStyle::STRIKETHROUGH),
            Event::End(TagEnd::Strikethrough) => inline.pop_style(InlineStyle::STRIKETHROUGH),
            Event::Start(Tag::Link { dest_url, .. }) => inline.start_link(&dest_url),
            Event::End(TagEnd::Link) => inline.end_link(),
            Event::Start(Tag::Image { dest_url, .. }) => inline.start_image(&dest_url),
            Event::End(TagEnd::Image) => inline.end_image(),
            _ => {}
        }
    }

    fn collect_text_until(&mut self, end: TagEnd) -> Arc<str> {
        let mut text = String::new();
        for (event, _) in self.events.by_ref() {
            match event {
                Event::Text(chunk) | Event::Html(chunk) => text.push_str(&chunk),
                Event::End(tag_end) if tag_end == end => break,
                _ => {}
            }
        }
        if text.ends_with('\n') {
            text.pop();
        }
        text.into()
    }

    fn skip_until(&mut self, end: TagEnd) {
        let mut depth = 0usize;
        for (event, _) in self.events.by_ref() {
            match event {
                Event::Start(tag) if tag.to_end() == end => depth += 1,
                Event::End(tag_end) if tag_end == end => {
                    if depth == 0 {
                        break;
                    }
                    depth -= 1;
                }
                _ => {}
            }
        }
    }
}

fn paragraph_or_image(inline: InlineBuilder) -> BlockKind {
    match inline.sole_image() {
        Some((url, alt)) => BlockKind::Image(Image { url, alt }),
        None => BlockKind::Paragraph(inline.finish()),
    }
}

fn is_inline_tag(tag: &Tag) -> bool {
    matches!(
        tag,
        Tag::Emphasis
            | Tag::Strong
            | Tag::Strikethrough
            | Tag::Superscript
            | Tag::Subscript
            | Tag::Link { .. }
            | Tag::Image { .. }
    )
}

fn is_inline_tag_end(tag_end: &TagEnd) -> bool {
    matches!(
        tag_end,
        TagEnd::Emphasis
            | TagEnd::Strong
            | TagEnd::Strikethrough
            | TagEnd::Superscript
            | TagEnd::Subscript
            | TagEnd::Link
            | TagEnd::Image
    )
}

fn heading_level(level: HeadingLevel) -> u8 {
    match level {
        HeadingLevel::H1 => 1,
        HeadingLevel::H2 => 2,
        HeadingLevel::H3 => 3,
        HeadingLevel::H4 => 4,
        HeadingLevel::H5 => 5,
        HeadingLevel::H6 => 6,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(source: &str) -> Vec<BlockKind> {
        MarkdownDocument::parse(source)
            .blocks()
            .iter()
            .map(|block| block.kind().clone())
            .collect()
    }

    fn paragraph(kind: &BlockKind) -> &InlineText {
        match kind {
            BlockKind::Paragraph(text) => text,
            other => panic!("expected a paragraph, got {other:?}"),
        }
    }

    #[test]
    fn splits_top_level_blocks_with_source_ranges() {
        let source = "# Title\n\nSome *text*.\n\n---\n";
        let document = MarkdownDocument::parse(source);
        let blocks = document.blocks();
        assert_eq!(blocks.len(), 3);
        assert_eq!(&source[blocks[0].source_range()], "# Title\n");
        assert!(matches!(blocks[2].kind(), BlockKind::Rule));
    }

    #[test]
    fn a_single_newline_breaks_the_line() {
        let kinds = kinds("first line\nsecond line  \nthird line");
        assert_eq!(
            &**paragraph(&kinds[0]).text(),
            "first line\nsecond line\nthird line"
        );
    }

    #[test]
    fn inline_styles_are_flattened_into_runs() {
        let kinds = kinds("plain **bold *both*** `code` [link](https://x.dev)");
        let text = paragraph(&kinds[0]);
        assert_eq!(&**text.text(), "plain bold both code link");
        let styles: Vec<_> = text
            .runs()
            .iter()
            .map(|run| (&text.text()[run.range()], run.style()))
            .collect();
        assert_eq!(styles[0], ("bold ", InlineStyle::STRONG));
        assert!(styles[1].1.contains(InlineStyle::STRONG));
        assert!(styles[1].1.contains(InlineStyle::EMPHASIS));
        assert_eq!(styles[2], ("code", InlineStyle::CODE));
        assert_eq!(styles[3], ("link", InlineStyle::LINK));
        assert_eq!(&**text.links()[0].url(), "https://x.dev");
    }

    #[test]
    fn tight_and_task_lists_keep_their_content() {
        let kinds = kinds("- [x] done\n- [ ] todo\n  - nested\n");
        let BlockKind::List(list) = &kinds[0] else {
            panic!("expected a list");
        };
        assert_eq!(list.start(), None);
        assert_eq!(list.items().len(), 2);
        assert_eq!(list.items()[0].task(), Some(true));
        assert_eq!(list.items()[1].task(), Some(false));
        assert_eq!(&**paragraph(&list.items()[0].children()[0]).text(), "done");
        assert!(matches!(list.items()[1].children()[1], BlockKind::List(_)));
    }

    #[test]
    fn ordered_lists_keep_their_start() {
        let kinds = kinds("3. three\n4. four\n");
        let BlockKind::List(list) = &kinds[0] else {
            panic!("expected a list");
        };
        assert_eq!(list.start(), Some(3));
    }

    #[test]
    fn fenced_code_keeps_language_and_drops_trailing_newline() {
        let kinds = kinds("```rust title=x\nfn main() {}\n```\n");
        let BlockKind::Code(code) = &kinds[0] else {
            panic!("expected code");
        };
        assert_eq!(code.language().map(|l| &**l), Some("rust"));
        assert_eq!(&**code.code(), "fn main() {}");
    }

    #[test]
    fn tables_split_header_and_rows() {
        let kinds = kinds("| a | b |\n|:--|--:|\n| 1 | 2 |\n| 3 | 4 |\n");
        let BlockKind::Table(table) = &kinds[0] else {
            panic!("expected a table");
        };
        assert_eq!(table.column_count(), 2);
        assert_eq!(table.alignments()[1], ColumnAlignment::Right);
        assert_eq!(&**table.header()[0].text(), "a");
        assert_eq!(table.rows().len(), 2);
        assert_eq!(&**table.rows()[1][1].text(), "4");
    }

    #[test]
    fn a_lone_image_becomes_an_image_block() {
        let kinds = kinds("![A cat](cat.png)\n\nText ![inline](x.png)\n");
        assert!(
            matches!(&kinds[0], BlockKind::Image(image) if &**image.url() == "cat.png" && &**image.alt() == "A cat")
        );
        assert!(matches!(&kinds[1], BlockKind::Paragraph(_)));
    }

    #[test]
    fn front_matter_renders_as_yaml() {
        let kinds = kinds("---\ntitle: Notes\n---\n\nBody\n");
        assert!(
            matches!(&kinds[0], BlockKind::Code(code) if code.language().map(|l| &**l) == Some("yaml"))
        );
    }

    #[test]
    fn identical_blocks_get_distinct_ids() {
        let document = MarkdownDocument::parse("same\n\nsame\n");
        let [first, second] = document.blocks() else {
            panic!("expected two blocks");
        };
        assert_eq!(first.fingerprint(), second.fingerprint());
        assert_ne!(first.id(), second.id());
    }

    #[test]
    fn maps_source_offsets_to_blocks() {
        let source = "# A\n\npara\n\n- item\n";
        let document = MarkdownDocument::parse(source);
        assert_eq!(document.block_ix_for_offset(0), Some(0));
        assert_eq!(
            document.block_ix_for_offset(source.find("para").unwrap() + 2),
            Some(1)
        );
        assert_eq!(document.block_ix_for_offset(source.len()), Some(2));
        assert_eq!(MarkdownDocument::parse("").block_ix_for_offset(0), None);
    }

    #[test]
    fn counts_words_and_lines() {
        let stats = MarkdownDocument::parse("one two\nthree\n").stats();
        assert_eq!((stats.words(), stats.lines()), (3, 2));
    }
}
