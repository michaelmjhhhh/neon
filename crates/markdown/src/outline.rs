//! A document's headings: for an outline beside the document, a `[TOC]`
//! block inside it, and links that point at a heading by its `#anchor`.
//!
//! Headings come from the parser, so a `#` line inside a code block or an
//! HTML block is not one, and a setext heading (text underlined with `===`
//! or `---`) is. Each heading gets the anchor GitHub gives it, so a link
//! written for GitHub, such as `[Setup](#getting-started)`, finds the same
//! heading here and in an exported page.

use std::collections::HashMap;
use std::ops::Range;
use std::sync::Arc;

/// One heading of a document.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutlineHeading {
    level: u8,
    text: Arc<str>,
    anchor: Arc<str>,
    source_range: Range<usize>,
}

impl OutlineHeading {
    /// 1 through 6.
    pub fn level(&self) -> u8 {
        self.level
    }

    /// The heading as the preview shows it, on one line: `## **Set** up`
    /// reads `Set up`.
    pub fn text(&self) -> &Arc<str> {
        &self.text
    }

    /// The fragment that links to the heading, without its `#`: unique in
    /// the document, and the one GitHub uses.
    pub fn anchor(&self) -> &Arc<str> {
        &self.anchor
    }

    /// The bytes of the source the heading was parsed from, through the end
    /// of its last line.
    pub fn source_range(&self) -> Range<usize> {
        self.source_range.clone()
    }
}

/// A document's headings, in the order they appear. Headings with no text,
/// such as a lone `#`, are left out: there is nothing to show or link to.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Outline {
    headings: Vec<OutlineHeading>,
}

impl Outline {
    /// The headings of `source`, read as [`crate::MarkdownDocument::parse`]
    /// reads it. Cheaper than a full parse: only headings are built.
    pub fn parse(source: &str) -> Self {
        Self::parse_with_cancellation(source, || false)
            .expect("an uncancelled outline parse always returns an outline")
    }

    /// Parse while checking whether the caller still needs the result,
    /// between headings.
    pub fn parse_with_cancellation(source: &str, cancelled: impl Fn() -> bool) -> Option<Self> {
        let headings = crate::document::scan_headings(source, cancelled)?;
        Some(Self::from_headings(headings))
    }

    /// Name each heading with its anchor, and drop those with no text.
    pub(crate) fn from_headings(headings: Vec<crate::document::RawHeading>) -> Self {
        let mut slugger = Slugger::default();
        let headings = headings
            .into_iter()
            .filter_map(|(level, text, source_range)| {
                let text = one_line(&text);
                if text.is_empty() {
                    return None;
                }
                let anchor = slugger.slug(&text).into();
                Some(OutlineHeading {
                    level,
                    text,
                    anchor,
                    source_range,
                })
            })
            .collect();
        Self { headings }
    }

    pub fn headings(&self) -> &[OutlineHeading] {
        &self.headings
    }

    pub fn is_empty(&self) -> bool {
        self.headings.is_empty()
    }

    /// The heading a link's fragment points at, written with or without its
    /// `#` and `%20`-style escapes. Tried in order, so an exact anchor always
    /// wins:
    ///
    /// 1. the anchor itself, as GitHub writes it: `#getting-started`;
    /// 2. the heading's text, ignoring case, as Obsidian writes a link to a
    ///    heading: `[[#Getting Started]]`;
    /// 3. the fragment made into an anchor, for a link typed by hand:
    ///    `#Getting Started` finds the first `Getting Started` heading.
    ///
    /// Obsidian's `#Chapter#Section` path names its last heading.
    pub fn find(&self, fragment: &str) -> Option<usize> {
        let fragment = fragment.strip_prefix('#').unwrap_or(fragment);
        let decoded = percent_decoded(fragment);
        let fragment = decoded.trim();
        if fragment.is_empty() {
            return None;
        }
        let by_anchor = |fragment: &str| {
            self.headings
                .iter()
                .position(|heading| &*heading.anchor == fragment)
        };
        let by_text = |fragment: &str| {
            let fragment = fragment.to_lowercase();
            self.headings
                .iter()
                .position(|heading| heading.text.to_lowercase() == fragment)
        };
        let by_slug = |fragment: &str| {
            let slug = slugify(fragment);
            (!slug.is_empty())
                .then(|| {
                    self.headings
                        .iter()
                        .position(|heading| slugify(&heading.text) == slug)
                })
                .flatten()
        };
        let last = fragment.rsplit('#').next().unwrap_or(fragment).trim();
        by_anchor(fragment)
            .or_else(|| by_text(fragment))
            .or_else(|| by_slug(fragment))
            .or_else(|| (last != fragment).then(|| by_text(last).or_else(|| by_slug(last)))?)
    }

    /// The heading whose section holds the source byte `offset`: the last
    /// one that starts at or before it. `None` above the first heading.
    pub fn section_at(&self, offset: usize) -> Option<usize> {
        self.headings
            .partition_point(|heading| heading.source_range.start <= offset)
            .checked_sub(1)
    }
}

/// One entry of a `[TOC]` block: a heading's level, text and anchor. Where
/// the heading is in the source is left out, so typing elsewhere does not
/// change the block.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct TocEntry {
    level: u8,
    text: Arc<str>,
    anchor: Arc<str>,
}

impl TocEntry {
    pub(crate) fn all(outline: &Outline) -> Arc<[TocEntry]> {
        outline
            .headings
            .iter()
            .map(|heading| TocEntry {
                level: heading.level,
                text: heading.text.clone(),
                anchor: heading.anchor.clone(),
            })
            .collect()
    }

    pub fn level(&self) -> u8 {
        self.level
    }

    pub fn text(&self) -> &Arc<str> {
        &self.text
    }

    /// The anchor of the heading, without its `#`.
    pub fn anchor(&self) -> &Arc<str> {
        &self.anchor
    }
}

/// How far to indent each entry of a contents list, in steps: one step
/// inside the nearest entry above it with a lower level, so `#` then `###`
/// reads as parent and child rather than leaving an empty step between
/// them. An entry with no such parent sits at 0.
pub fn indent_levels(levels: impl IntoIterator<Item = u8>) -> Vec<usize> {
    // The entries the next one may nest under, outermost first, each with
    // its level and depth.
    let mut parents: Vec<(u8, usize)> = Vec::new();
    levels
        .into_iter()
        .map(|level| {
            while parents.last().is_some_and(|&(open, _)| open >= level) {
                parents.pop();
            }
            let depth = parents.last().map_or(0, |&(_, depth)| depth + 1);
            parents.push((level, depth));
            depth
        })
        .collect()
}

/// Whether a paragraph written as `text` asks for a table of contents in its
/// place: `[TOC]` as Typora writes it, `[[_TOC_]]` as GitLab does, or
/// `[[TOC]]`, in any case, alone in its paragraph.
pub fn is_toc_marker(text: &str) -> bool {
    let text = text.trim();
    ["[toc]", "[[toc]]", "[[_toc_]]"]
        .iter()
        .any(|marker| text.eq_ignore_ascii_case(marker))
}

/// The anchor GitHub makes from a heading's text, before telling repeated
/// headings apart: lowercased, with punctuation and symbols dropped and each
/// space made a hyphen. Letters and digits of every script stay, so `## 安装
/// 指南` is `#安装-指南`.
pub fn slugify(text: &str) -> String {
    let mut slug = String::with_capacity(text.len());
    for c in text.chars() {
        if c == ' ' || c == '-' {
            slug.push('-');
        } else if c.is_alphanumeric() || c == '_' || is_combining_mark(c) {
            slug.extend(c.to_lowercase());
        }
    }
    slug
}

/// Gives repeated headings distinct anchors as GitHub does: the second
/// `Notes` is `notes-1`, the third `notes-2`, skipping any taken already.
#[derive(Default)]
struct Slugger {
    /// Each anchor handed out, with how many times its base was repeated.
    taken: HashMap<String, usize>,
}

impl Slugger {
    fn slug(&mut self, text: &str) -> String {
        let base = slugify(text);
        let mut slug = base.clone();
        while self.taken.contains_key(&slug) {
            let count = self.taken.entry(base.clone()).or_default();
            *count += 1;
            slug = format!("{base}-{count}");
        }
        self.taken.insert(slug.clone(), 0);
        slug
    }
}

/// Marks that combine with the letter before them, such as a combining
/// acute accent. Most are alphabetic already; these blocks hold the rest.
fn is_combining_mark(c: char) -> bool {
    matches!(
        c,
        '\u{0300}'..='\u{036F}'
            | '\u{1AB0}'..='\u{1AFF}'
            | '\u{1DC0}'..='\u{1DFF}'
            | '\u{20D0}'..='\u{20FF}'
            | '\u{FE20}'..='\u{FE2F}'
    )
}

/// Heading text on one line: a setext heading may run over several, and
/// the parser keeps a line break as a newline.
fn one_line(text: &str) -> Arc<str> {
    if !text.contains('\n') {
        return text.trim().into();
    }
    text.split('\n')
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
        .into()
}

/// `text` with each `%XX` escape decoded, as in a link's target:
/// `My%20Note` is `My Note`. As written if the result is not UTF-8.
pub fn percent_decoded(text: &str) -> std::borrow::Cow<'_, str> {
    if !text.contains('%') {
        return text.into();
    }
    let bytes = text.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut ix = 0;
    while ix < bytes.len() {
        let hex = |offset: usize| {
            bytes
                .get(ix + offset)
                .and_then(|&b| (b as char).to_digit(16))
        };
        match (bytes[ix], hex(1), hex(2)) {
            (b'%', Some(high), Some(low)) => {
                decoded.push((high * 16 + low) as u8);
                ix += 3;
            }
            (byte, _, _) => {
                decoded.push(byte);
                ix += 1;
            }
        }
    }
    String::from_utf8(decoded).map_or(text.into(), Into::into)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BlockKind, MarkdownDocument};

    fn outline(source: &str) -> Vec<(u8, String, String)> {
        Outline::parse(source)
            .headings()
            .iter()
            .map(|h| (h.level(), h.text().to_string(), h.anchor().to_string()))
            .collect()
    }

    #[test]
    fn slugs_follow_github() {
        assert_eq!(slugify("Getting Started"), "getting-started");
        assert_eq!(slugify("What's new in v2.0?"), "whats-new-in-v20");
        assert_eq!(slugify("snake_case & kebab-case"), "snake_case--kebab-case");
        assert_eq!(slugify("  Two  spaces "), "--two--spaces-");
        assert_eq!(slugify("Ünïcödé Straße"), "ünïcödé-straße");
        assert_eq!(slugify("安装 指南（快速）"), "安装-指南快速");
        assert_eq!(slugify("Emoji 🚀 rocket"), "emoji--rocket");
        assert_eq!(slugify("C++ / Rust"), "c--rust");
    }

    #[test]
    fn repeated_headings_get_numbered_anchors() {
        let anchors: Vec<_> = outline("# A\n## A\n### A-1\n# A\n")
            .into_iter()
            .map(|(_, _, anchor)| anchor)
            .collect();
        // The third heading takes `a-1` first, so the fourth skips it.
        assert_eq!(anchors, ["a", "a-1", "a-1-1", "a-2"]);
    }

    #[test]
    fn reads_headings_as_the_parser_does() {
        let source = "\
# Title

```sh
# not a heading
```

    # indented code

Setext *heading*
over two lines
---

> ## In a quote

- ### In a list

<div>
# html
</div>

## **Bold** `code` [link](x) ==mark==
#
###### Deep
";
        assert_eq!(
            outline(source),
            [
                (1, "Title".into(), "title".into()),
                (
                    2,
                    "Setext heading over two lines".into(),
                    "setext-heading-over-two-lines".into()
                ),
                (2, "In a quote".into(), "in-a-quote".into()),
                (3, "In a list".into(), "in-a-list".into()),
                (
                    2,
                    "Bold code link mark".into(),
                    "bold-code-link-mark".into()
                ),
                (6, "Deep".into(), "deep".into()),
            ]
        );
    }

    #[test]
    fn source_ranges_start_at_the_heading() {
        let source = "intro\n\n## Two\n\n> # Quoted\n";
        let outline = Outline::parse(source);
        let starts: Vec<_> = outline
            .headings()
            .iter()
            .map(|h| &source[h.source_range().start..])
            .collect();
        assert!(starts[0].starts_with("## Two"));
        assert!(starts[1].starts_with("# Quoted"));
    }

    #[test]
    fn the_document_outline_matches_the_scan() {
        let sources = [
            "# A\n\ntext\n\n## B\n\n- # C\n  - ## D\n\n> ### E\n>\n> > #### F\n",
            "Setext\n===\n\n[^1]: # In a footnote\n\n| a |\n|---|\n| # cell |\n",
            "---\ntitle: x\n---\n\n# After front matter\n\n> [!note] # Not\n> ## Callout heading\n",
        ];
        for source in sources {
            assert_eq!(
                MarkdownDocument::parse(source).outline(),
                &Outline::parse(source),
                "{source}"
            );
        }
    }

    #[test]
    fn finds_headings_by_anchor_text_or_typed_fragment() {
        let outline =
            Outline::parse("# Intro\n## Getting Started\n## Notes\n## Notes\n## 安装 指南\n");
        assert_eq!(outline.find("#getting-started"), Some(1));
        assert_eq!(outline.find("getting-started"), Some(1));
        assert_eq!(outline.find("#notes-1"), Some(3));
        assert_eq!(outline.find("#Getting Started"), Some(1));
        assert_eq!(outline.find("#getting%20started"), Some(1));
        assert_eq!(outline.find("#GETTING-STARTED"), Some(1));
        assert_eq!(
            outline.find("#%E5%AE%89%E8%A3%85-%E6%8C%87%E5%8D%97"),
            Some(4)
        );
        assert_eq!(outline.find("#Intro#Notes"), Some(2));
        assert_eq!(outline.find("#missing"), None);
        assert_eq!(outline.find("#"), None);
        assert_eq!(outline.find(""), None);
    }

    #[test]
    fn an_exact_anchor_wins_over_text() {
        // The second heading's anchor is `a-1`; the third's text is `a-1`.
        let outline = Outline::parse("# A\n# A\n# a-1\n");
        assert_eq!(outline.find("#a-1"), Some(1));
    }

    #[test]
    fn finds_the_section_holding_an_offset() {
        let source = "lead\n\n# One\n\nbody\n\n## Two\n";
        let outline = Outline::parse(source);
        assert_eq!(outline.section_at(0), None);
        assert_eq!(outline.section_at(source.find("# One").unwrap()), Some(0));
        assert_eq!(outline.section_at(source.find("body").unwrap()), Some(0));
        assert_eq!(outline.section_at(source.len()), Some(1));
        assert_eq!(Outline::default().section_at(10), None);
    }

    #[test]
    fn indents_by_nesting_not_by_level() {
        assert_eq!(indent_levels([1, 3, 3, 2, 4, 1]), [0, 1, 1, 1, 2, 0]);
        assert_eq!(indent_levels([3, 2, 1]), [0, 0, 0]);
        assert_eq!(indent_levels([2, 2, 3, 6]), [0, 0, 1, 2]);
        assert!(indent_levels([]).is_empty());
    }

    #[test]
    fn recognizes_toc_markers() {
        for marker in ["[TOC]", "[toc]", " [TOC] \n", "[[_TOC_]]", "[[toc]]"] {
            assert!(is_toc_marker(marker), "{marker:?}");
        }
        for text in ["[TOC] here", "TOC", "[ TOC ]", "[TOC]\n[TOC]", "[[TOC]"] {
            assert!(!is_toc_marker(text), "{text:?}");
        }
    }

    #[test]
    fn a_toc_paragraph_becomes_a_contents_block() {
        let document =
            MarkdownDocument::parse("[TOC]\n\n# One\n\n## Two\n\n> [TOC]\n\n[[_TOC_]]\n");
        let kinds: Vec<_> = document.blocks().iter().map(|b| b.kind()).collect();
        let BlockKind::TableOfContents(entries) = kinds[0] else {
            panic!("expected contents, got {:?}", kinds[0]);
        };
        let texts: Vec<_> = entries.iter().map(|e| (e.level(), &**e.anchor())).collect();
        assert_eq!(texts, [(1, "one"), (2, "two")]);
        // Inside a quote, the marker stays text.
        assert!(matches!(kinds[3], BlockKind::Quote(_)));
        assert!(matches!(kinds[4], BlockKind::TableOfContents(_)));
    }

    #[test]
    fn a_contents_block_changes_with_the_headings_only() {
        let fingerprint = |source: &str| MarkdownDocument::parse(source).blocks()[0].fingerprint();
        let base = fingerprint("[TOC]\n\n# One\n\ntext\n");
        assert_eq!(base, fingerprint("[TOC]\n\n# One\n\nmore text here\n"));
        assert_eq!(base, fingerprint("[TOC]\n\nlead\n\n# One\n\ntext\n"));
        assert_ne!(base, fingerprint("[TOC]\n\n# Uno\n\ntext\n"));
        assert_ne!(base, fingerprint("[TOC]\n\n## One\n\ntext\n"));
    }
}
