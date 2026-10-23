//! The parser's events with the Obsidian syntax the preview reads made
//! explicit, for output written while walking events rather than from
//! [`crate::Block`]s, such as the exports.
//!
//! Callouts, highlights, wikilinks and front matter are read with the same
//! rules as [`crate::MarkdownDocument`], so an export shows what the preview
//! shows.

use std::collections::VecDeque;
use std::ops::Range;
use std::sync::Arc;

use pulldown_cmark::{CodeBlockKind, CowStr, Event, LinkType, Parser, Tag, TagEnd};

use crate::document::{
    PARSER_OPTIONS, Property, callout_header, is_inline_tag, is_inline_tag_end, parse_properties,
    quote_kind_name, wiki_target,
};
use crate::inline::{highlight_markers, is_in, literal_prefix};

/// One step through a document.
#[derive(Clone, Debug, PartialEq)]
pub enum MarkdownEvent<'a> {
    /// An event as the parser reports it, except that a wikilink points at
    /// the file it names, `Note.md` for `[[Note]]`, and adjacent text is
    /// joined into one event.
    Event(Event<'a>),
    /// `==Highlighted==` text starts. A highlight holds only text and inline
    /// code: it ends before any tag and starts again after it, so it always
    /// nests inside the tags around it.
    StartHighlight,
    EndHighlight,
    /// A callout of this kind starts, in place of the block quote it was
    /// written as. Its title follows as inline content, up to
    /// [`Self::EndCalloutTitle`]; a callout without one is titled by its
    /// kind. Then its body follows as blocks, up to [`Self::EndCallout`].
    StartCallout(Arc<str>),
    EndCalloutTitle,
    EndCallout,
    /// Front matter, read as properties, in place of its metadata block.
    /// Front matter without keys stays a YAML code block.
    FrontMatter(Vec<Property>),
}

/// The events of `source`, read as [`crate::MarkdownDocument::parse`] reads
/// it.
pub fn events(source: &str) -> Vec<MarkdownEvent<'_>> {
    let mut raw: VecDeque<_> = Parser::new_ext(source, PARSER_OPTIONS)
        .into_offset_iter()
        .collect();
    let mut writer = Writer {
        out: Vec::with_capacity(raw.len()),
        inline: Vec::new(),
        quotes: Vec::new(),
        verbatim: false,
    };
    while let Some((event, range)) = raw.pop_front() {
        if writer.verbatim {
            if matches!(event, Event::End(TagEnd::CodeBlock | TagEnd::HtmlBlock)) {
                writer.verbatim = false;
            }
            writer.push(event);
            continue;
        }
        if is_inline(&event) {
            let piece = Piece::new(source, event, range);
            writer.inline.push(piece);
            continue;
        }
        writer.flush_inline();
        match event {
            Event::Start(Tag::CodeBlock(_) | Tag::HtmlBlock) => {
                writer.verbatim = true;
                writer.push(event);
            }
            Event::Start(Tag::MetadataBlock(_)) => {
                let mut yaml = String::new();
                while let Some((event, _)) = raw.pop_front() {
                    match event {
                        Event::Text(text) => yaml.push_str(&text),
                        Event::End(TagEnd::MetadataBlock(_)) => break,
                        _ => {}
                    }
                }
                let properties = parse_properties(&yaml);
                if properties.is_empty() {
                    writer.push(Event::Start(Tag::CodeBlock(CodeBlockKind::Fenced(
                        "yaml".into(),
                    ))));
                    writer.push(Event::Text(yaml.into()));
                    writer.push(Event::End(TagEnd::CodeBlock));
                } else {
                    writer.out.push(MarkdownEvent::FrontMatter(properties));
                }
            }
            Event::Start(Tag::BlockQuote(kind)) => {
                let callout = match kind {
                    Some(kind) => Some((quote_kind_name(kind).into(), Vec::new())),
                    None => take_callout_header(source, &mut raw),
                };
                writer.quotes.push(callout.is_some());
                let Some((kind, mut title)) = callout else {
                    writer.push(Event::Start(Tag::BlockQuote(kind)));
                    continue;
                };
                if title.is_empty() {
                    title.push(Piece {
                        event: Event::Text(capitalized(&kind).into()),
                        literal: 0,
                    });
                }
                writer.out.push(MarkdownEvent::StartCallout(kind));
                writer.inline = title;
                writer.flush_inline();
                writer.out.push(MarkdownEvent::EndCalloutTitle);
            }
            Event::End(TagEnd::BlockQuote(kind)) => {
                if writer.quotes.pop() == Some(true) {
                    writer.out.push(MarkdownEvent::EndCallout);
                } else {
                    writer.push(Event::End(TagEnd::BlockQuote(kind)));
                }
            }
            event => writer.push(event),
        }
    }
    writer.flush_inline();
    writer.out
}

/// An inline event, and how many of its first bytes are literal.
struct Piece<'a> {
    event: Event<'a>,
    literal: usize,
}

impl<'a> Piece<'a> {
    fn new(source: &str, event: Event<'a>, range: Range<usize>) -> Self {
        let literal = match &event {
            Event::Text(text) => literal_prefix(source, range, text),
            _ => 0,
        };
        let event = match event {
            Event::Start(Tag::Link {
                link_type: link_type @ LinkType::WikiLink { .. },
                dest_url,
                title,
                id,
            }) => Event::Start(Tag::Link {
                link_type,
                dest_url: wiki_target(&dest_url).into(),
                title,
                id,
            }),
            event => event,
        };
        Self { event, literal }
    }
}

struct Writer<'a> {
    out: Vec<MarkdownEvent<'a>>,
    /// The inline content since the last block event.
    inline: Vec<Piece<'a>>,
    /// Whether each block quote open is a callout.
    quotes: Vec<bool>,
    /// Inside a code or HTML block, whose text is as written.
    verbatim: bool,
}

impl<'a> Writer<'a> {
    /// Add an event, joining text to the text before it.
    fn push(&mut self, event: Event<'a>) {
        if let Event::Text(text) = &event
            && let Some(MarkdownEvent::Event(Event::Text(last))) = self.out.last_mut()
        {
            let mut joined = String::with_capacity(last.len() + text.len());
            joined.push_str(last);
            joined.push_str(text);
            *last = joined.into();
            return;
        }
        self.out.push(MarkdownEvent::Event(event));
    }

    /// Write the inline content gathered so far, with its highlight markers
    /// read as the preview reads them: on the content's text as one string.
    fn flush_inline(&mut self) {
        let pieces = std::mem::take(&mut self.inline);
        if pieces.is_empty() {
            return;
        }
        let mut text = String::new();
        let mut literal: Vec<Range<usize>> = Vec::new();
        let mut spans = Vec::with_capacity(pieces.len());
        for piece in &pieces {
            let start = text.len();
            match &piece.event {
                Event::Text(chunk) => {
                    text.push_str(chunk);
                    if piece.literal > 0 {
                        literal.push(start..start + piece.literal);
                    }
                }
                Event::Code(code) | Event::InlineMath(code) | Event::InlineHtml(code) => {
                    text.push_str(code);
                    literal.push(start..text.len());
                }
                Event::FootnoteReference(label) => {
                    text.push('[');
                    text.push_str(label);
                    text.push(']');
                }
                Event::SoftBreak | Event::HardBreak => text.push('\n'),
                _ => {}
            }
            spans.push(start..text.len());
        }
        let pairs = highlight_markers(&text, |ix| is_in(&literal, ix));
        if pairs.is_empty() {
            for piece in pieces {
                self.push(piece.event);
            }
            return;
        }
        let markers: Vec<Range<usize>> = pairs
            .iter()
            .flat_map(|&(open, close)| [open..open + 2, close..close + 2])
            .collect();
        let highlighted = |ix: usize| {
            let opened = pairs.partition_point(|&(open, _)| open + 2 <= ix);
            opened > 0 && ix < pairs[opened - 1].1
        };

        let mut lit = false;
        let mut image_depth = 0usize;
        for (piece, span) in pieces.into_iter().zip(spans) {
            match piece.event {
                Event::Text(_) if image_depth == 0 => {
                    let mut at = span.start;
                    while at < span.end {
                        let next = markers.partition_point(|marker| marker.end <= at);
                        match markers.get(next) {
                            Some(marker) if marker.start <= at => at = marker.end.min(span.end),
                            marker => {
                                let end = marker.map_or(span.end, |m| m.start.min(span.end));
                                self.set_highlight(&mut lit, highlighted(at));
                                self.push(Event::Text(text[at..end].to_string().into()));
                                at = end;
                            }
                        }
                    }
                }
                event @ (Event::Code(_)
                | Event::InlineMath(_)
                | Event::InlineHtml(_)
                | Event::FootnoteReference(_))
                    if image_depth == 0 =>
                {
                    self.set_highlight(&mut lit, highlighted(span.start));
                    self.push(event);
                }
                event => {
                    match &event {
                        Event::Start(Tag::Image { .. }) => image_depth += 1,
                        Event::End(TagEnd::Image) => image_depth = image_depth.saturating_sub(1),
                        _ => {}
                    }
                    self.set_highlight(&mut lit, false);
                    self.push(event);
                }
            }
        }
        self.set_highlight(&mut lit, false);
    }

    fn set_highlight(&mut self, lit: &mut bool, on: bool) {
        if *lit != on {
            self.out.push(if on {
                MarkdownEvent::StartHighlight
            } else {
                MarkdownEvent::EndHighlight
            });
            *lit = on;
        }
    }
}

/// If the block quote whose events follow in `raw` opens with `[!kind]`,
/// its kind and the title on that first line, taking both from `raw`. The
/// rest of the quote's first paragraph stays in `raw` as a paragraph.
fn take_callout_header<'a>(
    source: &str,
    raw: &mut VecDeque<(Event<'a>, Range<usize>)>,
) -> Option<(Arc<str>, Vec<Piece<'a>>)> {
    if !matches!(raw.front(), Some((Event::Start(Tag::Paragraph), _))) {
        return None;
    }
    // The parser splits `[!tip]` at its brackets; the kind is in the text
    // that opens the paragraph.
    let mut lead = String::new();
    let mut lead_events = 0;
    while let Some((Event::Text(text), _)) = raw.get(1 + lead_events) {
        lead.push_str(text);
        lead_events += 1;
    }
    let (kind, title_start) = callout_header(&lead)?;

    raw.pop_front();
    let mut title = Vec::new();
    let mut skip = title_start;
    for (event, range) in raw.drain(..lead_events) {
        let Event::Text(text) = &event else {
            continue;
        };
        let literal = literal_prefix(source, range, text);
        if skip >= text.len() {
            skip -= text.len();
            continue;
        }
        title.push(Piece {
            event: Event::Text(text[skip..].to_string().into()),
            literal: literal.saturating_sub(skip),
        });
        skip = 0;
    }
    // The rest of the first line belongs to the title too.
    loop {
        match raw.front() {
            Some((Event::SoftBreak | Event::HardBreak, _)) => {
                raw.pop_front();
                raw.push_front((Event::Start(Tag::Paragraph), 0..0));
                break;
            }
            Some((Event::End(TagEnd::Paragraph), _)) => {
                raw.pop_front();
                break;
            }
            Some(_) => {
                let (event, range) = raw.pop_front()?;
                title.push(Piece::new(source, event, range));
            }
            None => break,
        }
    }
    trim_title(&mut title);
    Some((kind, title))
}

/// Drop the whitespace around a title, and the title if nothing is left.
fn trim_title(title: &mut Vec<Piece>) {
    if let Some(Piece {
        event: Event::Text(text),
        literal,
    }) = title.first_mut()
    {
        let trimmed = text.trim_start();
        *literal = literal.saturating_sub(text.len() - trimmed.len());
        *text = CowStr::from(trimmed.to_string());
    }
    if let Some(Piece {
        event: Event::Text(text),
        ..
    }) = title.last_mut()
    {
        *text = CowStr::from(text.trim_end().to_string());
    }
    title.retain(|piece| !matches!(&piece.event, Event::Text(text) if text.is_empty()));
}

fn is_inline(event: &Event) -> bool {
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

/// `tip` as `Tip`: the title of a callout written without one.
pub(crate) fn capitalized(word: &str) -> String {
    let mut chars = word.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The events as a compact string: text as written, other events by
    /// name.
    fn trace(source: &str) -> String {
        let mut out = String::new();
        for event in events(source) {
            match event {
                MarkdownEvent::Event(Event::Text(text)) => out.push_str(&text),
                MarkdownEvent::Event(Event::Code(code)) => {
                    out.push('`');
                    out.push_str(&code);
                    out.push('`');
                }
                MarkdownEvent::Event(Event::SoftBreak) => out.push('/'),
                MarkdownEvent::Event(Event::Start(Tag::Paragraph)) => out.push_str("<p>"),
                MarkdownEvent::Event(Event::End(TagEnd::Paragraph)) => out.push_str("</p>"),
                MarkdownEvent::Event(Event::Start(Tag::Strong)) => out.push_str("<b>"),
                MarkdownEvent::Event(Event::End(TagEnd::Strong)) => out.push_str("</b>"),
                MarkdownEvent::Event(Event::Start(Tag::Link { dest_url, .. })) => {
                    out.push_str(&format!("<a {dest_url}>"));
                }
                MarkdownEvent::Event(Event::End(TagEnd::Link)) => out.push_str("</a>"),
                MarkdownEvent::Event(Event::Start(Tag::CodeBlock(_))) => out.push_str("<pre>"),
                MarkdownEvent::Event(Event::End(TagEnd::CodeBlock)) => out.push_str("</pre>"),
                MarkdownEvent::Event(other) => out.push_str(&format!("{other:?}")),
                MarkdownEvent::StartHighlight => out.push_str("<mark>"),
                MarkdownEvent::EndHighlight => out.push_str("</mark>"),
                MarkdownEvent::StartCallout(kind) => out.push_str(&format!("<callout {kind}>")),
                MarkdownEvent::EndCalloutTitle => out.push('|'),
                MarkdownEvent::EndCallout => out.push_str("</callout>"),
                MarkdownEvent::FrontMatter(properties) => {
                    for property in properties {
                        out.push_str(&format!("[{}={}]", property.key(), property.value()));
                    }
                }
            }
        }
        out
    }

    #[test]
    fn highlights_nest_inside_other_tags() {
        assert_eq!(
            trace("a ==marked **bold**== b, x == y, `==code==`"),
            "<p>a <mark>marked </mark><b><mark>bold</mark></b> b, x == y, `==code==`</p>"
        );
    }

    #[test]
    fn highlights_hold_inline_code_and_end_with_the_line() {
        assert_eq!(
            trace("==a `b` c==\n==open\nshut=="),
            "<p><mark>a `b` c</mark>/==open/shut==</p>"
        );
    }

    #[test]
    fn escaped_markers_stay_as_typed() {
        assert_eq!(
            trace(r"\==text== and ==x\==="),
            "<p>==text== and <mark>x=</mark></p>"
        );
    }

    #[test]
    fn code_blocks_keep_their_markers() {
        assert_eq!(trace("```\n==x==\n```"), "<pre>==x==\n</pre>");
    }

    #[test]
    fn obsidian_callouts_take_their_title_from_the_first_line() {
        assert_eq!(
            trace("> [!Tip]- Read **this** ==now==\n> Body text.\n"),
            "<callout tip>Read <b>this</b> <mark>now</mark>|<p>Body text.</p></callout>"
        );
    }

    #[test]
    fn untitled_callouts_are_titled_by_their_kind() {
        assert_eq!(
            trace("> [!WARNING]\n> Careful.\n"),
            "<callout warning>Warning|<p>Careful.</p></callout>"
        );
        assert_eq!(
            trace("> [!faq]\n\n> quote"),
            "<callout faq>Faq|</callout>Start(BlockQuote(None))<p>quote</p>End(BlockQuote(None))"
        );
    }

    #[test]
    fn wikilinks_point_at_markdown_files() {
        assert_eq!(
            trace("[[Weekly Review]] and [[plan#Goals|the goals]]"),
            "<p><a Weekly Review.md>Weekly Review</a> and <a plan.md#Goals>the goals</a></p>"
        );
    }

    #[test]
    fn front_matter_becomes_properties() {
        assert_eq!(
            trace("---\ntitle: Notes\ntags: [a, b]\n---\n\nBody"),
            "[title=Notes][tags=a, b]<p>Body</p>"
        );
        assert_eq!(
            trace("---\n- just a list\n---\n"),
            "<pre>- just a list\n</pre>"
        );
    }
}
