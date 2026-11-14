//! Headings: closing hashes, levels, punctuation.

use super::{Options, Sink, reject_unknown};
use crate::Edit;
use crate::context::Context;

pub(super) fn trailing_hashes(cx: &Context<'_>, _: &Options, sink: &mut Sink<'_, '_>) {
    let source = cx.source;
    for heading in &cx.headings {
        let start = heading.range.start;
        let line = cx.lines.of(start);
        let end = cx.lines.content(source, line).end;
        let text = &source[start..end];
        // Setext headings are underlined instead.
        if !text.starts_with('#') {
            continue;
        }
        let body = text.trim_end_matches([' ', '\t']);
        let before = body.trim_end_matches('#');
        if before.len() == body.len() || !before.ends_with([' ', '\t']) {
            continue;
        }
        let kept = before.trim_end_matches([' ', '\t']);
        // `# #` is a heading of hashes alone.
        if kept.trim_start_matches('#').trim().is_empty() {
            continue;
        }
        let range = start + kept.len()..start + body.len();
        sink.report(
            range.clone(),
            "Closing #s after a heading",
            vec![Edit {
                range,
                text: String::new(),
            }],
        );
    }
}

pub(super) fn increment(cx: &Context<'_>, _: &Options, sink: &mut Sink<'_, '_>) {
    let mut previous: Option<u8> = None;
    for heading in &cx.headings {
        if let Some(previous) = previous
            && heading.level > previous + 1
        {
            sink.report(
                first_line(cx, heading.range.start),
                format!("Heading level jumps from H{previous} to H{}", heading.level),
                Vec::new(),
            );
        }
        previous = Some(heading.level);
    }
}

pub(super) fn single_h1(cx: &Context<'_>, _: &Options, sink: &mut Sink<'_, '_>) {
    for heading in cx
        .headings
        .iter()
        .filter(|heading| heading.level == 1)
        .skip(1)
    {
        sink.report(
            first_line(cx, heading.range.start),
            "More than one top-level heading",
            Vec::new(),
        );
    }
}

const DEFAULT_PUNCTUATION: &str = ".,;:!。，；：！";

pub(super) fn punctuation_options(table: &toml::Table) -> Result<Options, String> {
    reject_unknown(table, &["punctuation"])?;
    let chars = match table.get("punctuation") {
        None => DEFAULT_PUNCTUATION,
        Some(value) => value.as_str().ok_or("`punctuation` must be a string")?,
    };
    Ok(Options::Chars(chars.chars().collect()))
}

pub(super) fn punctuation(cx: &Context<'_>, options: &Options, sink: &mut Sink<'_, '_>) {
    let default: Vec<char>;
    let chars = match options {
        Options::Chars(chars) => chars,
        _ => {
            default = DEFAULT_PUNCTUATION.chars().collect();
            &default
        }
    };
    for heading in &cx.headings {
        let Some(text) = &heading.last_text else {
            continue;
        };
        let words = &cx.source[text.clone()];
        let kept = words.trim_end_matches(|ch| chars.contains(&ch));
        if kept.len() == words.len() || kept.trim().is_empty() {
            continue;
        }
        let range = text.start + kept.len()..text.end;
        sink.report(
            range.clone(),
            "Heading ends with punctuation",
            vec![Edit {
                range,
                text: String::new(),
            }],
        );
    }
}

/// The line holding `offset`, for findings about a whole heading.
fn first_line(cx: &Context<'_>, offset: usize) -> std::ops::Range<usize> {
    let line = cx.lines.of(offset);
    offset..cx.lines.content(cx.source, line).end
}

#[cfg(test)]
mod tests {
    use crate::rules::test_support::{findings, fixed};

    #[test]
    fn closing_hashes_go() {
        assert_eq!(
            fixed("heading-trailing-hashes", "", "# Title ##\n"),
            "# Title\n"
        );
        assert_eq!(
            fixed("heading-trailing-hashes", "", "> ## Q #\n"),
            "> ## Q\n"
        );
        for kept in ["# C#\n", "# #\n", "# Title \\#\n", "Title\n===\n"] {
            assert_eq!(fixed("heading-trailing-hashes", "", kept), kept);
        }
    }

    #[test]
    fn levels_go_down_one_at_a_time() {
        let found = findings("heading-increment", "", "# A\n\n### B\n\n## C\n\n#### D\n");
        let messages: Vec<_> = found.iter().map(|f| f.message.as_ref()).collect();
        assert_eq!(
            messages,
            [
                "Heading level jumps from H1 to H3",
                "Heading level jumps from H2 to H4"
            ]
        );
        assert!(found.iter().all(|finding| finding.fix.is_empty()));
    }

    #[test]
    fn one_top_level_heading() {
        assert_eq!(findings("single-h1", "", "# A\n\n# B\n\n# C\n").len(), 2);
    }

    #[test]
    fn trailing_punctuation_goes() {
        assert_eq!(fixed("heading-punctuation", "", "# Title:\n"), "# Title\n");
        assert_eq!(fixed("heading-punctuation", "", "## 标题。\n"), "## 标题\n");
        assert_eq!(fixed("heading-punctuation", "", "# Why?\n"), "# Why?\n");
        assert_eq!(
            fixed("heading-punctuation", "punctuation = \"?\"", "# Why?\n"),
            "# Why\n"
        );
        // Text inside emphasis or code is not the heading's own.
        for kept in ["# *Note.*\n", "# `a;`\n", "# ...\n"] {
            assert_eq!(fixed("heading-punctuation", "", kept), kept);
        }
    }
}
