//! Whitespace: trailing spaces, blank lines, the final line ending and the
//! blank lines around blocks.

use super::{Options, Sink, reject_unknown};
use crate::Edit;
use crate::context::{BlockKind, Context};

pub(super) fn trailing_spaces(cx: &Context<'_>, _: &Options, sink: &mut Sink<'_, '_>) {
    let source = cx.source;
    let mut breaks = cx.hard_breaks.iter().peekable();
    for line in 0..cx.lines.count() {
        let content = cx.lines.content(source, line);
        let text = &source[content.clone()];
        let kept = text.trim_end_matches([' ', '\t']).len();
        if kept == text.len() {
            continue;
        }
        let spaces = content.start + kept..content.end;
        if cx.is_protected(&spaces) {
            continue;
        }
        while breaks.next_if(|hard| hard.start < spaces.start).is_some() {}
        if breaks.peek().is_some_and(|hard| hard.start == spaces.start) {
            // Two spaces end a line with a line break; keep exactly two.
            if &source[spaces.clone()] != "  " {
                sink.report(
                    spaces.clone(),
                    "Line break written with more than two spaces",
                    vec![Edit {
                        range: spaces,
                        text: "  ".into(),
                    }],
                );
            }
            continue;
        }
        sink.report(
            spaces.clone(),
            "Trailing whitespace",
            vec![Edit {
                range: spaces,
                text: String::new(),
            }],
        );
    }
}

pub(super) fn max_options(table: &toml::Table) -> Result<Options, String> {
    reject_unknown(table, &["max"])?;
    match table.get("max") {
        None => Ok(Options::Max(1)),
        Some(value) => match value.as_integer() {
            // None would join paragraphs.
            Some(max) if max >= 1 => Ok(Options::Max(max as usize)),
            _ => Err("`max` must be a whole number of at least 1".into()),
        },
    }
}

pub(super) fn consecutive_blank_lines(
    cx: &Context<'_>,
    options: &Options,
    sink: &mut Sink<'_, '_>,
) {
    let max = match options {
        Options::Max(max) => *max,
        _ => 1,
    };
    let source = cx.source;
    let count = cx.lines.count();
    let blank =
        |line: usize| cx.lines.is_blank(source, line) && !cx.is_protected(&cx.lines.full(line));
    let mut line = 0;
    while line < count {
        if !blank(line) {
            line += 1;
            continue;
        }
        let start = line;
        while line < count && blank(line) {
            line += 1;
        }
        // Blank lines that end the file are `final-newline`'s.
        if line == count {
            break;
        }
        // Front matter must start the file; a `---` after blank lines is a
        // rule or a heading, and removing the lines would change that.
        let allowed = if start == 0 && !source[cx.lines.content(source, line)].starts_with("---") {
            0
        } else {
            max
        };
        if line - start > allowed {
            let range = cx.lines.start(start + allowed)..cx.lines.start(line);
            let message = if start == 0 {
                "Blank lines at the start of the file".into()
            } else {
                format!("{} blank lines in a row", line - start)
            };
            sink.report(
                range.clone(),
                message,
                vec![Edit {
                    range,
                    text: String::new(),
                }],
            );
        }
    }
}

pub(super) fn final_newline(cx: &Context<'_>, _: &Options, sink: &mut Sink<'_, '_>) {
    let source = cx.source;
    let Some(last) = (0..cx.lines.count())
        .rev()
        .find(|line| !cx.lines.is_blank(source, *line))
    else {
        return;
    };
    let content = cx.lines.content(source, last);
    let tail = content.end..source.len();
    if &source[tail.clone()] == cx.newline
        || cx.is_protected(&tail)
        || (tail.is_empty() && cx.ends_in_verbatim)
    {
        return;
    }
    let message = if tail.is_empty() {
        "No line ending at the end of the file"
    } else {
        "Blank lines at the end of the file"
    };
    sink.report(
        content,
        message,
        vec![Edit {
            range: tail,
            text: cx.newline.into(),
        }],
    );
}

pub(super) fn around_headings(cx: &Context<'_>, _: &Options, sink: &mut Sink<'_, '_>) {
    around(cx, sink, BlockKind::Heading, "heading");
}

pub(super) fn around_fences(cx: &Context<'_>, _: &Options, sink: &mut Sink<'_, '_>) {
    around(cx, sink, BlockKind::FencedCode, "code block");
}

pub(super) fn around_tables(cx: &Context<'_>, _: &Options, sink: &mut Sink<'_, '_>) {
    around(cx, sink, BlockKind::Table, "table");
}

pub(super) fn around_blockquotes(cx: &Context<'_>, _: &Options, sink: &mut Sink<'_, '_>) {
    around(cx, sink, BlockKind::BlockQuote, "block quote");
}

pub(super) fn around_rules(cx: &Context<'_>, _: &Options, sink: &mut Sink<'_, '_>) {
    around(cx, sink, BlockKind::Rule, "horizontal rule");
}

/// A blank line before and after each top-level block of `kind`. Blocks in
/// lists and quotes are left alone: a blank line there needs the
/// container's markers.
fn around(cx: &Context<'_>, sink: &mut Sink<'_, '_>, kind: BlockKind, name: &str) {
    let source = cx.source;
    let lines = &cx.lines;
    for (ix, block) in cx.blocks.iter().enumerate() {
        if block.kind != kind {
            continue;
        }
        let first = lines.of(block.range.start);
        let last = lines.of(block.range.end.saturating_sub(1).max(block.range.start));
        if first > 0 && !lines.is_blank(source, first - 1) {
            // Front matter may sit right above the first heading.
            let after_metadata = ix > 0
                && cx.blocks[ix - 1].kind == BlockKind::Metadata
                && lines.of(cx.blocks[ix - 1].range.end.saturating_sub(1)) == first - 1;
            if !after_metadata {
                let at = lines.start(first);
                sink.report(
                    lines.content(source, first),
                    format!("No blank line before {name}"),
                    vec![Edit {
                        range: at..at,
                        text: cx.newline.into(),
                    }],
                );
            }
        }
        if last + 1 < lines.count() && !lines.is_blank(source, last + 1) {
            let at = lines.start(last + 1);
            sink.report(
                lines.content(source, last),
                format!("No blank line after {name}"),
                vec![Edit {
                    range: at..at,
                    text: cx.newline.into(),
                }],
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::rules::test_support::{findings, fixed};

    #[test]
    fn trailing_spaces_keep_line_breaks() {
        assert_eq!(fixed("trailing-spaces", "", "a \nb\t\n"), "a\nb\n");
        assert_eq!(fixed("trailing-spaces", "", "a  \nb\n"), "a  \nb\n");
        assert_eq!(fixed("trailing-spaces", "", "a    \nb\n"), "a  \nb\n");
        // The last line of a paragraph has no break to keep.
        assert_eq!(fixed("trailing-spaces", "", "a  \n\nb\n"), "a\n\nb\n");
        assert_eq!(fixed("trailing-spaces", "", "a \r\nb\r\n"), "a\r\nb\r\n");
    }

    #[test]
    fn trailing_spaces_leave_code_alone() {
        let source = "```\ncode  \n```\n\n    indented  \n";
        assert_eq!(fixed("trailing-spaces", "", source), source);
        let html = "<div>\nhi  \n</div>\n";
        assert_eq!(fixed("trailing-spaces", "", html), html);
        let front_matter = "---\na: 1  \n---\n";
        assert_eq!(fixed("trailing-spaces", "", front_matter), front_matter);
    }

    #[test]
    fn blank_lines_collapse() {
        assert_eq!(
            fixed("consecutive-blank-lines", "", "a\n\n\n\nb\n"),
            "a\n\nb\n"
        );
        assert_eq!(
            fixed("consecutive-blank-lines", "max = 2", "a\n\n\n\nb\n"),
            "a\n\n\nb\n"
        );
        assert_eq!(fixed("consecutive-blank-lines", "", "\n\na\n"), "a\n");
        let code = "```\na\n\n\n\nb\n```\n";
        assert_eq!(fixed("consecutive-blank-lines", "", code), code);
        // Front matter must stay at the start, so is never made of a rule.
        let rule = "\n---\na\n---\n";
        assert_eq!(fixed("consecutive-blank-lines", "", rule), rule);
    }

    #[test]
    fn one_line_ending_at_the_end() {
        assert_eq!(fixed("final-newline", "", "a"), "a\n");
        assert_eq!(fixed("final-newline", "", "a\n\n\n"), "a\n");
        assert_eq!(fixed("final-newline", "", "a\r\n\r\n"), "a\r\n");
        assert_eq!(fixed("final-newline", "", ""), "");
        assert_eq!(fixed("final-newline", "", "a  "), "a  \n");
        // A line ending would join code that runs to the end of the file.
        for kept in ["    code", "```\ncode", "<div>"] {
            assert_eq!(fixed("final-newline", "", kept), kept);
        }
        assert_eq!(
            fixed("final-newline", "", "```\ncode\n```"),
            "```\ncode\n```\n"
        );
        assert_eq!(fixed("final-newline", "", "a `code`"), "a `code`\n");
    }

    #[test]
    fn blank_lines_around_blocks() {
        assert_eq!(
            fixed("blank-lines-around-headings", "", "a\n# H\nb\n"),
            "a\n\n# H\n\nb\n"
        );
        assert_eq!(
            fixed("blank-lines-around-headings", "", "---\nt: 1\n---\n# H\n"),
            "---\nt: 1\n---\n# H\n"
        );
        assert_eq!(
            fixed("blank-lines-around-fences", "", "a\n```\nx\n```\nb\n"),
            "a\n\n```\nx\n```\n\nb\n"
        );
        assert_eq!(
            fixed(
                "blank-lines-around-tables",
                "",
                "a\n\n| a |\n|---|\n| 1 |\n# H\n"
            ),
            "a\n\n| a |\n|---|\n| 1 |\n\n# H\n"
        );
        // Inside a list a blank line would need the list's indent.
        let nested = "- a\n  # H\n  b\n";
        assert!(findings("blank-lines-around-headings", "", nested).is_empty());
    }

    #[test]
    fn rejects_bad_options() {
        assert!(super::max_options(&toml::from_str("max = 0").unwrap()).is_err());
        assert!(super::max_options(&toml::from_str("maximum = 2").unwrap()).is_err());
    }
}
