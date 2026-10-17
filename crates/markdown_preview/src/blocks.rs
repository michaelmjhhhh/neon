//! Turns parsed Markdown blocks into GPUI elements.
//!
//! Rendering runs only for blocks in or near the viewport, on every frame they
//! are visible, so it must stay allocation-light: text is shared from the
//! parsed document and GPUI caches its shaped lines across frames.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use gpui_kit::component::{ActiveTheme as _, Icon, IconName, Sizable as _};
use gpui_kit::{
    AnyElement, App, ElementId, FontStyle, FontWeight, HighlightStyle, Hsla,
    InteractiveElement as _, InteractiveText, IntoElement, ObjectFit, ParentElement as _, Pixels,
    SharedString, StatefulInteractiveElement as _, StrikethroughStyle, Styled as _,
    StyledImage as _, StyledText, UnderlineStyle, Window, div, img, prelude::FluentBuilder as _,
    px, relative, rems,
};
use markdown::{
    Block, BlockKind, CodeBlock, ColumnAlignment, Heading, Image, InlineStyle, InlineText,
    MarkdownList, Table,
};

/// Width of the reading column, about 75 characters of body text. Long lines
/// are harder to read than short ones.
const MAX_READING_WIDTH_REMS: f32 = 40.;

pub(crate) fn render_block(
    block: &Block,
    base_dir: Option<&Arc<Path>>,
    font_family: SharedString,
    font_scale: f32,
    _: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let mut renderer = BlockRenderer::new(base_dir, cx);
    renderer.font_scale = font_scale;
    let content = renderer.render_kind(block.kind());
    // A heading sits closer to the text it introduces than to the text
    // before it, so each section reads as one group.
    let is_heading = matches!(block.kind(), BlockKind::Heading(_));

    // The top-level block owns the id namespace for everything inside it, so
    // nested ids only need to be unique within this one block.
    div()
        .id(("block", block.id().as_u64()))
        .font_family(font_family)
        .text_size(rems(font_scale))
        .w_full()
        .flex()
        .justify_center()
        .px_8()
        .child(
            div()
                .w_full()
                .max_w(rems(MAX_READING_WIDTH_REMS))
                .map(|this| if is_heading { this.pb_2() } else { this.pb_4() })
                .child(content),
        )
        .into_any_element()
}

/// The theme values one block needs, read once per block.
struct Palette {
    foreground: Hsla,
    muted: Hsla,
    muted_foreground: Hsla,
    border: Hsla,
    link: Hsla,
    primary: Hsla,
    primary_foreground: Hsla,
    table_head: Hsla,
    code_background: Hsla,
    quote_border: Hsla,
    radius: Pixels,
    radius_lg: Pixels,
    mono_font_family: SharedString,
}

struct BlockRenderer<'a> {
    base_dir: Option<&'a Arc<Path>>,
    palette: Palette,
    /// Counter for element ids inside the block. The block's content is
    /// immutable, so the same content always yields the same ids.
    next_id: usize,
    font_scale: f32,
}

impl<'a> BlockRenderer<'a> {
    fn new(base_dir: Option<&'a Arc<Path>>, cx: &App) -> Self {
        let theme = cx.theme();
        Self {
            base_dir,
            palette: Palette {
                foreground: theme.foreground,
                muted: theme.muted,
                muted_foreground: theme.muted_foreground,
                border: theme.border,
                link: theme.link,
                primary: theme.primary,
                primary_foreground: theme.primary_foreground,
                table_head: theme.table_head,
                code_background: theme.group_box,
                quote_border: theme.muted_foreground.opacity(0.4),
                radius: theme.radius,
                radius_lg: theme.radius_lg,
                mono_font_family: theme.mono_font_family.clone(),
            },
            next_id: 0,
            font_scale: 1.,
        }
    }

    fn next_id(&mut self, kind: &'static str) -> ElementId {
        self.next_id += 1;
        (kind, self.next_id).into()
    }

    fn render_kind(&mut self, kind: &BlockKind) -> AnyElement {
        match kind {
            BlockKind::Heading(heading) => self.render_heading(heading),
            BlockKind::Paragraph(text) => self.render_paragraph(text),
            BlockKind::Image(image) => self.render_image(image),
            BlockKind::Code(code) => self.render_code(code),
            BlockKind::Quote(children) => self.render_quote(children),
            BlockKind::List(list) => self.render_list(list),
            BlockKind::Table(table) => self.render_table(table),
            BlockKind::Footnote { label, children } => self.render_footnote(label, children),
            BlockKind::Html(html) => self.render_html(html),
            BlockKind::Rule => div()
                .w_full()
                .my_4()
                .border_b_1()
                .border_color(self.palette.border)
                .into_any_element(),
        }
    }

    /// Children of a container block, separated like top-level blocks.
    fn render_children(&mut self, children: &[BlockKind]) -> AnyElement {
        let children: Vec<_> = children.iter().map(|kind| self.render_kind(kind)).collect();
        div()
            .flex()
            .flex_col()
            .gap_3()
            .min_w_0()
            .children(children)
            .into_any_element()
    }

    fn render_heading(&mut self, heading: &Heading) -> AnyElement {
        let text = self.render_inline(heading.text());
        let level = heading.level();
        let scale = self.font_scale;
        div()
            .font_weight(match level {
                1 => FontWeight::BOLD,
                2 | 3 => FontWeight::SEMIBOLD,
                _ => FontWeight::MEDIUM,
            })
            .map(|this| match level {
                1 => this
                    .pt_2()
                    .line_height(relative(1.15))
                    .text_size(rems(2. * scale)),
                2 => this
                    .pt_6()
                    .line_height(relative(1.25))
                    .text_size(rems(1.5 * scale)),
                3 => this
                    .pt_4()
                    .line_height(relative(1.3))
                    .text_size(rems(1.25 * scale)),
                4 => this
                    .pt_2()
                    .line_height(relative(1.4))
                    .text_size(rems(1.0625 * scale)),
                5 => this
                    .pt_2()
                    .line_height(relative(1.4))
                    .text_size(rems(scale)),
                _ => this
                    .pt_2()
                    .line_height(relative(1.4))
                    .text_size(rems(0.875 * scale))
                    .text_color(self.palette.muted_foreground),
            })
            .child(text)
            .into_any_element()
    }

    fn render_paragraph(&mut self, text: &InlineText) -> AnyElement {
        div()
            .line_height(relative(1.65))
            .child(self.render_inline(text))
            .into_any_element()
    }

    fn render_inline(&mut self, text: &InlineText) -> AnyElement {
        let palette = &self.palette;
        let highlights = text
            .runs()
            .iter()
            .map(|run| (run.range(), highlight_style(run.style(), palette)));
        let code_fonts = text
            .runs()
            .iter()
            .filter(|run| run.style().contains(InlineStyle::CODE))
            .map(|run| (run.range(), palette.mono_font_family.clone()));
        let styled = StyledText::new(SharedString::from(text.text().clone()))
            .with_highlights(highlights)
            .with_font_family_overrides(code_fonts);

        if text.links().is_empty() {
            return styled.into_any_element();
        }

        let ranges = text.links().iter().map(|link| link.range()).collect();
        let urls: Vec<Arc<str>> = text.links().iter().map(|link| link.url().clone()).collect();
        let base_dir = self.base_dir.cloned();
        InteractiveText::new(self.next_id("inline"), styled)
            .on_click(ranges, move |ix, _, cx| {
                open_link(&urls[ix], base_dir.as_deref(), cx);
            })
            .into_any_element()
    }

    fn render_image(&mut self, image: &Image) -> AnyElement {
        let alt = SharedString::from(image.alt().clone());
        let muted_foreground = self.palette.muted_foreground;
        let font_scale = self.font_scale;
        let fallback = move || {
            div()
                .italic()
                .text_size(rems(0.875 * font_scale))
                .text_color(muted_foreground)
                .child(alt.clone())
                .into_any_element()
        };
        match resolve_image(image.url(), self.base_dir.map(|dir| &**dir)) {
            Some(source) => img(source)
                .max_w_full()
                .object_fit(ObjectFit::ScaleDown)
                .with_fallback(fallback)
                .into_any_element(),
            None => fallback(),
        }
    }

    fn render_code(&mut self, code: &CodeBlock) -> AnyElement {
        let scroll_id = self.next_id("code");
        let palette = &self.palette;
        div()
            .flex()
            .flex_col()
            .rounded(palette.radius_lg)
            .border_1()
            .border_color(palette.border)
            .bg(palette.code_background)
            .overflow_hidden()
            .when_some(code.language(), |this, language| {
                this.child(
                    div()
                        .px_4()
                        .pt_2p5()
                        .text_xs()
                        .text_color(palette.muted_foreground)
                        .child(SharedString::from(language.clone())),
                )
            })
            .child(
                div()
                    .id(scroll_id)
                    .overflow_x_scroll()
                    .px_4()
                    .py_3()
                    .font_family(palette.mono_font_family.clone())
                    .text_size(rems(0.8125 * self.font_scale))
                    .line_height(relative(1.6))
                    .whitespace_nowrap()
                    .child(SharedString::from(code.code().clone())),
            )
            .into_any_element()
    }

    fn render_quote(&mut self, children: &[BlockKind]) -> AnyElement {
        let children = self.render_children(children);
        div()
            .pl_4()
            .border_l_2()
            .border_color(self.palette.quote_border)
            .text_color(self.palette.muted_foreground)
            .child(children)
            .into_any_element()
    }

    fn render_list(&mut self, list: &MarkdownList) -> AnyElement {
        let items: Vec<_> = list
            .items()
            .iter()
            .enumerate()
            .map(|(ix, item)| {
                let marker = match (item.task(), list.start()) {
                    (Some(checked), _) => self.render_task_marker(checked),
                    (None, Some(start)) => div()
                        .text_color(self.palette.muted_foreground)
                        .child(SharedString::from(format!("{}.", start + ix as u64)))
                        .into_any_element(),
                    (None, None) => div()
                        .text_color(self.palette.muted_foreground)
                        .child("•")
                        .into_any_element(),
                };
                let done = item.task() == Some(true);
                let content = self.render_children(item.children());
                div()
                    .flex()
                    .flex_row()
                    .items_start()
                    .gap_2()
                    .line_height(relative(1.65))
                    .when(done, |this| this.text_color(self.palette.muted_foreground))
                    // A fixed lane keeps item text aligned whatever the marker.
                    .child(div().flex_none().w_6().flex().justify_end().child(marker))
                    .child(div().flex_1().min_w_0().child(content))
                    .into_any_element()
            })
            .collect();
        div()
            .flex()
            .flex_col()
            .gap_1()
            .children(items)
            .into_any_element()
    }

    /// A readonly task state. The preview does not edit the source, so this is
    /// an indicator, not a checkbox control.
    fn render_task_marker(&self, checked: bool) -> AnyElement {
        let palette = &self.palette;
        div()
            .mt_1()
            .size_4()
            .flex()
            .items_center()
            .justify_center()
            .rounded(palette.radius * 0.75)
            .border_1()
            .border_color(if checked {
                palette.primary
            } else {
                palette.quote_border
            })
            .when(checked, |this| {
                this.bg(palette.primary).child(
                    Icon::new(IconName::Check)
                        .xsmall()
                        .text_color(palette.primary_foreground),
                )
            })
            .into_any_element()
    }

    fn render_table(&mut self, table: &Table) -> AnyElement {
        let alignments = table.alignments().to_vec();
        let header = self.render_table_row(table.header(), &alignments, true);
        let rows: Vec<_> = table
            .rows()
            .iter()
            .map(|row| self.render_table_row(row, &alignments, false))
            .collect();
        div()
            .id(self.next_id("table"))
            .overflow_x_scroll()
            .rounded(self.palette.radius_lg)
            .border_1()
            .border_color(self.palette.border)
            .child(header)
            .children(rows)
            .into_any_element()
    }

    fn render_table_row(
        &mut self,
        cells: &[InlineText],
        alignments: &[ColumnAlignment],
        is_header: bool,
    ) -> AnyElement {
        let cells: Vec<_> = alignments
            .iter()
            .enumerate()
            .map(|(ix, alignment)| {
                let content = cells.get(ix).map(|cell| self.render_inline(cell));
                div()
                    .flex_1()
                    .min_w_24()
                    .px_3()
                    .py_2()
                    .map(|this| match alignment {
                        ColumnAlignment::Center => this.text_center(),
                        ColumnAlignment::Right => this.text_right(),
                        ColumnAlignment::Left | ColumnAlignment::None => this,
                    })
                    .children(content)
            })
            .collect();
        div()
            .flex()
            .flex_row()
            .when(is_header, |this| {
                this.bg(self.palette.table_head)
                    .font_weight(FontWeight::MEDIUM)
            })
            .when(!is_header, |this| {
                this.border_t_1().border_color(self.palette.border)
            })
            .children(cells)
            .into_any_element()
    }

    fn render_footnote(&mut self, label: &Arc<str>, children: &[BlockKind]) -> AnyElement {
        let children = self.render_children(children);
        div()
            .flex()
            .flex_row()
            .gap_2()
            .text_size(rems(0.875 * self.font_scale))
            .child(
                div()
                    .flex_none()
                    .text_color(self.palette.link)
                    .child(SharedString::from(format!("[{label}]"))),
            )
            .child(div().flex_1().min_w_0().child(children))
            .into_any_element()
    }

    fn render_html(&mut self, html: &Arc<str>) -> AnyElement {
        div()
            .font_family(self.palette.mono_font_family.clone())
            .text_size(rems(0.875 * self.font_scale))
            .text_color(self.palette.muted_foreground)
            .child(SharedString::from(html.clone()))
            .into_any_element()
    }
}

fn highlight_style(style: InlineStyle, palette: &Palette) -> HighlightStyle {
    let mut highlight = HighlightStyle::default();
    if style.contains(InlineStyle::STRONG) {
        highlight.font_weight = Some(FontWeight::BOLD);
    }
    if style.contains(InlineStyle::EMPHASIS) {
        highlight.font_style = Some(FontStyle::Italic);
    }
    if style.contains(InlineStyle::STRIKETHROUGH) {
        highlight.strikethrough = Some(StrikethroughStyle {
            // A one-device-pixel stroke, like a hairline border.
            thickness: px(1.),
            color: Some(palette.foreground),
        });
    }
    if style.contains(InlineStyle::CODE) {
        highlight.background_color = Some(palette.muted);
    }
    if style.contains(InlineStyle::LINK) {
        // The text keeps the reading color; the accent underline marks it.
        highlight.underline = Some(UnderlineStyle {
            thickness: px(1.),
            color: Some(palette.primary),
            wavy: false,
        });
    }
    highlight
}

fn has_scheme(url: &str) -> bool {
    url.split_once(':').is_some_and(|(scheme, _)| {
        scheme.len() > 1
            && scheme
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
    })
}

/// Resolve a document-relative path; absolute paths are kept.
fn resolve_path(url: &str, base_dir: Option<&Path>) -> Option<PathBuf> {
    let path = url
        .split(['#', '?'])
        .next()
        .filter(|path| !path.is_empty())?;
    let path = Path::new(path);
    if path.is_absolute() {
        Some(path.to_path_buf())
    } else {
        base_dir.map(|dir| dir.join(path))
    }
}

fn resolve_image(url: &str, base_dir: Option<&Path>) -> Option<gpui_kit::ImageSource> {
    if url.starts_with("http://") || url.starts_with("https://") {
        return Some(SharedString::from(url.to_string()).into());
    }
    if has_scheme(url) {
        return None;
    }
    resolve_path(url, base_dir).map(Into::into)
}

/// Open a link the way a browser would: web and mail targets go to the
/// system, relative paths open the referenced file, in-page anchors do nothing.
fn open_link(url: &str, base_dir: Option<&Path>, cx: &mut App) {
    if url.starts_with('#') {
        return;
    }
    if has_scheme(url) {
        cx.open_url(url);
    } else if let Some(path) = resolve_path(url, base_dir) {
        cx.open_with_system(&path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_url_schemes_but_not_windows_drives_or_paths() {
        assert!(has_scheme("https://example.com"));
        assert!(has_scheme("mailto:me@example.com"));
        assert!(!has_scheme("notes/today.md"));
        assert!(!has_scheme("C:\\notes.md"));
    }

    #[test]
    fn resolves_relative_paths_against_the_document() {
        let base = Path::new("/docs");
        assert_eq!(
            resolve_path("img/cat.png", Some(base)),
            Some(PathBuf::from("/docs/img/cat.png"))
        );
        assert_eq!(
            resolve_path("other.md#section", Some(base)),
            Some(PathBuf::from("/docs/other.md"))
        );
        assert_eq!(resolve_path("cat.png", None), None);
        assert_eq!(
            resolve_path("/abs/cat.png", None),
            Some(PathBuf::from("/abs/cat.png"))
        );
    }
}
