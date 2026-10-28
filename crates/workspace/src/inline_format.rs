//! Cmd+B and Cmd+I toggle Markdown bold or italic.
//!
//! What is already styled is read from the Markdown parser with the
//! preview's options, not guessed from asterisks, so the shortcut agrees
//! with what the preview shows. The rules follow word processors:
//!
//! - text that has the style loses it, text that has it only in part gets
//!   it as a whole;
//! - with no selection the word at the cursor is styled, and nothing is
//!   typed away from a word;
//! - each paragraph, heading, list item or table cell of a selection is
//!   styled on its own, since markers cannot span blocks;
//! - code, HTML, front matter and link targets are never touched, and an
//!   edit whose markers would not render is not made.

use std::ops::Range;

use gpui_kit::component::input::{Rope, RopeExt as _};
use pulldown_cmark::{Event, LinkType, Parser, Tag, TagEnd};

use crate::slash_commands::step_fence;

/// An inline style that Markdown marks with asterisks.
#[derive(Clone, Copy)]
pub(crate) enum Style {
    Bold,
    Italic,
}

impl Style {
    fn marker(self) -> &'static str {
        match self {
            Style::Bold => "**",
            Style::Italic => "*",
        }
    }

    fn is_tag(self, tag: &Tag) -> bool {
        match self {
            Style::Bold => matches!(tag, Tag::Strong),
            Style::Italic => matches!(tag, Tag::Emphasis),
        }
    }
}

/// An edit to the document: replace `range` with `text`, then select
/// `selection`.
#[derive(Debug, PartialEq)]
pub(crate) struct Edit {
    pub range: Range<usize>,
    pub text: String,
    pub selection: Range<usize>,
}

/// The edit that toggles `style` on `selection` in `text`, or `None` when
/// there is nothing to do. `text` must hold whole blocks, as
/// [`toggle_in_rope`] passes it.
///
/// `milk` becomes `**milk**` with `milk` still selected, `**two [words]**`
/// becomes `two [words]`, `[buy **milk**]` becomes `**[buy milk]**`, and
/// with only a cursor `mi|lk` becomes `**mi|lk**`.
pub(crate) fn toggle(text: &str, selection: Range<usize>, style: Style) -> Option<Edit> {
    let parsed = Parsed::new(text, style);
    if parsed.is_literal(selection.start) || parsed.is_literal(selection.end) {
        return None;
    }
    let (ops, kept, added) = if selection.is_empty() {
        parsed.cursor_ops(text, selection.start, style)?
    } else {
        parsed.selection_ops(text, selection, style)?
    };

    let range = ops.iter().map(Op::start).min()?..ops.iter().map(Op::end).max()?;
    let marker = style.marker();
    let mut replaced = String::new();
    let mut at = range.start;
    for op in &ops {
        replaced.push_str(&text[at..op.start()]);
        at = op.end();
        if let Op::Open(_) | Op::Close(_) = op {
            replaced.push_str(marker);
        }
    }
    replaced.push_str(&text[at..range.end]);

    let map = |offset: usize| map_offset(&ops, offset, marker.len());
    // Markers that do not render, as in `a**(b)**`, would only add clutter.
    if !added.is_empty() {
        let result = format!("{}{replaced}{}", &text[..range.start], &text[range.end..]);
        let spans = Parsed::new(&result, style).spans;
        let renders = added.iter().all(|segment| {
            let segment = map(segment.start)..map(segment.end);
            spans
                .iter()
                .any(|span| span.start <= segment.start && segment.end <= span.end)
        });
        if !renders {
            return None;
        }
    }

    Some(Edit {
        range,
        text: replaced,
        selection: map(kept.start)..map(kept.end),
    })
}

/// [`toggle`] on a rope, parsing only the blocks the selection spans: inline
/// styles never cross a blank line, so the rest of the document cannot
/// change the edit, and a long document is not parsed per keypress.
pub(crate) fn toggle_in_rope(text: &Rope, selection: Range<usize>, style: Style) -> Option<Edit> {
    let start =
        text.line_start_offset(block_start(text, text.offset_to_point(selection.start).row));
    let end = block_end(text, text.offset_to_point(selection.end).row);
    let blocks = text.slice(start..end).to_string();
    let edit = toggle(
        &blocks,
        selection.start - start..selection.end - start,
        style,
    )?;
    Some(Edit {
        range: edit.range.start + start..edit.range.end + start,
        text: edit.text,
        selection: edit.selection.start + start..edit.selection.end + start,
    })
}

/// The first row of the top-level block holding `row`: the last unindented
/// line after a blank line, outside code fences and front matter. Indented
/// lines are skipped because, read alone, they could parse as code.
///
/// Fences can only be told apart from the top, so this reads every line
/// above `row`, from the rope's chunks rather than line by line: slicing out
/// each line of a 10 MB document costs about 180 ms.
fn block_start(text: &Rope, row: usize) -> usize {
    let mut scan = BlockScan::default();
    let mut line = String::new();
    let mut ix = 0;
    for chunk in text.slice(..text.line_start_offset(row)).chunks() {
        for piece in chunk.split_inclusive('\n') {
            line.push_str(piece);
            if piece.ends_with('\n') {
                scan.line(ix, &line);
                line.clear();
                ix += 1;
            }
        }
    }
    scan.line(row, &text.slice_line(row).to_string());
    scan.start
}

#[derive(Default)]
struct BlockScan {
    start: usize,
    fence: Option<(char, usize)>,
    front_matter: bool,
    /// Whether the previous line was blank, or there was none.
    in_text: bool,
}

impl BlockScan {
    fn line(&mut self, ix: usize, line: &str) {
        let trimmed = line.trim();
        if ix == 0 && trimmed == "---" {
            self.front_matter = true;
        } else if self.front_matter {
            self.front_matter = !matches!(trimmed, "---" | "...");
        } else if self.fence.is_none()
            && !self.in_text
            && line.chars().next().is_some_and(|ch| !ch.is_whitespace())
        {
            self.start = ix;
        }
        step_fence(&mut self.fence, line.chars());
        self.in_text = !trimmed.is_empty();
    }
}

/// The offset where the block holding `row` ends: the next blank line.
fn block_end(text: &Rope, row: usize) -> usize {
    text.iter_lines()
        .enumerate()
        .skip(row + 1)
        .find(|(_, line)| line.chars().all(char::is_whitespace))
        .map_or(text.len(), |(ix, _)| text.line_start_offset(ix))
}

/// A change to the text: drop a range, or insert the style's marker that
/// opens or closes a span.
#[derive(Clone, Debug)]
enum Op {
    Remove(Range<usize>),
    Open(usize),
    Close(usize),
}

impl Op {
    fn start(&self) -> usize {
        match self {
            Op::Remove(range) => range.start,
            Op::Open(at) | Op::Close(at) => *at,
        }
    }

    fn end(&self) -> usize {
        match self {
            Op::Remove(range) => range.end,
            Op::Open(at) | Op::Close(at) => *at,
        }
    }
}

/// Where `offset` lands after `ops`. A cursor at an opening marker moves
/// past it and one at a closing marker stays before it, so `|milk` and
/// `milk|` stay inside the new span.
fn map_offset(ops: &[Op], offset: usize, marker_len: usize) -> usize {
    let mut mapped = offset;
    for op in ops {
        match op {
            Op::Remove(range) if range.end <= offset => mapped -= range.len(),
            Op::Remove(range) if range.start < offset => mapped -= offset - range.start,
            Op::Open(at) if *at <= offset => mapped += marker_len,
            Op::Close(at) if *at < offset => mapped += marker_len,
            _ => {}
        }
    }
    mapped
}

/// The operations of an edit, the range to select once they are applied,
/// and the ranges that gain the style.
type Plan = (Vec<Op>, Range<usize>, Vec<Range<usize>>);

/// What the parser reports about one style in a run of whole blocks.
#[derive(Default)]
struct Parsed {
    /// Spans with the style, markers included.
    spans: Vec<Range<usize>>,
    /// Ranges whose inside is not prose: code, HTML, front matter, and link
    /// targets.
    literal: Vec<Range<usize>>,
    /// The inline content of each paragraph, heading, list item and table
    /// cell.
    leaves: Vec<Range<usize>>,
}

impl Parsed {
    fn new(text: &str, style: Style) -> Self {
        let mut parsed = Self::default();
        // The open blocks, each with its inline content so far when it holds
        // inline content directly.
        let mut blocks: Vec<(bool, Option<Range<usize>>)> = Vec::new();
        // The open links and images, with the range of their text so far.
        let mut links: Vec<(Range<usize>, bool, Option<Range<usize>>)> = Vec::new();

        for (event, range) in Parser::new_ext(text, markdown::PARSER_OPTIONS).into_offset_iter() {
            let inline = match &event {
                Event::Start(tag) if is_inline(tag) => {
                    if style.is_tag(tag) {
                        parsed.spans.push(range.clone());
                    }
                    if let Tag::Link { link_type, .. } | Tag::Image { link_type, .. } = tag {
                        let whole = matches!(tag, Tag::Image { .. })
                            || matches!(
                                link_type,
                                LinkType::Autolink
                                    | LinkType::Email
                                    | LinkType::WikiLink { has_pothole: false }
                            );
                        // Extend the outer link before this one opens.
                        extend(links.last_mut().map(|link| &mut link.2), &range);
                        extend(leaf(&mut blocks), &range);
                        links.push((range, whole, None));
                        continue;
                    }
                    true
                }
                Event::Start(tag) => {
                    if matches!(
                        tag,
                        Tag::CodeBlock(_) | Tag::HtmlBlock | Tag::MetadataBlock(_)
                    ) {
                        parsed.literal.push(range.clone());
                    }
                    blocks.push((is_leaf(tag), None));
                    false
                }
                Event::End(TagEnd::Link | TagEnd::Image) => {
                    if let Some((link, whole, text)) = links.pop() {
                        match text {
                            Some(text) if !whole => {
                                parsed.literal.push(link.start..text.start);
                                parsed.literal.push(text.end..link.end);
                            }
                            _ => parsed.literal.push(link),
                        }
                    }
                    false
                }
                Event::End(end) if is_inline_end(end) => false,
                Event::End(_) => {
                    if let Some((true, Some(content))) = blocks.pop() {
                        parsed.leaves.push(content);
                    }
                    false
                }
                Event::Code(_) | Event::InlineMath(_) | Event::InlineHtml(_) => {
                    parsed.literal.push(range.clone());
                    true
                }
                Event::Text(_)
                | Event::FootnoteReference(_)
                | Event::SoftBreak
                | Event::HardBreak => true,
                _ => false,
            };
            if inline {
                extend(links.last_mut().map(|link| &mut link.2), &range);
                extend(leaf(&mut blocks), &range);
            }
        }
        parsed
    }

    /// Whether `offset` falls strictly inside code, HTML or a link target.
    fn is_literal(&self, offset: usize) -> bool {
        self.literal
            .iter()
            .any(|range| range.start < offset && offset < range.end)
    }

    /// The innermost span with the style that holds `range`.
    fn span_around(&self, range: &Range<usize>) -> Option<&Range<usize>> {
        self.spans
            .iter()
            .filter(|span| span.start <= range.start && range.end <= span.end)
            .min_by_key(|span| span.len())
    }

    fn cursor_ops(&self, text: &str, cursor: usize, style: Style) -> Option<Plan> {
        if let Some(span) = self.span_around(&(cursor..cursor)) {
            return Some((unwrap(span, style), cursor..cursor, Vec::new()));
        }
        let word = word_at(text, cursor)?;
        if !self
            .leaves
            .iter()
            .any(|leaf| leaf.start <= word.start && word.end <= leaf.end)
        {
            return None;
        }
        Some((self.wrap(word.clone(), style), cursor..cursor, vec![word]))
    }

    fn selection_ops(&self, text: &str, selection: Range<usize>, style: Style) -> Option<Plan> {
        let segments: Vec<Range<usize>> = self
            .leaves
            .iter()
            .filter_map(|leaf| {
                let start = leaf.start.max(selection.start);
                let end = leaf.end.min(selection.end);
                let inner = text.get(start..end)?;
                let start = start + (inner.len() - inner.trim_start().len());
                let end = end - (inner.len() - inner.trim_end().len());
                (start < end).then_some(start..end)
            })
            .collect();
        let kept = segments.first()?.start..segments.last()?.end;

        let mut ops = Vec::new();
        let mut added = Vec::new();
        if segments
            .iter()
            .all(|segment| self.span_around(segment).is_some())
        {
            for segment in &segments {
                ops.extend(unwrap(self.span_around(segment)?, style));
            }
        } else {
            for segment in segments {
                if self.span_around(&segment).is_none() {
                    ops.extend(self.wrap(segment.clone(), style));
                    added.push(segment);
                }
            }
        }
        ops.sort_by_key(|op| (op.start(), matches!(op, Op::Close(_))));
        Some((ops, kept, added))
    }

    /// The operations that give `range` the style as one span: grow it over
    /// spans it cuts into, and drop the markers of spans inside it, so
    /// `bu[y **mi]lk**` becomes `bu**y milk**`, not nested markers.
    fn wrap(&self, mut range: Range<usize>, style: Style) -> Vec<Op> {
        loop {
            let grown = self
                .spans
                .iter()
                .filter(|span| span.start < range.end && range.start < span.end)
                .fold(range.clone(), |grown, span| {
                    grown.start.min(span.start)..grown.end.max(span.end)
                });
            if grown == range {
                break;
            }
            range = grown;
        }
        let mut ops = vec![Op::Open(range.start)];
        for span in &self.spans {
            if range.start <= span.start && span.end <= range.end {
                ops.extend(unwrap(span, style));
            }
        }
        ops.push(Op::Close(range.end));
        ops.sort_by_key(|op| (op.start(), matches!(op, Op::Close(_))));
        ops
    }
}

/// The operations that remove the markers of `span`.
fn unwrap(span: &Range<usize>, style: Style) -> Vec<Op> {
    let len = style.marker().len();
    vec![
        Op::Remove(span.start..span.start + len),
        Op::Remove(span.end - len..span.end),
    ]
}

/// The inline content of the innermost open block, when it holds inline
/// content directly.
fn leaf(blocks: &mut [(bool, Option<Range<usize>>)]) -> Option<&mut Option<Range<usize>>> {
    match blocks.last_mut() {
        Some((true, content)) => Some(content),
        _ => None,
    }
}

fn extend(content: Option<&mut Option<Range<usize>>>, range: &Range<usize>) {
    if let Some(content) = content {
        *content = Some(match content.take() {
            Some(content) => content.start.min(range.start)..content.end.max(range.end),
            None => range.clone(),
        });
    }
}

fn is_inline(tag: &Tag) -> bool {
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

fn is_inline_end(end: &TagEnd) -> bool {
    matches!(
        end,
        TagEnd::Emphasis
            | TagEnd::Strong
            | TagEnd::Strikethrough
            | TagEnd::Superscript
            | TagEnd::Subscript
            | TagEnd::Link
            | TagEnd::Image
    )
}

/// Blocks whose content is inline text rather than other blocks.
fn is_leaf(tag: &Tag) -> bool {
    matches!(
        tag,
        Tag::Paragraph
            | Tag::Heading { .. }
            | Tag::Item
            | Tag::TableCell
            | Tag::DefinitionListTitle
            | Tag::DefinitionListDefinition
    )
}

/// The word the cursor sits in or touches at either end.
fn word_at(text: &str, cursor: usize) -> Option<Range<usize>> {
    let is_word = |c: char| c.is_alphanumeric() || c == '_';
    let start = text[..cursor]
        .char_indices()
        .rev()
        .take_while(|&(_, c)| is_word(c))
        .last()
        .map_or(cursor, |(i, _)| i);
    let end = text[cursor..]
        .char_indices()
        .find(|&(_, c)| !is_word(c))
        .map_or(text.len(), |(i, _)| cursor + i);
    (start < end).then_some(start..end)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Toggle `style` on the part of `text` between `{` and `}`, and return
    /// the document with the new selection marked the same way. Braces stand
    /// in for the selection because brackets are Markdown links.
    fn toggle_marked(text: &str, style: Style) -> String {
        let start = text.find('{').unwrap();
        let end = text.find('}').unwrap() - 1;
        let text = text.replace(['{', '}'], "");
        let Some(edit) = toggle(&text, start..end, style) else {
            return format!(
                "{}{{{}}}{}",
                &text[..start],
                &text[start..end],
                &text[end..]
            );
        };
        mark(&text, edit)
    }

    /// The same through [`toggle_in_rope`], which finds the blocks itself.
    fn toggle_marked_in_rope(text: &str, style: Style) -> String {
        let start = text.find('{').unwrap();
        let end = text.find('}').unwrap() - 1;
        let text = text.replace(['{', '}'], "");
        let Some(edit) = toggle_in_rope(&Rope::from(text.as_str()), start..end, style) else {
            return format!(
                "{}{{{}}}{}",
                &text[..start],
                &text[start..end],
                &text[end..]
            );
        };
        mark(&text, edit)
    }

    fn mark(text: &str, edit: Edit) -> String {
        let mut result = text.to_string();
        result.replace_range(edit.range, &edit.text);
        result.insert(edit.selection.end, '}');
        result.insert(edit.selection.start, '{');
        result
    }

    #[test]
    fn wraps_the_selection() {
        assert_eq!(
            toggle_marked("buy {milk} now", Style::Bold),
            "buy **{milk}** now"
        );
        assert_eq!(
            toggle_marked("buy {milk} now", Style::Italic),
            "buy *{milk}* now"
        );
    }

    #[test]
    fn removes_the_style_of_the_selection() {
        assert_eq!(toggle_marked("**{milk}**", Style::Bold), "{milk}");
        assert_eq!(toggle_marked("{**milk**}", Style::Bold), "{milk}");
        assert_eq!(toggle_marked("{*milk*}", Style::Italic), "{milk}");
        assert_eq!(toggle_marked("__{milk}__", Style::Bold), "{milk}");
        assert_eq!(
            toggle_marked("**two {words}** now", Style::Bold),
            "two {words} now"
        );
        assert_eq!(toggle_marked("*{two} words*", Style::Italic), "{two} words");
    }

    #[test]
    fn styles_partly_styled_text_as_a_whole() {
        assert_eq!(
            toggle_marked("{buy **milk**} now", Style::Bold),
            "**{buy milk}** now"
        );
        assert_eq!(
            toggle_marked("bu{y **mi}lk** now", Style::Bold),
            "bu**{y mi}lk** now"
        );
        assert_eq!(toggle_marked("{*a* and *b*}", Style::Italic), "*{a and b}*");
    }

    #[test]
    fn bold_and_italic_combine() {
        assert_eq!(toggle_marked("**{milk}**", Style::Italic), "***{milk}***");
        assert_eq!(toggle_marked("*{milk}*", Style::Bold), "***{milk}***");
        assert_eq!(toggle_marked("***{milk}***", Style::Bold), "*{milk}*");
        assert_eq!(toggle_marked("***{milk}***", Style::Italic), "**{milk}**");
        assert_eq!(
            toggle_marked("**two {words}**", Style::Italic),
            "**two *{words}***"
        );
    }

    #[test]
    fn trims_spaces_from_the_selection() {
        assert_eq!(
            toggle_marked("buy { milk }now", Style::Bold),
            "buy  **{milk}** now"
        );
        assert_eq!(toggle_marked("buy{ }now", Style::Bold), "buy{ }now");
    }

    #[test]
    fn wraps_the_word_at_the_cursor() {
        assert_eq!(
            toggle_marked("buy mi{}lk now", Style::Bold),
            "buy **mi{}lk** now"
        );
        assert_eq!(
            toggle_marked("buy milk{} now", Style::Italic),
            "buy *milk{}* now"
        );
        assert_eq!(toggle_marked("buy {}milk", Style::Bold), "buy **{}milk**");
        assert_eq!(toggle_marked("買{}牛奶", Style::Bold), "**買{}牛奶**");
        assert_eq!(
            toggle_marked("[mi{}lk](https://a.b)", Style::Bold),
            "[**mi{}lk**](https://a.b)"
        );
    }

    #[test]
    fn does_nothing_away_from_a_word() {
        assert_eq!(toggle_marked("buy {}", Style::Bold), "buy {}");
        assert_eq!(toggle_marked("buy {} now", Style::Italic), "buy {} now");
        assert_eq!(toggle_marked("{}", Style::Bold), "{}");
    }

    #[test]
    fn removes_the_style_the_cursor_is_in() {
        assert_eq!(
            toggle_marked("**two wo{}rds** now", Style::Bold),
            "two wo{}rds now"
        );
        assert_eq!(toggle_marked("_two wo{}rds_", Style::Italic), "two wo{}rds");
        assert_eq!(toggle_marked("**milk**{}", Style::Bold), "milk{}");
        assert_eq!(toggle_marked("{}**milk**", Style::Bold), "{}milk");
        assert_eq!(toggle_marked("- **mi{}lk**", Style::Bold), "- mi{}lk");
        assert_eq!(toggle_marked("*mi{}lk*", Style::Bold), "***mi{}lk***");
        assert_eq!(toggle_marked("***mi{}lk***", Style::Italic), "**mi{}lk**");
    }

    #[test]
    fn leaves_code_html_and_link_targets_alone() {
        assert_eq!(
            toggle_marked("run `l{}s` now", Style::Bold),
            "run `l{}s` now"
        );
        assert_eq!(
            toggle_marked("run `l{s` n}ow", Style::Bold),
            "run `l{s` n}ow"
        );
        assert_eq!(
            toggle_marked("```\nbuy mi{}lk\n```", Style::Bold),
            "```\nbuy mi{}lk\n```"
        );
        assert_eq!(
            toggle_marked("[milk](https://ex{}ample.com)", Style::Bold),
            "[milk](https://ex{}ample.com)"
        );
        assert_eq!(
            toggle_marked("<https://ex{}ample.com>", Style::Bold),
            "<https://ex{}ample.com>"
        );
        assert_eq!(toggle_marked("[[mi{}lk]]", Style::Bold), "[[mi{}lk]]");
        assert_eq!(
            toggle_marked("<span cl{}ass=\"x\">milk</span>", Style::Bold),
            "<span cl{}ass=\"x\">milk</span>"
        );
        assert_eq!(
            toggle_marked("---\ntitle: mi{}lk\n---", Style::Bold),
            "---\ntitle: mi{}lk\n---"
        );
    }

    #[test]
    fn a_selection_around_code_is_wrapped() {
        assert_eq!(
            toggle_marked("{run `ls`} now", Style::Bold),
            "**{run `ls`}** now"
        );
    }

    #[test]
    fn styles_each_block_of_the_selection() {
        assert_eq!(
            toggle_marked("{- a\n- b}", Style::Bold),
            "- **{a**\n- **b}**"
        );
        assert_eq!(
            toggle_marked("{- **a**\n- **b**}", Style::Bold),
            "- {a\n- b}"
        );
        assert_eq!(
            toggle_marked("{- **a**\n- b}", Style::Bold),
            "- {**a**\n- **b}**"
        );
        assert_eq!(toggle_marked("{# Title}", Style::Bold), "# **{Title}**");
        assert_eq!(
            toggle_marked("{- [ ] buy milk}", Style::Bold),
            "- [ ] **{buy milk}**"
        );
        assert_eq!(
            toggle_marked("{one\n\ntwo}", Style::Italic),
            "*{one*\n\n*two}*"
        );
    }

    #[test]
    fn does_not_add_markers_that_would_not_render() {
        assert_eq!(toggle_marked("a{(b)}", Style::Bold), "a{(b)}");
    }

    #[test]
    fn reads_the_blocks_around_the_selection_in_a_rope() {
        assert_eq!(
            toggle_marked_in_rope("intro\n\nbuy **mi{}lk** now\n\noutro", Style::Bold),
            "intro\n\nbuy mi{}lk now\n\noutro"
        );
        assert_eq!(
            toggle_marked_in_rope("**two\nwo{}rds**", Style::Bold),
            "two\nwo{}rds"
        );
        // Read alone, the indented paragraph would be a code block.
        assert_eq!(
            toggle_marked_in_rope("- item\n\n    more **mi{}lk**", Style::Bold),
            "- item\n\n    more mi{}lk"
        );
        assert_eq!(
            toggle_marked_in_rope("```\na\n\nmi{}lk\n```", Style::Bold),
            "```\na\n\nmi{}lk\n```"
        );
        assert_eq!(
            toggle_marked_in_rope("---\na: 1\n\ntitle: mi{}lk\n---", Style::Bold),
            "---\na: 1\n\ntitle: mi{}lk\n---"
        );
    }
}
