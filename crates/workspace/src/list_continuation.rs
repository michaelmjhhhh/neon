//! Enter on a Markdown list item starts the next item.

use std::ops::Range;

use gpui_kit::component::input::{Rope, RopeExt as _};

use crate::slash_commands::in_code_block;

/// The edit Enter makes at `cursor`, if the cursor ends a list item's text.
///
/// After `- milk` it inserts a newline and `- `, after `3. eggs` a newline and
/// `4. `, and after a task a new unchecked task. Enter on an empty item removes
/// its marker, which ends the list. Returns the range to replace and its text.
pub(crate) fn on_enter(text: &Rope, cursor: usize) -> Option<(Range<usize>, String)> {
    let row = text.offset_to_point(cursor).row;
    let line_start = text.line_start_offset(row);
    let line_end = text.line_end_offset(row);
    let line = text.slice(line_start..line_end).to_string();
    let item = ListItem::parse(&line)?;
    // A cursor inside the indent or marker splits the line as usual.
    if cursor - line_start < item.prefix_len || in_code_block(text, row) {
        return None;
    }
    if line[item.prefix_len..].trim().is_empty() {
        return Some((line_start..line_end, String::new()));
    }
    let task = if item.task { "[ ] " } else { "" };
    Some((
        cursor..cursor,
        format!("\n{}{} {task}", item.indent, item.next_marker()),
    ))
}

/// Whether the line at `cursor` is a list item outside a code block.
pub(crate) fn is_list_item(text: &Rope, cursor: usize) -> bool {
    let row = text.offset_to_point(cursor).row;
    let line = text
        .slice(text.line_start_offset(row)..text.line_end_offset(row))
        .to_string();
    ListItem::parse(&line).is_some() && !in_code_block(text, row)
}

struct ListItem<'a> {
    indent: &'a str,
    marker: Marker,
    task: bool,
    /// Bytes from the line start to the item's text.
    prefix_len: usize,
}

enum Marker {
    Bullet(char),
    Ordered { number: u64, delimiter: char },
}

impl<'a> ListItem<'a> {
    fn parse(line: &'a str) -> Option<Self> {
        let rest = line.trim_start_matches([' ', '\t']);
        let indent = &line[..line.len() - rest.len()];
        let (marker, rest) = match rest.chars().next()? {
            bullet @ ('-' | '*' | '+') => (Marker::Bullet(bullet), &rest[1..]),
            _ => {
                let digits =
                    rest.len() - rest.trim_start_matches(|c: char| c.is_ascii_digit()).len();
                // CommonMark allows at most nine digits.
                if !(1..=9).contains(&digits) {
                    return None;
                }
                let delimiter = rest[digits..]
                    .chars()
                    .next()
                    .filter(|c| matches!(c, '.' | ')'))?;
                let number = rest[..digits].parse().ok()?;
                (Marker::Ordered { number, delimiter }, &rest[digits + 1..])
            }
        };
        let rest = rest.strip_prefix(' ')?;
        let after_task = ["[ ] ", "[x] ", "[X] "]
            .iter()
            .find_map(|task| rest.strip_prefix(task));
        let rest = after_task.unwrap_or(rest);
        Some(Self {
            indent,
            marker,
            task: after_task.is_some(),
            prefix_len: line.len() - rest.len(),
        })
    }

    fn next_marker(&self) -> String {
        match self.marker {
            Marker::Bullet(bullet) => bullet.to_string(),
            Marker::Ordered { number, delimiter } => format!("{}{delimiter}", number + 1),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The document after Enter at the end of `text`, if Enter continues a list.
    fn enter_at_end(text: &str) -> Option<String> {
        let rope = Rope::from(text);
        let (range, insert) = on_enter(&rope, text.len())?;
        let mut result = text.to_string();
        result.replace_range(range, &insert);
        Some(result)
    }

    #[test]
    fn bullets_continue_with_the_same_marker_and_indent() {
        assert_eq!(enter_at_end("- milk").as_deref(), Some("- milk\n- "));
        assert_eq!(enter_at_end("  * milk").as_deref(), Some("  * milk\n  * "));
        assert_eq!(enter_at_end("+ milk").as_deref(), Some("+ milk\n+ "));
    }

    #[test]
    fn ordered_items_count_up() {
        assert_eq!(enter_at_end("1. eggs").as_deref(), Some("1. eggs\n2. "));
        assert_eq!(enter_at_end("9) eggs").as_deref(), Some("9) eggs\n10) "));
    }

    #[test]
    fn tasks_continue_unchecked() {
        assert_eq!(
            enter_at_end("- [x] done").as_deref(),
            Some("- [x] done\n- [ ] ")
        );
    }

    #[test]
    fn empty_items_end_the_list() {
        assert_eq!(enter_at_end("- milk\n- ").as_deref(), Some("- milk\n"));
        assert_eq!(enter_at_end("- [ ] ").as_deref(), Some(""));
        assert_eq!(enter_at_end("  2. ").as_deref(), Some(""));
    }

    #[test]
    fn other_lines_are_left_alone() {
        assert_eq!(enter_at_end("milk"), None);
        assert_eq!(enter_at_end("-milk"), None);
        assert_eq!(enter_at_end("---"), None);
        assert_eq!(enter_at_end("2024 was"), None);
        assert_eq!(enter_at_end("```\n- milk"), None);
    }

    #[test]
    fn list_items_are_recognized_by_line() {
        let rope = Rope::from("intro\n  - milk\n3. eggs\n```\n- code\n```");
        assert!(!is_list_item(&rope, 2));
        assert!(is_list_item(&rope, 8));
        assert!(is_list_item(&rope, 16));
        assert!(!is_list_item(&rope, 28));
    }

    #[test]
    fn a_cursor_in_the_marker_splits_the_line() {
        assert_eq!(on_enter(&Rope::from("- milk"), 1), None);
        let rope = Rope::from("- milk");
        assert_eq!(on_enter(&rope, 4), Some((4..4, "\n- ".to_string())));
    }
}
