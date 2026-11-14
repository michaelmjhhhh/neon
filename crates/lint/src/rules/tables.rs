//! Table columns padded to line up.
//!
//! Only the spaces and pipes between cells change, never a cell's text, so
//! code or links in a cell stay as written.

use std::ops::Range;

use pulldown_cmark::Alignment;
use unicode_width::UnicodeWidthStr;

use super::{Options, Sink};
use crate::Edit;
use crate::context::{Context, Table};

pub(super) fn alignment(cx: &Context<'_>, _: &Options, sink: &mut Sink<'_, '_>) {
    for table in cx.tables.iter().filter(|table| table.top_level) {
        if let Some(edits) = align(cx, table)
            && !edits.is_empty()
        {
            let first = cx.lines.of(table.range.start);
            sink.report(
                cx.lines.content(cx.source, first),
                "Table columns not lined up",
                edits,
            );
        }
    }
}

/// The edits that line up `table`, or `None` for a table this rule cannot
/// rewrite safely: one that is indented, spans lines oddly, or has text
/// outside its cells, as in a row with more cells than the header.
fn align(cx: &Context<'_>, table: &Table) -> Option<Vec<Edit>> {
    let source = cx.source;
    let lines = &cx.lines;
    let first = lines.of(table.range.start);
    if lines.start(first) != table.range.start || table.rows.is_empty() {
        return None;
    }
    let columns = table.alignments.len();
    // Each row's cells, trimmed, padded to the header's width.
    let mut rows: Vec<(usize, Vec<Range<usize>>)> = Vec::with_capacity(table.rows.len());
    for (ix, (row, cells)) in table.rows.iter().enumerate() {
        let line = lines.of(row.start);
        // The delimiter row sits between the header and the first row.
        let expected = if ix == 0 { first } else { first + 1 + ix };
        if line != expected || cells.len() != columns {
            return None;
        }
        let content = lines.content(source, line);
        // A missing cell is reported, empty, past the line's end.
        let cells: Vec<Range<usize>> = cells
            .iter()
            .map(|cell| {
                trim(
                    source,
                    &(cell.start.min(content.end)..cell.end.min(content.end)),
                )
            })
            .collect();
        // Text outside the cells, other than pipes and spaces, would be lost.
        let mut at = content.start;
        for cell in cells.iter().chain([&(content.end..content.end)]) {
            if cell.start < at || !only(&source[at..cell.start], &['|', ' ', '\t']) {
                return None;
            }
            at = cell.end.max(at);
        }
        rows.push((line, cells));
    }
    let delimiter = lines.content(source, first + 1);
    if !only(&source[delimiter.clone()], &['|', '-', ':', ' ', '\t']) {
        return None;
    }

    let widths: Vec<usize> = (0..columns)
        .map(|column| {
            rows.iter()
                .map(|(_, cells)| source[cells[column].clone()].width())
                .max()
                .unwrap_or(0)
                .max(3)
        })
        .collect();

    let mut edits = Vec::new();
    for (line, cells) in &rows {
        let content = lines.content(source, *line);
        let mut gap_start = content.start;
        let mut gap = String::from("| ");
        for (column, cell) in cells.iter().enumerate() {
            let padding = widths[column] - source[cell.clone()].width();
            let (left, right) = match table.alignments[column] {
                Alignment::Right => (padding, 0),
                Alignment::Center => (padding / 2, padding - padding / 2),
                Alignment::Left | Alignment::None => (0, padding),
            };
            gap.extend(std::iter::repeat_n(' ', left));
            replace(source, &mut edits, gap_start..cell.start, &gap);
            gap.clear();
            gap.extend(std::iter::repeat_n(' ', right));
            gap.push_str(if column + 1 == columns { " |" } else { " | " });
            gap_start = cell.end;
        }
        replace(source, &mut edits, gap_start..content.end, &gap);
    }
    let mut row = String::from("|");
    for (column, width) in widths.iter().enumerate() {
        let (left, right) = match table.alignments[column] {
            Alignment::Left => (":", "-"),
            Alignment::Right => ("-", ":"),
            Alignment::Center => (":", ":"),
            Alignment::None => ("-", "-"),
        };
        row.push(' ');
        row.push_str(left);
        row.extend(std::iter::repeat_n('-', width - 2));
        row.push_str(right);
        row.push_str(" |");
    }
    replace(source, &mut edits, delimiter, &row);
    Some(edits)
}

fn replace(source: &str, edits: &mut Vec<Edit>, range: Range<usize>, text: &str) {
    if source[range.clone()] != *text {
        edits.push(Edit {
            range,
            text: text.into(),
        });
    }
}

fn trim(source: &str, cell: &Range<usize>) -> Range<usize> {
    let text = &source[cell.clone()];
    let start = cell.start + (text.len() - text.trim_start().len());
    let end = cell.start + text.trim_end().len();
    start..end.max(start)
}

fn only(text: &str, allowed: &[char]) -> bool {
    text.chars().all(|ch| allowed.contains(&ch))
}

#[cfg(test)]
mod tests {
    use crate::rules::test_support::fixed;

    #[test]
    fn columns_line_up() {
        assert_eq!(
            fixed("table-alignment", "", "|a|bb|\n|:-|-:|\n|ccc|d|\n"),
            "| a   |  bb |\n| :-- | --: |\n| ccc |   d |\n"
        );
        assert_eq!(
            fixed("table-alignment", "", "a | b\n--- | :---:\n1 | 2\n"),
            "| a   |  b  |\n| --- | :-: |\n| 1   |  2  |\n"
        );
    }

    #[test]
    fn wide_characters_count_double() {
        assert_eq!(
            fixed(
                "table-alignment",
                "",
                "| 名字 | a |\n|---|---|\n| x | y |\n"
            ),
            "| 名字 | a   |\n| ---- | --- |\n| x    | y   |\n"
        );
    }

    #[test]
    fn cell_text_is_kept() {
        assert_eq!(
            fixed("table-alignment", "", "| `a\\|b` | x \\| y |\n|---|---|\n"),
            "| `a\\|b` | x \\| y |\n| ------ | ------ |\n"
        );
    }

    #[test]
    fn rows_with_extra_cells_are_left_alone() {
        let source = "| a |\n|---|\n| 1 | 2 |\n";
        assert_eq!(fixed("table-alignment", "", source), source);
    }

    #[test]
    fn missing_cells_are_filled() {
        assert_eq!(
            fixed("table-alignment", "", "| a | b |\n|---|---|\n| 1 |\n"),
            "| a   | b   |\n| --- | --- |\n| 1   |     |\n"
        );
    }
}
