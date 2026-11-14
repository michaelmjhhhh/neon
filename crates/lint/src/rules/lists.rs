//! List markers and numbering.

use super::{Options, Sink, marker_style, reject_unknown};
use crate::Edit;
use crate::context::Context;

pub(super) fn marker_options(table: &toml::Table) -> Result<Options, String> {
    marker_style(table, &[("dash", '-'), ("asterisk", '*'), ("plus", '+')])
}

pub(super) fn unordered_marker(cx: &Context<'_>, options: &Options, sink: &mut Sink<'_, '_>) {
    let bytes = cx.source.as_bytes();
    let mut target = match options {
        Options::Marker(marker) => marker.map(|marker| marker as u8),
        _ => None,
    };
    for list in cx.lists.iter().filter(|list| list.start.is_none()) {
        for &item in &list.items {
            let marker = bytes[item];
            if !matches!(marker, b'-' | b'*' | b'+') {
                continue;
            }
            let target = *target.get_or_insert(marker);
            if marker == target {
                continue;
            }
            sink.report(
                item..item + 1,
                format!(
                    "List marker `{}` instead of `{}`",
                    marker as char, target as char
                ),
                vec![Edit {
                    range: item..item + 1,
                    text: (target as char).to_string(),
                }],
            );
        }
    }
}

pub(super) fn numbering_options(table: &toml::Table) -> Result<Options, String> {
    reject_unknown(table, &["style"])?;
    match table.get("style").map(|value| value.as_str()) {
        None | Some(Some("ascending")) => Ok(Options::Ascending(true)),
        Some(Some("lazy")) => Ok(Options::Ascending(false)),
        _ => Err("`style` must be ascending or lazy".into()),
    }
}

pub(super) fn ordered_numbering(cx: &Context<'_>, options: &Options, sink: &mut Sink<'_, '_>) {
    let ascending = !matches!(options, Options::Ascending(false));
    let source = cx.source;
    for list in &cx.lists {
        let Some(start) = list.start else {
            continue;
        };
        for (ix, &item) in list.items.iter().enumerate() {
            let digits = source[item..]
                .bytes()
                .take_while(u8::is_ascii_digit)
                .count();
            let Ok(number) = source[item..item + digits].parse::<u64>() else {
                continue;
            };
            let expected = if ascending {
                start.saturating_add(ix as u64)
            } else {
                start
            };
            if number == expected {
                continue;
            }
            sink.report(
                item..item + digits,
                format!("Item numbered {number} instead of {expected}"),
                vec![Edit {
                    range: item..item + digits,
                    text: expected.to_string(),
                }],
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::rules::test_support::fixed;

    #[test]
    fn bullets_match() {
        assert_eq!(
            fixed("unordered-list-marker", "", "- a\n- b\n\ntext\n\n* c\n"),
            "- a\n- b\n\ntext\n\n- c\n"
        );
        assert_eq!(
            fixed(
                "unordered-list-marker",
                "style = \"asterisk\"",
                "- a\n  + b\n"
            ),
            "* a\n  * b\n"
        );
    }

    #[test]
    fn bullets_that_would_change_the_lists_are_left_alone() {
        let config = crate::Config::only("unordered-list-marker", "");
        // A change of bullet starts a new list: one bullet would join them.
        // `- ---` is a horizontal rule, not an item holding one.
        for source in ["- a\n\n* b\n", "- a\n\ntext\n\n* ---\n"] {
            let outcome = crate::fix(source, &config, crate::FixMode::All).unwrap();
            assert_eq!(outcome.text, source);
            assert_eq!(outcome.rejected.len(), 1);
        }
    }

    #[test]
    fn numbers_count_up() {
        assert_eq!(
            fixed("ordered-list-numbering", "", "1. a\n1. b\n1. c\n"),
            "1. a\n2. b\n3. c\n"
        );
        assert_eq!(
            fixed("ordered-list-numbering", "", "3) a\n7) b\n"),
            "3) a\n4) b\n"
        );
        assert_eq!(
            fixed("ordered-list-numbering", "style = \"lazy\"", "1. a\n2. b\n"),
            "1. a\n1. b\n"
        );
        assert_eq!(
            fixed(
                "ordered-list-numbering",
                "",
                "1. a\n   1. x\n   5. y\n2. b\n"
            ),
            "1. a\n   1. x\n   2. y\n2. b\n"
        );
    }
}
