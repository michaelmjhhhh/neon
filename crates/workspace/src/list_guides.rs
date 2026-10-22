//! The line that hangs from a list item's marker past the lines nested under
//! it, so the editor shows which item a nested line belongs to.

use std::ops::{Range, RangeInclusive};

use gpui_kit::component::ActiveTheme as _;
use gpui_kit::component::input::{EditorState, Rope, RopeExt as _};
use gpui_kit::{
    App, Bounds, ContentMask, Entity, EntityInputHandler as _, Pixels, Point, SharedString,
    TextRun, Window, fill, point, px, size,
};

use crate::list_continuation;
use crate::typography::Typography;

/// How far around the visible rows to look for the items that own them. An
/// item further away than this gets no guide.
const MAX_CONTEXT_ROWS: usize = 1000;

/// A list item with nested lines.
#[derive(Debug, PartialEq)]
struct ListGuide {
    /// The item's row and the byte range of its marker in that row.
    item_row: usize,
    marker: Range<usize>,
    /// The nested rows, without the blank rows after the last one.
    rows: RangeInclusive<usize>,
}

/// The guides of the list items whose nested lines meet `rows`.
fn guides(text: &Rope, rows: Range<usize>) -> Vec<ListGuide> {
    let line = |row: usize| text.slice_line(row).to_string();
    // Only spaces and tabs indent, as in the list parser, so a nested row's
    // indent always ends on a character boundary.
    let indent = |line: &str| {
        (!line.trim().is_empty()).then(|| line.len() - line.trim_start_matches([' ', '\t']).len())
    };
    // Start at a line nothing is nested under, so every open item is known.
    let mut start = rows.start;
    while start > 0
        && rows.start - start < MAX_CONTEXT_ROWS
        && indent(&line(start)).is_none_or(|indent| indent > 0)
    {
        start -= 1;
    }
    let end = (rows.end + MAX_CONTEXT_ROWS).min(text.lines_len());

    struct Open {
        row: usize,
        indent: usize,
        marker: Option<Range<usize>>,
        last_nested: Option<usize>,
    }
    let mut open: Vec<Open> = Vec::new();
    let mut guides = Vec::new();
    let mut close = |item: Open| {
        if let (Some(marker), Some(last)) = (item.marker, item.last_nested)
            && item.row < rows.end
            && last >= rows.start
        {
            guides.push(ListGuide {
                item_row: item.row,
                marker,
                rows: item.row + 1..=last,
            });
        }
    };
    for row in start..end {
        let line = line(row);
        let Some(indent) = indent(&line) else {
            continue;
        };
        while open.last().is_some_and(|item| item.indent >= indent) {
            close(open.pop().unwrap());
        }
        if open.is_empty() && row >= rows.end {
            break;
        }
        for item in &mut open {
            item.last_nested = Some(row);
        }
        open.push(Open {
            row,
            indent,
            marker: list_continuation::marker(&line),
            last_nested: None,
        });
    }
    open.into_iter().rev().for_each(close);
    guides
}

/// Paints the guides of the rows the editor last laid out, within `clip`.
///
/// Each guide runs down the middle of its item's marker, from just under the
/// item to just above the line after its last nested row. The innermost guide
/// around the cursor is drawn darker, so the item being edited stands out.
pub(crate) fn paint(
    editor: &Entity<EditorState>,
    typography: &Entity<Typography>,
    clip: Bounds<Pixels>,
    window: &mut Window,
    cx: &mut App,
) {
    let state = editor.read(cx);
    let (Some(text_bounds), Some(visible), Some(line_height)) = (
        state.text_bounds(),
        state.visible_row_range(),
        state.line_height(),
    ) else {
        return;
    };
    if visible.is_empty() {
        return;
    }
    let text = state.text().clone();
    let cursor_row = text.offset_to_point(state.cursor()).row;
    let guides = guides(&text, visible.clone());
    if guides.is_empty() {
        return;
    }

    let origin = |offset: usize, window: &mut Window, cx: &mut App| {
        self::origin(editor, &text, offset, text_bounds, window, cx)
    };
    let tops = row_tops(editor, &text, visible.clone(), line_height, window, cx);
    let row_top = |row: usize| tops[row - visible.start];
    let is_blank = |row: usize| text.slice_line(row).to_string().trim().is_empty();

    let typography = typography.read(cx);
    let font = gpui_kit::font(typography.editor_font(cx));
    let font_size = window.rem_size() * (typography.editor_size(cx) / 16.);
    let scale = window.scale_factor();
    // Clear of the item's marker above and the next item below.
    let gap = line_height * 0.2;

    let mut lines = Vec::new();
    for guide in guides {
        let first = (*guide.rows.start()).max(visible.start);
        let last = (*guide.rows.end()).min(visible.end - 1);
        let shown: Vec<(usize, Pixels)> = (first..=last)
            .filter_map(|row| Some((row, row_top(row)?)))
            .collect();
        let (Some(&(_, top)), Some(&(bottom_row, _))) = (shown.first(), shown.last()) else {
            continue;
        };
        // A nested row starts with at least the item's indent, so the
        // marker's left edge lines up with the same offset in that row.
        let Some(&(measured_row, _)) = shown.iter().find(|(row, _)| !is_blank(*row)) else {
            continue;
        };
        let left = origin(
            text.line_start_offset(measured_row) + guide.marker.start,
            window,
            cx,
        );
        let bottom = origin(text.line_end_offset(bottom_row), window, cx);
        let (Some(left), Some(bottom)) = (left, bottom) else {
            continue;
        };
        let marker = text.slice_line(guide.item_row).to_string()[guide.marker.clone()].to_string();
        let marker_width = window
            .text_system()
            .shape_line(
                SharedString::from(marker.clone()),
                font_size,
                &[TextRun {
                    len: marker.len(),
                    font: font.clone(),
                    color: gpui_kit::black(),
                    background_color: None,
                    underline: None,
                    strikethrough: None,
                }],
                None,
            )
            .width;

        let mut y_top = top;
        if *guide.rows.start() >= visible.start {
            y_top += gap;
        }
        let mut y_bottom = bottom.y + line_height;
        if *guide.rows.end() < visible.end {
            y_bottom -= gap;
        }
        if y_bottom <= y_top {
            continue;
        }
        // Snapped to device pixels, so the line stays one crisp pixel wide.
        let x = ((left.x + marker_width / 2. - px(0.5)) * scale).round() / scale;
        let bounds = Bounds::new(point(x, y_top), size(px(1.), y_bottom - y_top));
        let around_cursor = (guide.item_row..=*guide.rows.end()).contains(&cursor_row);
        lines.push((guide.item_row, bounds, around_cursor));
    }

    let active = lines
        .iter()
        .filter(|(_, _, around_cursor)| *around_cursor)
        .map(|(item_row, _, _)| *item_row)
        .max();
    let theme = cx.theme();
    let (quiet, strong) = (theme.border, theme.muted_foreground.opacity(0.45));
    window.with_content_mask(Some(ContentMask { bounds: clip }), |window| {
        for (item_row, bounds, _) in lines {
            let color = if active == Some(item_row) {
                strong
            } else {
                quiet
            };
            window.paint_quad(fill(bounds, color));
        }
    });
}

/// Where `offset` is drawn, from the editor's layout for this frame.
fn origin(
    editor: &Entity<EditorState>,
    text: &Rope,
    offset: usize,
    text_bounds: Bounds<Pixels>,
    window: &mut Window,
    cx: &mut App,
) -> Option<Point<Pixels>> {
    let offset = text.offset_to_offset_utf16(offset);
    editor
        .update(cx, |editor, cx| {
            editor.bounds_for_range(offset..offset, text_bounds, window, cx)
        })
        .map(|bounds| bounds.origin)
}

/// The top of each of the laid-out `rows`, or `None` for a row a fold hides.
///
/// The editor skips hidden rows, and places an offset in one at the start of
/// the next row it laid out, or at the cursor when none follows. So both ends
/// of a hidden row land on the same point, which never happens to a shown row
/// with text, and a hidden blank row starts where the row after it does.
fn row_tops(
    editor: &Entity<EditorState>,
    text: &Rope,
    rows: Range<usize>,
    line_height: Pixels,
    window: &mut Window,
    cx: &mut App,
) -> Vec<Option<Pixels>> {
    let state = editor.read(cx);
    let Some(text_bounds) = state.text_bounds() else {
        return vec![None; rows.len()];
    };
    let cursor = state.cursor();
    let cursor_row = text.offset_to_point(cursor).row;
    let at = |offset: usize, window: &mut Window, cx: &mut App| {
        origin(editor, text, offset, text_bounds, window, cx)
    };
    let near = |a: Point<Pixels>, b: Point<Pixels>| {
        (a.x - b.x).abs() < px(0.5) && (a.y - b.y).abs() < line_height / 2.
    };
    let cursor_at = at(cursor, window, cx);
    let starts: Vec<Option<Point<Pixels>>> = rows
        .clone()
        .map(|row| at(text.line_start_offset(row), window, cx))
        .collect();
    let mut tops = Vec::with_capacity(rows.len());
    for (ix, row) in rows.enumerate() {
        let Some(start) = starts[ix] else {
            tops.push(None);
            continue;
        };
        let hidden = if !text.slice_line(row).to_string().trim().is_empty() {
            at(text.line_end_offset(row), window, cx).is_some_and(|end| near(start, end))
        } else if row == cursor_row {
            false
        } else {
            let next = starts.get(ix + 1).copied().unwrap_or(cursor_at);
            next.is_some_and(|next| (next.y - start.y).abs() < line_height / 2.)
        };
        tops.push((!hidden).then_some(start.y));
    }
    tops
}
