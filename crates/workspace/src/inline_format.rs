//! Cmd+B and Cmd+I wrap the selection in Markdown bold or italic markers.

use std::ops::Range;

/// An inline style that Markdown marks with asterisks.
#[derive(Clone, Copy)]
pub(crate) enum Style {
    Bold,
    Italic,
}

impl Style {
    fn marker(self) -> &'static str {
        match self {
            Style::Bold => "**",
            Style::Italic => "*",
        }
    }

    /// Whether a run of `stars` asterisks on both sides applies this style:
    /// `*a*` is italic, `**a**` bold, and `***a***` both.
    fn is_in(self, stars: usize) -> bool {
        match self {
            Style::Bold => stars >= 2,
            Style::Italic => stars % 2 == 1,
        }
    }
}

/// An edit to the document: replace `range` with `text`, then select
/// `selection`.
#[derive(Debug, PartialEq)]
pub(crate) struct Edit {
    pub range: Range<usize>,
    pub text: String,
    pub selection: Range<usize>,
}

/// The edit that toggles `style` on `selection` in `text`.
///
/// `milk` becomes `**milk**` with `milk` still selected. Pressing the key
/// again removes the markers, whether they sit just outside the selection or
/// at its ends. With no selection it inserts an empty pair with the cursor
/// between, or removes that pair if the cursor sits inside one.
pub(crate) fn toggle(text: &str, selection: Range<usize>, style: Style) -> Edit {
    let marker = style.marker();
    let len = marker.len();
    let Range { start, end } = selection;

    let outside = stars_before(&text[..start]).min(stars_after(&text[end..]));
    if style.is_in(outside) {
        return Edit {
            range: start - len..end + len,
            text: text[start..end].to_string(),
            selection: start - len..end - len,
        };
    }

    let selected = &text[start..end];
    let inside = stars_after(selected).min(stars_before(selected));
    // A selection of only asterisks has no text to unwrap.
    if 2 * inside < selected.len() && style.is_in(inside) {
        return Edit {
            range: selection.clone(),
            text: selected[len..selected.len() - len].to_string(),
            selection: start..end - 2 * len,
        };
    }

    Edit {
        range: selection.clone(),
        text: format!("{marker}{selected}{marker}"),
        selection: start + len..end + len,
    }
}

fn stars_before(text: &str) -> usize {
    text.len() - text.trim_end_matches('*').len()
}

fn stars_after(text: &str) -> usize {
    text.len() - text.trim_start_matches('*').len()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Toggle `style` on the part of `text` between `[` and `]`, and return
    /// the document with the new selection marked the same way.
    fn toggle_marked(text: &str, style: Style) -> String {
        let start = text.find('[').unwrap();
        let end = text.find(']').unwrap() - 1;
        let text = text.replace(['[', ']'], "");
        let edit = toggle(&text, start..end, style);
        let mut result = text.clone();
        result.replace_range(edit.range, &edit.text);
        result.insert(edit.selection.end, ']');
        result.insert(edit.selection.start, '[');
        result
    }

    #[test]
    fn wraps_the_selection() {
        assert_eq!(
            toggle_marked("buy [milk] now", Style::Bold),
            "buy **[milk]** now"
        );
        assert_eq!(
            toggle_marked("buy [milk] now", Style::Italic),
            "buy *[milk]* now"
        );
    }

    #[test]
    fn inserts_an_empty_pair_at_the_cursor() {
        assert_eq!(toggle_marked("buy []", Style::Bold), "buy **[]**");
        assert_eq!(toggle_marked("buy []", Style::Italic), "buy *[]*");
    }

    #[test]
    fn removes_markers_around_the_selection() {
        assert_eq!(toggle_marked("**[milk]**", Style::Bold), "[milk]");
        assert_eq!(toggle_marked("*[milk]*", Style::Italic), "[milk]");
        assert_eq!(toggle_marked("**[]**", Style::Bold), "[]");
    }

    #[test]
    fn removes_markers_inside_the_selection() {
        assert_eq!(toggle_marked("[**milk**]", Style::Bold), "[milk]");
        assert_eq!(toggle_marked("[*milk*]", Style::Italic), "[milk]");
    }

    #[test]
    fn bold_and_italic_combine() {
        assert_eq!(toggle_marked("**[milk]**", Style::Italic), "***[milk]***");
        assert_eq!(toggle_marked("*[milk]*", Style::Bold), "***[milk]***");
        assert_eq!(toggle_marked("***[milk]***", Style::Bold), "*[milk]*");
        assert_eq!(toggle_marked("***[milk]***", Style::Italic), "**[milk]**");
    }

    #[test]
    fn a_selection_of_only_asterisks_is_wrapped() {
        assert_eq!(toggle_marked("[**]", Style::Bold), "**[**]**");
        assert_eq!(toggle_marked("[*]", Style::Italic), "*[*]*");
    }
}
