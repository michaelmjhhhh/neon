//! Turns parsed Markdown blocks into GPUI elements.
//!
//! Rendering runs only for blocks in or near the viewport, on every frame they
//! are visible, so it must stay allocation-light: text is shared from the
//! parsed document and GPUI caches its shaped lines across frames.

use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use gpui_kit::assets::IconName as AssetIcon;
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, Sizable as _};
use gpui_kit::{
    AnyElement, App, Bounds, ElementId, Entity, FontStyle, FontWeight, HighlightStyle, Hsla,
    InteractiveElement as _, InteractiveText, IntoElement, ObjectFit, ParentElement as _, Pixels,
    Point, SharedString, StatefulInteractiveElement as _, StrikethroughStyle, Styled as _,
    StyledImage as _, StyledText, TextLayout, UnderlineStyle, canvas, div, fill, img, point,
    prelude::FluentBuilder as _, px, relative, rems, size,
};
use markdown::{
    Block, BlockKind, Callout, CodeBlock, ColumnAlignment, Heading, Image, InlineStyle, InlineText,
    MarkdownList, Property, Table,
};

use crate::image_cache::PreviewImageCache;

/// Width of the reading column, about 70 characters of body text. Long lines
/// are harder to read than short ones.
const MAX_READING_WIDTH_REMS: f32 = 38.;

/// Leading for running text. A little more than the UI's, so paragraphs set
/// in a serif stay open and easy to track from line to line.
const BODY_LINE_HEIGHT: f32 = 1.7;

/// How blocks are set: their typeface and scale, and the column they sit in.
pub(crate) struct BlockLayout {
    pub(crate) font_family: SharedString,
    pub(crate) font_scale: f32,
    /// Leading and trailing insets in place of the centered reading column.
    pub(crate) column_insets: Option<(Pixels, Pixels)>,
}

pub(crate) fn render_block(
    block: &Block,
    row_id: u64,
    list_items: Option<Range<usize>>,
    base_dir: Option<&Arc<Path>>,
    image_cache: &Entity<PreviewImageCache>,
    layout: &BlockLayout,
    cx: &mut App,
) -> AnyElement {
    let BlockLayout {
        font_family,
        font_scale,
        column_insets,
    } = layout;
    let (font_scale, column_insets) = (*font_scale, *column_insets);
    let mut renderer = BlockRenderer::new(base_dir, image_cache, cx);
    renderer.font_scale = font_scale;
    let (content, continues_list) = match (block.kind(), list_items) {
        (BlockKind::List(list), Some(items)) => {
            let continues = items.end < list.items().len();
            (renderer.render_list_items(list, items), continues)
        }
        (kind, _) => (renderer.render_kind(kind), false),
    };
    // A heading sits closer to the text it introduces than to the text
    // before it, so each section reads as one group.
    let heading_level = match block.kind() {
        BlockKind::Heading(heading) => Some(heading.level()),
        _ => None,
    };

    // Each immutable row owns its child identity namespace. Chunked lists use
    // content-derived row ids, so an unchanged chunk keeps its element state.
    div()
        .id(("block", row_id))
        .font_family(font_family.clone())
        .text_size(rems(font_scale))
        .w_full()
        .flex()
        .justify_center()
        .map(|this| match column_insets {
            Some((leading, trailing)) => this.pl(leading).pr(trailing),
            None => this.px_8(),
        })
        .child(
            div()
                .w_full()
                .when(column_insets.is_none(), |this| {
                    this.max_w(rems(MAX_READING_WIDTH_REMS))
                })
                .map(|this| {
                    if continues_list {
                        // This is the same gap as between items in one list.
                        this.pb_1()
                    } else if heading_level == Some(1) {
                        this.pb_3()
                    } else if heading_level.is_some() {
                        this.pb_2()
                    } else {
                        this.pb_4()
                    }
                })
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
    /// Behind `==highlighted==` text: a wash of the accent, or inside a
    /// callout, a deeper wash of the callout's own color.
    highlight: Hsla,
    /// How strong a highlight's wash is; stronger on a dark background.
    highlight_opacity: f32,
    /// Behind inline code: a film of the text color, so the chip shows on
    /// the page and on a tinted callout alike.
    code_chip: Hsla,
    info: Hsla,
    success: Hsla,
    warning: Hsla,
    danger: Hsla,
    table_head: Hsla,
    code_background: Hsla,
    quote_border: Hsla,
    radius: Pixels,
    radius_lg: Pixels,
    mono_font_family: SharedString,
}

struct BlockRenderer<'a> {
    base_dir: Option<&'a Arc<Path>>,
    image_cache: &'a Entity<PreviewImageCache>,
    palette: Palette,
    /// Counter for element ids inside the block. The block's content is
    /// immutable, so the same content always yields the same ids.
    next_id: usize,
    font_scale: f32,
}

impl<'a> BlockRenderer<'a> {
    fn new(
        base_dir: Option<&'a Arc<Path>>,
        image_cache: &'a Entity<PreviewImageCache>,
        cx: &App,
    ) -> Self {
        let theme = cx.theme();
        let highlight_opacity = if theme.is_dark() { 0.3 } else { 0.18 };
        Self {
            base_dir,
            image_cache,
            palette: Palette {
                foreground: theme.foreground,
                muted: theme.muted,
                muted_foreground: theme.muted_foreground,
                border: theme.border,
                link: theme.link,
                primary: theme.primary,
                primary_foreground: theme.primary_foreground,
                highlight: theme.primary.opacity(highlight_opacity),
                highlight_opacity,
                code_chip: theme
                    .foreground
                    .opacity(if theme.is_dark() { 0.1 } else { 0.07 }),
                info: theme.info,
                success: theme.success,
                warning: theme.warning,
                danger: theme.danger,
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
            BlockKind::Callout(callout) => self.render_callout(callout),
            BlockKind::FrontMatter(properties) => self.render_properties(properties),
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

    /// A list item's content. A nested list sits as close to its parent's
    /// text as items sit to each other, so the list reads as one outline.
    fn render_item_children(&mut self, children: &[BlockKind]) -> AnyElement {
        let mut column = div().flex().flex_col().min_w_0();
        for (ix, kind) in children.iter().enumerate() {
            let nested_list = matches!(kind, BlockKind::List(_));
            let element = self.render_kind(kind);
            column = column.child(
                div()
                    .when(ix > 0, |this| {
                        if nested_list {
                            this.mt_1()
                        } else {
                            this.mt_3()
                        }
                    })
                    .child(element),
            );
        }
        column.into_any_element()
    }

    fn render_heading(&mut self, heading: &Heading) -> AnyElement {
        let text = self.render_inline(heading.text());
        let level = heading.level();
        let scale = self.font_scale;
        div()
            // Semibold where the face has it: headings lead by size and
            // space, not by weight alone.
            .font_weight(match level {
                1..=3 => FontWeight::SEMIBOLD,
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
                    .text_size(rems(1.4 * scale)),
                3 => this
                    .pt_4()
                    .line_height(relative(1.3))
                    .text_size(rems(1.175 * scale)),
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
            .line_height(relative(BODY_LINE_HEIGHT))
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
        let backgrounds = run_backgrounds(text, palette);
        let layout = styled.layout().clone();
        let radius = palette.radius * 0.75;

        let element = if text.links().is_empty() {
            styled.into_any_element()
        } else {
            let ranges = text.links().iter().map(|link| link.range()).collect();
            let urls: Vec<Arc<str>> = text.links().iter().map(|link| link.url().clone()).collect();
            let base_dir = self.base_dir.cloned();
            InteractiveText::new(self.next_id("inline"), styled)
                .on_click(ranges, move |ix, window, cx| {
                    open_link(&urls[ix], base_dir.as_deref(), window, cx);
                })
                .into_any_element()
        };
        if backgrounds.is_empty() {
            return element;
        }
        // Code and highlights get rounded backgrounds, painted before the
        // text from its layout; the text's own run backgrounds are square
        // and fill the whole line height.
        let source = text.text().clone();
        div()
            .relative()
            .child(
                canvas(
                    |_, _, _| {},
                    move |_, _, window, _| {
                        paint_run_backgrounds(&layout, &source, &backgrounds, radius, window);
                    },
                )
                .absolute()
                .size_full(),
            )
            .child(element)
            .into_any_element()
    }

    fn render_image(&mut self, image: &Image) -> AnyElement {
        // An image that cannot be shown says which file it looked for, so a
        // wrong path is easy to spot and fix.
        let alt = SharedString::from(image.alt().clone());
        let url = SharedString::from(image.url().clone());
        let palette = &self.palette;
        let (foreground, muted, muted_foreground, border, radius) = (
            palette.foreground,
            palette.muted,
            palette.muted_foreground,
            palette.border,
            palette.radius_lg,
        );
        let font_scale = self.font_scale;
        let fallback = move || {
            div()
                .flex()
                .items_center()
                .gap_3()
                .px_4()
                .py_3()
                .rounded(radius)
                .border_1()
                .border_color(border)
                .bg(muted.opacity(0.5))
                .text_size(rems(0.875 * font_scale))
                .text_color(muted_foreground)
                .child(
                    Icon::new(AssetIcon::ImageOff)
                        .small()
                        .text_color(muted_foreground),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .min_w_0()
                        .when(!alt.is_empty(), |this| {
                            this.child(div().text_color(foreground).child(alt.clone()))
                        })
                        .child(
                            div()
                                .truncate()
                                .child(SharedString::from(format!("Couldn’t load {url}"))),
                        ),
                )
                .into_any_element()
        };
        match resolve_image(image.url(), self.base_dir.map(|dir| &**dir)) {
            Some(source) => img(source)
                .image_cache(self.image_cache)
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

    /// A callout: a tinted panel with its kind's icon and title, in the
    /// color of what it means.
    fn render_callout(&mut self, callout: &Callout) -> AnyElement {
        let (color, icon) = callout_look(callout.kind(), &self.palette);
        // The accent wash would muddy against the panel's tint; a deeper
        // shade of the panel's color marks highlights instead.
        let highlight = std::mem::replace(
            &mut self.palette.highlight,
            color.opacity(self.palette.highlight_opacity + 0.1),
        );
        let title = match callout.title() {
            Some(title) => self.render_inline(title),
            None => SharedString::from(capitalized(callout.kind())).into_any_element(),
        };
        let children =
            (!callout.children().is_empty()).then(|| self.render_children(callout.children()));
        self.palette.highlight = highlight;
        let palette = &self.palette;
        div()
            .flex()
            .flex_col()
            .gap_1p5()
            .px_4()
            .py_3()
            .rounded(palette.radius_lg)
            .bg(color.opacity(0.1))
            .line_height(relative(BODY_LINE_HEIGHT))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(color)
                    .child(icon.small().text_color(color))
                    .child(div().min_w_0().child(title)),
            )
            .children(children)
            .into_any_element()
    }

    /// Front matter as quiet properties above the document: each key in a
    /// muted lane, its value beside it.
    fn render_properties(&mut self, properties: &[Property]) -> AnyElement {
        let palette = &self.palette;
        let rows = properties.iter().map(|property| {
            div()
                .flex()
                .flex_row()
                .gap_4()
                .child(
                    div()
                        .flex_none()
                        .w(rems(7.))
                        .truncate()
                        .text_color(palette.muted_foreground)
                        .child(SharedString::from(property.key().clone())),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(SharedString::from(property.value().clone())),
                )
        });
        div()
            .flex()
            .flex_col()
            .gap_1()
            .pb_4()
            .border_b_1()
            .border_color(palette.border)
            .text_size(rems(0.8125 * self.font_scale))
            .line_height(relative(1.5))
            .children(rows)
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
        self.render_list_items(list, 0..list.items().len())
    }

    fn render_list_items(&mut self, list: &MarkdownList, range: Range<usize>) -> AnyElement {
        let first_item = range.start;
        let items: Vec<_> = list.items()[range]
            .iter()
            .enumerate()
            .map(|(ix, item)| {
                let marker = match (item.task(), list.start()) {
                    (Some(checked), _) => div()
                        // Centered on the first line of the item's text.
                        .mt(rems((BODY_LINE_HEIGHT * self.font_scale - 1.) / 2.))
                        .child(self.render_task_marker(checked))
                        .into_any_element(),
                    (None, Some(start)) => div()
                        .text_color(self.palette.muted_foreground)
                        .child(SharedString::from(format!(
                            "{}.",
                            start + (first_item + ix) as u64
                        )))
                        .into_any_element(),
                    (None, None) => div()
                        .text_color(self.palette.muted_foreground)
                        .child("•")
                        .into_any_element(),
                };
                let done = item.task() == Some(true);
                let content = self.render_item_children(item.children());
                div()
                    .flex()
                    .flex_row()
                    .items_start()
                    .gap_2()
                    .line_height(relative(BODY_LINE_HEIGHT))
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

/// The color and icon for a callout kind, following Obsidian's groups. A kind
/// it does not know looks like a note.
fn callout_look(kind: &str, palette: &Palette) -> (Hsla, Icon) {
    match kind {
        "tip" | "hint" | "important" => (palette.success, Icon::new(AssetIcon::Lightbulb)),
        "success" | "check" | "done" => (palette.success, Icon::new(IconName::CircleCheck)),
        "question" | "help" | "faq" => (palette.warning, Icon::new(AssetIcon::CircleQuestionMark)),
        "warning" | "caution" | "attention" => {
            (palette.warning, Icon::new(IconName::TriangleAlert))
        }
        "failure" | "fail" | "missing" | "danger" | "error" | "bug" => {
            (palette.danger, Icon::new(IconName::CircleX))
        }
        "quote" | "cite" => (palette.muted_foreground, Icon::new(AssetIcon::Quote)),
        _ => (palette.info, Icon::new(IconName::Info)),
    }
}

fn capitalized(word: &str) -> String {
    let mut chars = word.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// The ranges of `text` that sit on a tinted background, and their tints:
/// code on a film of the text color, highlights on the accent wash. Neighboring
/// runs of one tint merge, so `**bold** code` reads as one chip.
fn run_backgrounds(text: &InlineText, palette: &Palette) -> Vec<(Range<usize>, Hsla)> {
    let mut backgrounds: Vec<(Range<usize>, Hsla)> = Vec::new();
    for run in text.runs() {
        let tint = if run.style().contains(InlineStyle::HIGHLIGHT) {
            palette.highlight
        } else if run.style().contains(InlineStyle::CODE) {
            palette.code_chip
        } else {
            continue;
        };
        match backgrounds.last_mut() {
            Some((range, last)) if *last == tint && range.end == run.range().start => {
                range.end = run.range().end;
            }
            _ => backgrounds.push((run.range(), tint)),
        }
    }
    backgrounds
}

/// Paint each background as a rounded chip per visual line it covers, a
/// little shorter than the line and a little wider than its text.
///
/// Positions come from the text's layout, which scans a line per lookup, so
/// this asks only for each range's ends and, when it wraps, binary-searches
/// for where each line breaks.
fn paint_run_backgrounds(
    layout: &TextLayout,
    text: &str,
    backgrounds: &[(Range<usize>, Hsla)],
    radius: Pixels,
    window: &mut gpui_kit::Window,
) {
    let line_height = layout.line_height();
    let height = line_height * 0.78;
    let inset = (line_height - height) / 2.;
    let pad = px(3.);
    let left = layout.bounds().origin.x;
    let mut chip = |start: Point<Pixels>, end_x: Pixels, color: Hsla| {
        let bounds = Bounds::new(
            point(start.x - pad, start.y + inset),
            size(end_x - start.x + pad * 2., height),
        );
        window.paint_quad(fill(bounds, color).corner_radii(radius));
    };
    for (range, color) in backgrounds {
        let Some(slice) = text.get(range.clone()) else {
            continue;
        };
        // Character boundaries of the range, its end included.
        let boundaries: Vec<usize> = slice
            .char_indices()
            .map(|(offset, _)| range.start + offset)
            .chain([range.end])
            .collect();
        let Some(end) = layout.position_for_index(range.end) else {
            continue;
        };
        let mut first = 0;
        while let Some(start) = line_start(layout, &boundaries, first, left) {
            if end.y <= start.y {
                chip(start, end.x, *color);
                break;
            }
            // The last boundary on this line is where it wraps; it reads as
            // the line's end.
            let on_line = boundaries[first + 1..].partition_point(|&ix| {
                layout
                    .position_for_index(ix)
                    .is_some_and(|position| position.y <= start.y)
            });
            if on_line == 0 {
                break;
            }
            first += on_line;
            let Some(wrap) = layout.position_for_index(boundaries[first]) else {
                break;
            };
            chip(start, wrap.x, *color);
        }
    }
}

/// Where the text from `boundaries[ix]` starts. The index where a line
/// wraps reads as the end of the line above, so when the next character is
/// a line lower, the text starts at that line's leading edge.
fn line_start(
    layout: &TextLayout,
    boundaries: &[usize],
    ix: usize,
    left: Pixels,
) -> Option<Point<Pixels>> {
    let here = layout.position_for_index(*boundaries.get(ix)?)?;
    match boundaries
        .get(ix + 1)
        .and_then(|&next| layout.position_for_index(next))
    {
        Some(next) if next.y > here.y => Some(point(left, next.y)),
        _ => Some(here),
    }
}

/// Whether `url` names a scheme, as `https:` does. A URL has no spaces, so
/// a note title like `Re: meeting` is a path, not a scheme.
fn has_scheme(url: &str) -> bool {
    !url.contains(char::is_whitespace)
        && url.split_once(':').is_some_and(|(scheme, _)| {
            scheme.len() > 1
                && scheme
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
        })
}

/// The file a link or image names, without its `#anchor` or `?query`, and
/// with `%20`-style escapes decoded: `My%20Note.md` is `My Note.md`.
fn link_path(url: &str) -> Option<PathBuf> {
    let path = url
        .split(['#', '?'])
        .next()
        .filter(|path| !path.is_empty())?;
    Some(PathBuf::from(percent_decoded(path).into_owned()))
}

/// Resolve a document-relative path; absolute paths are kept.
fn resolve_path(url: &str, base_dir: Option<&Path>) -> Option<PathBuf> {
    let path = link_path(url)?;
    if path.is_absolute() {
        Some(path)
    } else {
        base_dir.map(|dir| dir.join(path))
    }
}

/// `text` with each `%XX` escape decoded; as written if that is not UTF-8.
fn percent_decoded(text: &str) -> std::borrow::Cow<'_, str> {
    if !text.contains('%') {
        return text.into();
    }
    let bytes = text.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut ix = 0;
    while ix < bytes.len() {
        let hex = |offset: usize| {
            bytes
                .get(ix + offset)
                .and_then(|&b| (b as char).to_digit(16))
        };
        match (bytes[ix], hex(1), hex(2)) {
            (b'%', Some(high), Some(low)) => {
                decoded.push((high * 16 + low) as u8);
                ix += 3;
            }
            (byte, _, _) => {
                decoded.push(byte);
                ix += 1;
            }
        }
    }
    String::from_utf8(decoded).map_or(text.into(), Into::into)
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
/// system, another note opens in Neon, other files open in their own app, and
/// in-page anchors do nothing.
fn open_link(url: &str, base_dir: Option<&Path>, window: &mut gpui_kit::Window, cx: &mut App) {
    if url.starts_with('#') {
        return;
    }
    if has_scheme(url) {
        cx.open_url(url);
        return;
    }
    let Some(link) = link_path(url) else {
        return;
    };
    if is_markdown_path(&link) {
        // An unsaved document has no folder of its own; the window looks
        // for the note in the folder it shows.
        let path = resolve_path(url, base_dir).unwrap_or_else(|| link.clone());
        window.dispatch_action(Box::new(crate::OpenLinkedFile { path, link }), cx);
    } else if let Some(path) = resolve_path(url, base_dir) {
        cx.open_with_system(&path);
    }
}

fn is_markdown_path(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("md") || ext.eq_ignore_ascii_case("markdown"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_url_schemes_but_not_windows_drives_or_paths() {
        assert!(has_scheme("https://example.com"));
        assert!(has_scheme("mailto:me@example.com"));
        assert!(!has_scheme("notes/today.md"));
        assert!(!has_scheme("Re: meeting.md"));
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
            resolve_path("My%20Notes/caf%C3%A9.md", Some(base)),
            Some(PathBuf::from("/docs/My Notes/café.md"))
        );
        assert_eq!(
            resolve_path("100%25%zz.md", Some(base)),
            Some(PathBuf::from("/docs/100%%zz.md"))
        );
        assert_eq!(
            resolve_path("/abs/cat.png", None),
            Some(PathBuf::from("/abs/cat.png"))
        );
    }
}
