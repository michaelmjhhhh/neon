//! The line that hangs from a list item's marker past the lines nested under
//! it, so the editor shows which item a nested line belongs to.

use std::ops::{Range, RangeInclusive};

use gpui_kit::component::ActiveTheme as _;
use gpui_kit::component::input::{EditorState, Rope, RopeExt as _};
use gpui_kit::{
    App, Bounds, ContentMask, Entity, EntityInputHandler as _, Pixels, SharedString, TextRun,
    Window, fill, point, px, size,
};

use crate::list_continuation;
use crate::typography::Typography;

/// How far around the visible rows to look for the items that own them. A
/// guide whose item is further up starts at the top of the editor anyway.
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
    let indent =
        |line: &str| (!line.trim().is_empty()).then(|| line.len() - line.trim_start().len());
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
    let active = guides
        .iter()
        .filter(|guide| (guide.item_row..=*guide.rows.end()).contains(&cursor_row))
        .map(|guide| guide.item_row)
        .max();

    // Where `offset` is drawn. Only offsets in the laid-out rows have one.
    let origin = |offset: usize, window: &mut Window, cx: &mut App| {
        let offset = text.offset_to_offset_utf16(offset);
        editor
            .update(cx, |editor, cx| {
                editor.bounds_for_range(offset..offset, text_bounds, window, cx)
            })
            .map(|bounds| bounds.origin)
    };
    let typography = typography.read(cx);
    let font = gpui_kit::font(typography.editor_font(cx));
    let font_size = window.rem_size() * (typography.editor_size(cx) / 16.);
    let theme = cx.theme();
    let (quiet, strong) = (theme.border, theme.muted_foreground.opacity(0.45));
    let scale = window.scale_factor();

    window.with_content_mask(Some(ContentMask { bounds: clip }), |window| {
        for guide in &guides {
            let first = (*guide.rows.start()).max(visible.start);
            let last = (*guide.rows.end()).min(visible.end - 1);
            // The first nested row on screen starts with at least the item's
            // indent, so the marker's left edge lines up with that row's.
            let Some(measured_row) =
                (first..=last).find(|&row| !text.slice_line(row).to_string().trim().is_empty())
            else {
                continue;
            };
            let marker_start = text.line_start_offset(measured_row) + guide.marker.start;
            let top = origin(text.line_start_offset(first), window, cx);
            let bottom = origin(text.line_end_offset(last), window, cx);
            let left = origin(marker_start, window, cx);
            let (Some(top), Some(bottom), Some(left)) = (top, bottom, left) else {
                continue;
            };
            let marker =
                text.slice_line(guide.item_row).to_string()[guide.marker.clone()].to_string();
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

            // Clear of the item's marker above and the next item below.
            let gap = line_height * 0.2;
            let mut y_top = top.y;
            if first == *guide.rows.start() {
                y_top += gap;
            }
            let mut y_bottom = bottom.y + line_height;
            if last == *guide.rows.end() {
                y_bottom -= gap;
            }
            if y_bottom <= y_top {
                continue;
            }
            let x = ((left.x + marker_width / 2.) * scale).round() / scale;
            let color = if active == Some(guide.item_row) {
                strong
            } else {
                quiet
            };
            window.paint_quad(fill(
                Bounds::new(point(x - px(0.5), y_top), size(px(1.), y_bottom - y_top)),
                color,
            ));
        }
    });
}
