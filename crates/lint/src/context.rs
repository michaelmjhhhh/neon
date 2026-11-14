//! One parse of a document, shared by every rule.

use std::hash::{DefaultHasher, Hash, Hasher};
use std::ops::Range;

use pulldown_cmark::{
    Alignment, CodeBlockKind, Event, HeadingLevel, LinkType, Options, Parser, Tag, TagEnd,
};

use crate::suppress::Suppressions;

/// The dialect Neon reads, as in `markdown::PARSER_OPTIONS`, plus math so
/// `$…$` and `$$…$$` are left alone.
const OPTIONS: Options = Options::ENABLE_TABLES
    .union(Options::ENABLE_FOOTNOTES)
    .union(Options::ENABLE_STRIKETHROUGH)
    .union(Options::ENABLE_TASKLISTS)
    .union(Options::ENABLE_YAML_STYLE_METADATA_BLOCKS)
    .union(Options::ENABLE_GFM)
    .union(Options::ENABLE_WIKILINKS)
    .union(Options::ENABLE_MATH);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BlockKind {
    Heading,
    FencedCode,
    IndentedCode,
    Table,
    BlockQuote,
    Rule,
    Metadata,
    Other,
}

/// A block at the top level of the document, outside any list or quote.
#[derive(Clone, Debug)]
pub(crate) struct Block {
    pub kind: BlockKind,
    pub range: Range<usize>,
}

#[derive(Clone, Debug)]
pub(crate) struct Heading {
    pub level: u8,
    pub range: Range<usize>,
    /// The heading's last inline piece of text, if it ends the heading.
    pub last_text: Option<Range<usize>>,
}

#[derive(Clone, Debug)]
pub(crate) struct List {
    /// The first number of an ordered list.
    pub start: Option<u64>,
    /// Where each item's marker starts.
    pub items: Vec<usize>,
}

#[derive(Clone, Debug)]
pub(crate) struct Emphasis {
    pub range: Range<usize>,
    pub strong: bool,
}

#[derive(Clone, Debug)]
pub(crate) struct Fence {
    pub range: Range<usize>,
    pub info_empty: bool,
}

#[derive(Clone, Debug)]
pub(crate) struct Table {
    pub range: Range<usize>,
    pub alignments: Vec<Alignment>,
    /// Each row's range and its cells' ranges; the header first.
    pub rows: Vec<(Range<usize>, Vec<Range<usize>>)>,
    pub top_level: bool,
}

/// The line structure of a source.
#[derive(Clone, Debug)]
pub(crate) struct Lines {
    starts: Vec<usize>,
    len: usize,
}

impl Lines {
    pub fn new(source: &str) -> Self {
        let mut starts = Vec::with_capacity(source.len() / 32 + 1);
        starts.push(0);
        starts.extend(
            source
                .bytes()
                .enumerate()
                .filter(|(_, byte)| *byte == b'\n')
                .map(|(ix, _)| ix + 1),
        );
        Self {
            starts,
            len: source.len(),
        }
    }

    /// Lines holding text; a final line ending does not start another.
    pub fn count(&self) -> usize {
        if self.len == 0 {
            0
        } else if *self.starts.last().unwrap() == self.len {
            self.starts.len() - 1
        } else {
            self.starts.len()
        }
    }

    pub fn of(&self, offset: usize) -> usize {
        self.starts.partition_point(|start| *start <= offset) - 1
    }

    pub fn start(&self, line: usize) -> usize {
        self.starts[line]
    }

    /// The line with its line ending.
    pub fn full(&self, line: usize) -> Range<usize> {
        let end = self.starts.get(line + 1).copied().unwrap_or(self.len);
        self.starts[line]..end
    }

    /// The line without its line ending.
    pub fn content(&self, source: &str, line: usize) -> Range<usize> {
        let Range { start, mut end } = self.full(line);
        let bytes = source.as_bytes();
        if end > start && bytes[end - 1] == b'\n' {
            end -= 1;
            if end > start && bytes[end - 1] == b'\r' {
                end -= 1;
            }
        }
        start..end
    }

    pub fn is_blank(&self, source: &str, line: usize) -> bool {
        source[self.content(source, line)]
            .bytes()
            .all(|byte| byte == b' ' || byte == b'\t')
    }
}

pub(crate) struct Context<'a> {
    pub source: &'a str,
    pub lines: Lines,
    /// The document's line ending, for inserted lines.
    pub newline: &'static str,
    /// Ranges no fix may touch, sorted and merged.
    pub protected: Vec<Range<usize>>,
    pub blocks: Vec<Block>,
    pub headings: Vec<Heading>,
    /// Runs of plain text, adjacent text events joined, outside code, math,
    /// HTML and links whose text is their target.
    pub texts: Vec<Range<usize>>,
    /// Hard line breaks written with trailing spaces.
    pub hard_breaks: Vec<Range<usize>>,
    pub lists: Vec<List>,
    pub emphasis: Vec<Emphasis>,
    pub fences: Vec<Fence>,
    pub tables: Vec<Table>,
    pub suppressions: Suppressions,
    /// Whether the file ends in code or HTML kept verbatim, which a line
    /// ending added at the end would change.
    pub ends_in_verbatim: bool,
    /// A hash of what fixes must keep: the tags, code, math, HTML and link
    /// targets. Text and blank lines are left out.
    pub signature: u64,
}

impl<'a> Context<'a> {
    pub fn parse(source: &'a str) -> Self {
        let lines = Lines::new(source);
        let newline = match source.find('\n') {
            Some(ix) if ix > 0 && source.as_bytes()[ix - 1] == b'\r' => "\r\n",
            _ => "\n",
        };
        let mut builder = Builder {
            source,
            protected: Vec::new(),
            blocks: Vec::new(),
            headings: Vec::new(),
            texts: Vec::new(),
            hard_breaks: Vec::new(),
            lists: Vec::new(),
            emphasis: Vec::new(),
            fences: Vec::new(),
            tables: Vec::new(),
            comments: Vec::new(),
            ends_in_verbatim: false,
            hasher: DefaultHasher::new(),
            depth: 0,
            verbatim: 0,
            opaque_links: 0,
            links: Vec::new(),
            open_lists: Vec::new(),
            open_heading: None,
            open_table: None,
        };
        let mut parser = Parser::new_ext(source, OPTIONS).into_offset_iter();
        for (event, range) in &mut parser {
            builder.event(event, range);
        }
        for (_, definition) in parser.reference_definitions().iter() {
            definition.dest.hash(&mut builder.hasher);
            builder.protected.push(definition.span.clone());
        }
        let Builder {
            mut protected,
            blocks,
            headings,
            texts,
            hard_breaks,
            lists,
            emphasis,
            fences,
            tables,
            comments,
            ends_in_verbatim,
            hasher,
            ..
        } = builder;
        protected.sort_unstable_by_key(|range| range.start);
        let protected = merge(protected);
        let suppressions = Suppressions::from_comments(source, &lines, &comments);
        Self {
            source,
            lines,
            newline,
            protected,
            blocks,
            headings,
            texts,
            hard_breaks,
            lists,
            emphasis,
            fences,
            tables,
            suppressions,
            ends_in_verbatim,
            signature: hasher.finish(),
        }
    }

    /// Whether an edit of `range` would touch protected text. An insertion
    /// at the edge of a protected range is allowed.
    pub fn is_protected(&self, range: &Range<usize>) -> bool {
        let ix = self
            .protected
            .partition_point(|protected| protected.end <= range.start);
        let Some(protected) = self.protected.get(ix) else {
            return false;
        };
        if range.is_empty() {
            protected.start < range.start
        } else {
            protected.start < range.end
        }
    }
}

fn merge(ranges: Vec<Range<usize>>) -> Vec<Range<usize>> {
    let mut merged: Vec<Range<usize>> = Vec::with_capacity(ranges.len());
    for range in ranges {
        match merged.last_mut() {
            Some(last) if range.start <= last.end => last.end = last.end.max(range.end),
            _ => merged.push(range),
        }
    }
    merged
}

struct OpenLink {
    start: usize,
    /// Where the link's text last ended, so the target after it is
    /// protected.
    text_end: usize,
    /// A wikilink or autolink, whose text is its target.
    opaque: bool,
}

struct Builder<'a> {
    source: &'a str,
    protected: Vec<Range<usize>>,
    blocks: Vec<Block>,
    headings: Vec<Heading>,
    texts: Vec<Range<usize>>,
    hard_breaks: Vec<Range<usize>>,
    lists: Vec<List>,
    emphasis: Vec<Emphasis>,
    fences: Vec<Fence>,
    tables: Vec<Table>,
    comments: Vec<Range<usize>>,
    ends_in_verbatim: bool,
    hasher: DefaultHasher,
    /// How many tags are open.
    depth: usize,
    /// Open code blocks, front matter and HTML blocks, whose text is kept
    /// verbatim.
    verbatim: usize,
    /// Open links whose text is their target: wikilinks and autolinks.
    opaque_links: usize,
    /// Open links and images.
    links: Vec<OpenLink>,
    /// Indexes into `lists` of the lists open around the current event.
    open_lists: Vec<usize>,
    open_heading: Option<usize>,
    open_table: Option<usize>,
}

impl Builder<'_> {
    fn event(&mut self, event: Event<'_>, range: Range<usize>) {
        if let Some(link) = self.links.last_mut()
            && !matches!(event, Event::End(TagEnd::Link | TagEnd::Image))
        {
            link.text_end = link.text_end.max(range.end);
        }
        // Only text that ends the heading counts as its last text: `# A *b*`
        // ends in emphasis.
        if let Some(ix) = self.open_heading
            && !matches!(event, Event::End(TagEnd::Heading(_)))
        {
            self.headings[ix].last_text = match event {
                Event::Text(_) => Some(range.clone()),
                _ => None,
            };
        }
        match event {
            Event::Start(tag) => {
                self.start(tag, range);
                self.depth += 1;
            }
            Event::End(end) => {
                self.depth -= 1;
                self.end(end, range);
            }
            Event::Text(text) => {
                if self.verbatim > 0 {
                    text.hash(&mut self.hasher);
                    self.ends_in_verbatim |= range.end == self.source.len();
                    // A fenced block's fences may change; its code may not.
                    self.protect(range);
                    return;
                }
                if self.opaque_links > 0 {
                    return;
                }
                match self.texts.last_mut() {
                    Some(last) if last.end == range.start => last.end = range.end,
                    _ => self.texts.push(range),
                }
            }
            Event::Code(code) | Event::InlineMath(code) | Event::DisplayMath(code) => {
                code.hash(&mut self.hasher);
                self.protect(range);
            }
            Event::Html(html) | Event::InlineHtml(html) => {
                html.hash(&mut self.hasher);
                if self.verbatim > 0 {
                    self.ends_in_verbatim |= range.end == self.source.len();
                }
                if html.trim_start().starts_with("<!--") {
                    self.comments.push(range.clone());
                }
                self.protect(range);
            }
            Event::FootnoteReference(label) => {
                label.hash(&mut self.hasher);
                self.protect(range);
            }
            Event::TaskListMarker(checked) => {
                checked.hash(&mut self.hasher);
                self.protect(range);
            }
            Event::HardBreak => {
                1u8.hash(&mut self.hasher);
                if self.source.as_bytes()[range.start] != b'\\' {
                    self.hard_breaks.push(range);
                }
            }
            Event::Rule => {
                2u8.hash(&mut self.hasher);
                if self.depth == 0 {
                    self.blocks.push(Block {
                        kind: BlockKind::Rule,
                        range,
                    });
                }
            }
            Event::SoftBreak => {}
        }
    }

    fn start(&mut self, tag: Tag<'_>, range: Range<usize>) {
        b's'.hash(&mut self.hasher);
        std::mem::discriminant(&tag).hash(&mut self.hasher);
        let top_level = self.depth == 0;
        let kind = match &tag {
            Tag::Heading { level, .. } => {
                (*level as u8).hash(&mut self.hasher);
                self.open_heading = Some(self.headings.len());
                self.headings.push(Heading {
                    level: heading_level(*level),
                    range: range.clone(),
                    last_text: None,
                });
                BlockKind::Heading
            }
            Tag::CodeBlock(kind) => {
                self.verbatim += 1;
                match kind {
                    CodeBlockKind::Fenced(info) => {
                        self.fences.push(Fence {
                            range: range.clone(),
                            info_empty: info.trim().is_empty(),
                        });
                        BlockKind::FencedCode
                    }
                    CodeBlockKind::Indented => {
                        self.ends_in_verbatim |= range.end == self.source.len();
                        self.protect(range.clone());
                        BlockKind::IndentedCode
                    }
                }
            }
            Tag::MetadataBlock(_) | Tag::HtmlBlock => {
                self.verbatim += 1;
                self.protect(range.clone());
                if matches!(tag, Tag::MetadataBlock(_)) {
                    BlockKind::Metadata
                } else {
                    BlockKind::Other
                }
            }
            Tag::BlockQuote(kind) => {
                kind.map(|kind| kind as u8).hash(&mut self.hasher);
                BlockKind::BlockQuote
            }
            Tag::List(start) => {
                start.hash(&mut self.hasher);
                self.open_lists.push(self.lists.len());
                self.lists.push(List {
                    start: *start,
                    items: Vec::new(),
                });
                BlockKind::Other
            }
            Tag::Item => {
                if let Some(&list) = self.open_lists.last() {
                    self.lists[list].items.push(range.start);
                }
                BlockKind::Other
            }
            Tag::Table(alignments) => {
                for alignment in alignments {
                    (*alignment as u8).hash(&mut self.hasher);
                }
                self.open_table = Some(self.tables.len());
                self.tables.push(Table {
                    range: range.clone(),
                    alignments: alignments.clone(),
                    rows: Vec::new(),
                    top_level,
                });
                BlockKind::Table
            }
            Tag::TableHead | Tag::TableRow => {
                if let Some(table) = self.open_table {
                    self.tables[table].rows.push((range.clone(), Vec::new()));
                }
                BlockKind::Other
            }
            Tag::TableCell => {
                if let Some(table) = self.open_table
                    && let Some(row) = self.tables[table].rows.last_mut()
                {
                    row.1.push(range.clone());
                }
                BlockKind::Other
            }
            Tag::Emphasis | Tag::Strong => {
                self.emphasis.push(Emphasis {
                    range: range.clone(),
                    strong: matches!(tag, Tag::Strong),
                });
                BlockKind::Other
            }
            Tag::Link {
                link_type,
                dest_url,
                title,
                ..
            }
            | Tag::Image {
                link_type,
                dest_url,
                title,
                ..
            } => {
                std::mem::discriminant(link_type).hash(&mut self.hasher);
                dest_url.hash(&mut self.hasher);
                title.hash(&mut self.hasher);
                let opaque = matches!(
                    link_type,
                    LinkType::WikiLink { .. } | LinkType::Autolink | LinkType::Email
                );
                if opaque {
                    self.opaque_links += 1;
                    self.protect(range.clone());
                }
                self.links.push(OpenLink {
                    start: range.start,
                    text_end: range.start + 1,
                    opaque,
                });
                BlockKind::Other
            }
            Tag::FootnoteDefinition(label) => {
                label.hash(&mut self.hasher);
                BlockKind::Other
            }
            _ => BlockKind::Other,
        };
        if top_level {
            self.blocks.push(Block { kind, range });
        }
    }

    fn end(&mut self, end: TagEnd, range: Range<usize>) {
        b'e'.hash(&mut self.hasher);
        std::mem::discriminant(&end).hash(&mut self.hasher);
        match end {
            TagEnd::Heading(_) => self.open_heading = None,
            TagEnd::CodeBlock | TagEnd::MetadataBlock(_) | TagEnd::HtmlBlock => {
                self.verbatim -= 1;
            }
            TagEnd::List(_) => {
                self.open_lists.pop();
            }
            TagEnd::Table => self.open_table = None,
            TagEnd::Link | TagEnd::Image => {
                if let Some(link) = self.links.pop() {
                    if link.opaque {
                        // Protected whole as it started.
                        self.opaque_links -= 1;
                    } else {
                        // `[text](target)`: what follows the text is the
                        // target.
                        let target_start = link.text_end.max(link.start + 1).min(range.end);
                        self.protect(target_start..range.end);
                    }
                }
            }
            _ => {}
        }
    }

    fn protect(&mut self, range: Range<usize>) {
        if !range.is_empty() {
            self.protected.push(range);
        }
    }
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
