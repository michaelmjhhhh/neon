//! Emphasis markers and code fences.

use super::{Options, Sink, marker_style, reject_unknown};
use crate::Edit;
use crate::context::Context;

pub(super) fn delimiter_options(table: &toml::Table) -> Result<Options, String> {
    marker_style(table, &[("asterisk", '*'), ("underscore", '_')])
}

pub(super) fn emphasis(cx: &Context<'_>, options: &Options, sink: &mut Sink<'_, '_>) {
    delimiters(cx, options, sink, false);
}

pub(super) fn strong(cx: &Context<'_>, options: &Options, sink: &mut Sink<'_, '_>) {
    delimiters(cx, options, sink, true);
}

fn delimiters(cx: &Context<'_>, options: &Options, sink: &mut Sink<'_, '_>, strong: bool) {
    let source = cx.source;
    let bytes = source.as_bytes();
    let width = if strong { 2 } else { 1 };
    let mut target = match options {
        Options::Marker(marker) => marker.map(|marker| marker as u8),
        _ => None,
    };
    for span in cx.emphasis.iter().filter(|span| span.strong == strong) {
        let range = &span.range;
        if range.len() < 2 * width {
            continue;
        }
        let open = range.start..range.start + width;
        let close = range.end - width..range.end;
        let marker = bytes[range.start];
        if !matches!(marker, b'*' | b'_')
            || !bytes[open.clone()]
                .iter()
                .chain(&bytes[close.clone()])
                .all(|byte| *byte == marker)
        {
            continue;
        }
        let target = *target.get_or_insert(marker);
        if marker == target {
            continue;
        }
        // Underscores do not open or close emphasis inside a word.
        if target == b'_' {
            let before = source[..range.start].chars().next_back();
            let after = source[range.end..].chars().next();
            if before.is_some_and(char::is_alphanumeric) || after.is_some_and(char::is_alphanumeric)
            {
                continue;
            }
        }
        let text = (target as char).to_string().repeat(width);
        let kind = if strong { "Bold" } else { "Emphasis" };
        sink.report(
            range.clone(),
            format!(
                "{kind} marked with `{}`",
                (marker as char).to_string().repeat(width)
            ),
            vec![
                Edit {
                    range: open,
                    text: text.clone(),
                },
                Edit { range: close, text },
            ],
        );
    }
}

pub(super) fn fence_options(table: &toml::Table) -> Result<Options, String> {
    marker_style(table, &[("backtick", '`'), ("tilde", '~')])
}

pub(super) fn fence(cx: &Context<'_>, options: &Options, sink: &mut Sink<'_, '_>) {
    let source = cx.source;
    let lines = &cx.lines;
    let mut target = match options {
        Options::Marker(marker) => marker,
        _ => &None,
    }
    .map(|marker| marker as u8);
    for fence in &cx.fences {
        let range = &fence.range;
        let first = lines.of(range.start);
        let opening = &source[range.start..lines.content(source, first).end];
        let marker = opening.as_bytes()[0];
        if !matches!(marker, b'`' | b'~') {
            continue;
        }
        let length = opening.bytes().take_while(|byte| *byte == marker).count();
        let target = *target.get_or_insert(marker);
        if marker == target {
            continue;
        }
        let new_fence = |length| (target as char).to_string().repeat(length);
        // A backtick fence's info string may not hold a backtick.
        if target == b'`' && opening[length..].contains('`') {
            continue;
        }
        let last = lines.of(range.end.saturating_sub(1).max(range.start));
        let mut edits = vec![Edit {
            range: range.start..range.start + length,
            text: new_fence(length),
        }];
        let mut body_end = last + 1;
        if last > first {
            let content = lines.content(source, last);
            let line = source[content.clone()].trim_end();
            let closing = line.len() - line.trim_end_matches(marker as char).len();
            let prefix = &line[..line.len() - closing];
            if closing >= length && prefix.chars().all(|ch| matches!(ch, ' ' | '\t' | '>')) {
                body_end = last;
                edits.push(Edit {
                    range: content.start + prefix.len()..content.start + line.len(),
                    text: new_fence(closing),
                });
            }
        }
        // A line of the new fence's markers in the code would close it.
        let closes_early = (first + 1..body_end).any(|line| {
            source[lines.content(source, line)]
                .trim_start_matches([' ', '\t', '>'])
                .starts_with(&new_fence(3))
        });
        if closes_early {
            continue;
        }
        sink.report(
            range.start..range.start + length,
            format!("Code fence written with `{}`", marker as char),
            edits,
        );
    }
}

pub(super) fn language_options(table: &toml::Table) -> Result<Options, String> {
    reject_unknown(table, &["language"])?;
    match table.get("language") {
        None => Ok(Options::Language("text".into())),
        Some(value) => {
            let language = value.as_str().ok_or("`language` must be a string")?.trim();
            if language.contains(char::is_whitespace) || language.contains(['`', '~']) {
                return Err("`language` must be one word".into());
            }
            Ok(Options::Language(language.into()))
        }
    }
}

pub(super) fn fence_language(cx: &Context<'_>, options: &Options, sink: &mut Sink<'_, '_>) {
    let language = match options {
        Options::Language(language) => language.as_str(),
        _ => "text",
    };
    let source = cx.source;
    for fence in cx.fences.iter().filter(|fence| fence.info_empty) {
        let start = fence.range.start;
        let marker = source.as_bytes()[start];
        let length = source[start..]
            .bytes()
            .take_while(|byte| *byte == marker)
            .count();
        let at = start + length;
        let fix = if language.is_empty() {
            Vec::new()
        } else {
            vec![Edit {
                range: at..at,
                text: language.into(),
            }]
        };
        sink.report(start..at, "Code block without a language", fix);
    }
}

#[cfg(test)]
mod tests {
    use crate::rules::test_support::{findings, fixed};

    #[test]
    fn emphasis_markers_match() {
        assert_eq!(
            fixed("emphasis-style", "", "*a* and _b_\n"),
            "*a* and *b*\n"
        );
        assert_eq!(
            fixed(
                "emphasis-style",
                "style = \"underscore\"",
                "*a* and x*b*y\n"
            ),
            "_a_ and x*b*y\n"
        );
        assert_eq!(
            fixed("strong-style", "style = \"asterisk\"", "__a__ ***b***\n"),
            "**a** ***b***\n"
        );
        assert_eq!(
            fixed("strong-style", "style = \"underscore\"", "***b***\n"),
            "*__b__*\n"
        );
    }

    #[test]
    fn fences_match() {
        assert_eq!(
            fixed(
                "code-fence-style",
                "style = \"backtick\"",
                "~~~rust\nfn a() {}\n~~~~\n"
            ),
            "```rust\nfn a() {}\n````\n"
        );
        assert_eq!(
            fixed(
                "code-fence-style",
                "style = \"tilde\"",
                "> ```\n> x\n> ```\n"
            ),
            "> ~~~\n> x\n> ~~~\n"
        );
        // Backticks in the code would close a backtick fence.
        let kept = "~~~\n```\n~~~\n";
        assert_eq!(
            fixed("code-fence-style", "style = \"backtick\"", kept),
            kept
        );
    }

    #[test]
    fn fences_name_a_language() {
        assert_eq!(
            fixed("fence-language", "", "```\nx\n```\n"),
            "```text\nx\n```\n"
        );
        assert_eq!(
            fixed("fence-language", "language = \"sh\"", "~~~\nx\n~~~\n"),
            "~~~sh\nx\n~~~\n"
        );
        let found = findings("fence-language", "language = \"\"", "```\nx\n```\n");
        assert_eq!(found.len(), 1);
        assert!(found[0].fix.is_empty());
    }
}
