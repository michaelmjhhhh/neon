//! Rules over the words: spaces between them, between scripts, and
//! ellipses. Each reads only runs of plain text, so code, math, HTML and
//! link targets are never changed.

use super::{Options, Sink};
use crate::Edit;
use crate::context::Context;

pub(super) fn multiple_spaces(cx: &Context<'_>, _: &Options, sink: &mut Sink<'_, '_>) {
    for text in &cx.texts {
        let bytes = &cx.source.as_bytes()[text.clone()];
        let mut ix = 0;
        while ix < bytes.len() {
            if bytes[ix] != b' ' {
                ix += 1;
                continue;
            }
            let start = ix;
            while ix < bytes.len() && bytes[ix] == b' ' {
                ix += 1;
            }
            // Only between words: not spaces that open or close the run.
            if ix - start >= 2 && start > 0 && ix < bytes.len() {
                let range = text.start + start..text.start + ix;
                sink.report(
                    range.clone(),
                    "More than one space between words",
                    vec![Edit {
                        range,
                        text: " ".into(),
                    }],
                );
            }
        }
    }
}

/// Han, kana, Hangul and their extensions; not CJK punctuation, which needs
/// no space.
fn is_cjk(ch: char) -> bool {
    matches!(ch,
        '\u{2E80}'..='\u{2FDF}'
        | '\u{3040}'..='\u{30FF}'
        | '\u{3100}'..='\u{312F}'
        | '\u{3130}'..='\u{318F}'
        | '\u{31A0}'..='\u{31BF}'
        | '\u{31F0}'..='\u{31FF}'
        | '\u{3400}'..='\u{4DBF}'
        | '\u{4E00}'..='\u{9FFF}'
        | '\u{AC00}'..='\u{D7AF}'
        | '\u{F900}'..='\u{FAFF}'
        | '\u{20000}'..='\u{2FA1F}')
}

pub(super) fn cjk_spacing(cx: &Context<'_>, _: &Options, sink: &mut Sink<'_, '_>) {
    for text in &cx.texts {
        let words = &cx.source[text.clone()];
        // Pure ASCII text has no CJK to space from.
        if words.is_ascii() {
            continue;
        }
        let mut previous: Option<char> = None;
        for (ix, ch) in words.char_indices() {
            if let Some(previous) = previous
                && ((is_cjk(previous) && ch.is_ascii_alphanumeric())
                    || (previous.is_ascii_alphanumeric() && is_cjk(ch)))
            {
                let at = text.start + ix;
                sink.report(
                    at - previous.len_utf8()..at + ch.len_utf8(),
                    "No space between CJK and Latin text",
                    vec![Edit {
                        range: at..at,
                        text: " ".into(),
                    }],
                );
            }
            previous = Some(ch);
        }
    }
}

pub(super) fn proper_ellipsis(cx: &Context<'_>, _: &Options, sink: &mut Sink<'_, '_>) {
    for text in &cx.texts {
        let words = &cx.source[text.clone()];
        let bytes = words.as_bytes();
        let mut from = 0;
        while let Some(found) = words[from..].find("...") {
            let start = from + found;
            let mut end = start + 3;
            while end < bytes.len() && bytes[end] == b'.' {
                end += 1;
            }
            from = end;
            // Four or more periods are something else, as in a table of
            // contents.
            if end - start != 3 || (start > 0 && bytes[start - 1] == b'.') {
                continue;
            }
            let range = text.start + start..text.start + end;
            sink.report(
                range.clone(),
                "Three periods instead of an ellipsis",
                vec![Edit {
                    range,
                    text: "…".into(),
                }],
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::rules::test_support::fixed;

    #[test]
    fn spaces_between_words_collapse() {
        assert_eq!(fixed("multiple-spaces", "", "a  b   c\n"), "a b c\n");
        let kept = "`a  b` [x](<a  b>)\n";
        assert_eq!(fixed("multiple-spaces", "", kept), kept);
        // Indentation and line breaks are not between words.
        let indented = "- a\n\n    b\n\nc  \nd\n";
        assert_eq!(fixed("multiple-spaces", "", indented), indented);
    }

    #[test]
    fn cjk_and_latin_get_a_space() {
        assert_eq!(
            fixed("cjk-spacing", "", "在Neon里写Markdown很快3倍\n"),
            "在 Neon 里写 Markdown 很快 3 倍\n"
        );
        assert_eq!(
            fixed("cjk-spacing", "", "中文，English。\n"),
            "中文，English。\n"
        );
        // Code, link targets and wikilinks stay as written.
        let kept = "`中文abc` [[笔记A]] <https://例子.com/a>\n";
        assert_eq!(fixed("cjk-spacing", "", kept), kept);
        assert_eq!(
            fixed("cjk-spacing", "", "[链接a](https://x.com/中文a)\n"),
            "[链接 a](https://x.com/中文a)\n"
        );
    }

    #[test]
    fn three_periods_become_an_ellipsis() {
        assert_eq!(
            fixed("proper-ellipsis", "", "Wait... what\n"),
            "Wait… what\n"
        );
        for kept in ["a.... b\n", "`a...`\n", "$x...y$\n", "```\n...\n```\n"] {
            assert_eq!(fixed("proper-ellipsis", "", kept), kept);
        }
    }
}
