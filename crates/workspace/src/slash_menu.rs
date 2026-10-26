//! The slash menu drawn over the editor: the templates on offer for the
//! command being typed, beside a preview of the highlighted one.
//!
//! Every size comes from the editor's text size, so the menu grows and
//! shrinks with the editor font instead of keeping a fixed UI size.

use std::rc::Rc;

use gpui_kit::component::input::{EditorState, Enter};
use gpui_kit::component::{ActiveTheme as _, ThemeStyled as _};
use gpui_kit::{
    AnyElement, App, AvailableSpace, Bounds, Element, ElementId, Entity, GlobalElementId,
    HighlightStyle, InspectorElementId, InteractiveElement as _, IntoElement, LayoutId,
    MouseButton, ParentElement as _, Pixels, SharedString, Size, Style, Styled as _, StyledText,
    TextRun, WeakEntity, Window, deferred, div, point, prelude::FluentBuilder as _, relative,
};

use crate::slash_commands::{MenuItem, SlashMenu};

/// Menu text relative to the editor's text.
const TEXT_SCALE: f32 = 0.875;
/// Width ranges of the list and the preview, in menu text sizes. Each fits
/// its content within its range.
const LIST_WIDTH: (f32, f32) = (12., 24.);
const PREVIEW_WIDTH: (f32, f32) = (12., 26.);
/// Space between the caret's line and the menu, and between the menu and
/// the window's edges, in menu text sizes.
const GAP: f32 = 0.25;
const MARGIN: f32 = 0.5;

/// The menu over `editor`, when it is open there. `font` and `text_size` are
/// the editor's.
pub(crate) fn render(
    menu: &Entity<SlashMenu>,
    editor: &Entity<EditorState>,
    font: SharedString,
    text_size: Pixels,
    cx: &App,
) -> Option<AnyElement> {
    let (start, items, selected) = menu.read(cx).shown(editor.entity_id())?;
    let items: Rc<[MenuItem]> = items.into();
    let size = text_size * TEXT_SCALE;
    let menu = menu.clone();
    let build = move |stacked: bool, cx: &App| {
        let theme = cx.theme();
        let list = div()
            .popover_style(cx)
            .occlude()
            .min_w(size * LIST_WIDTH.0)
            .max_w(size * LIST_WIDTH.1)
            .p(size * 0.25)
            .flex()
            .flex_col()
            .text_size(size)
            .line_height(relative(1.4))
            .children(items.iter().enumerate().map(|(ix, item)| {
                let menu = menu.clone();
                div()
                    .id(ix)
                    .flex()
                    .items_baseline()
                    .gap(size * 0.5)
                    .px(size * 0.5)
                    .py(size * 0.25)
                    .rounded(theme.radius / 2.)
                    .whitespace_nowrap()
                    .map(|row| {
                        if ix == selected {
                            row.bg(theme.accent).text_color(theme.accent_foreground)
                        } else {
                            row.hover(|row| row.bg(theme.accent.opacity(0.5)))
                        }
                    })
                    .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                        // Keeps focus in the editor, which takes the Enter.
                        window.prevent_default();
                        cx.stop_propagation();
                        menu.update(cx, |menu, _| menu.select(ix));
                        let enter = Enter {
                            secondary: false,
                            shift: false,
                        };
                        window.dispatch_action(Box::new(enter), cx);
                    })
                    .child(StyledText::new(item.label.clone()).with_highlights([(
                        0..item.matched,
                        HighlightStyle {
                            color: Some(theme.blue),
                            ..Default::default()
                        },
                    )]))
                    .child(
                        div()
                            .min_w_0()
                            .overflow_hidden()
                            .text_ellipsis()
                            .text_color(if ix == selected {
                                theme.accent_foreground.opacity(0.75)
                            } else {
                                theme.muted_foreground
                            })
                            .child(item.detail.clone()),
                    )
            }));

        let item = &items[selected];
        let code_size = size * 0.9;
        let preview = div()
            .popover_style(cx)
            .occlude()
            .min_w(size * PREVIEW_WIDTH.0)
            .max_w(size * PREVIEW_WIDTH.1)
            .p(size * 0.75)
            .flex()
            .flex_col()
            .gap(size * 0.5)
            .child(
                div()
                    .overflow_hidden()
                    .font_family(theme.mono_font_family.clone())
                    .text_size(code_size)
                    .children(item.preview.lines().map(|line| {
                        div()
                            .h(code_size * 1.5)
                            .whitespace_nowrap()
                            .child(line.to_string())
                    })),
            )
            .when_some(item.hint, |preview, hint| {
                preview.child(
                    div()
                        .text_size(size)
                        .line_height(relative(1.4))
                        .text_color(theme.muted_foreground)
                        .child(hint_text(hint, theme.foreground)),
                )
            });

        div()
            .flex()
            .when(stacked, |this| this.flex_col())
            .items_start()
            .gap(size * GAP)
            .text_color(theme.popover_foreground)
            // A click between the items keeps focus in the editor too.
            .on_mouse_down(MouseButton::Left, |_, window, cx| {
                window.prevent_default();
                cx.stop_propagation();
            })
            .child(list)
            .child(preview)
            .into_any_element()
    };
    let popup = CaretPopup {
        editor: editor.downgrade(),
        start,
        font,
        text_size,
        size,
        build: Box::new(build),
        content: None,
    };
    Some(deferred(popup).with_priority(1).into_any_element())
}

/// `hint` with its backticks dropped and the examples they marked shown in
/// `example`, against the muted rest.
fn hint_text(hint: &str, example: gpui_kit::Hsla) -> StyledText {
    let mut text = String::new();
    let mut highlights = Vec::new();
    for (ix, part) in hint.split('`').enumerate() {
        if ix % 2 == 1 {
            let style = HighlightStyle {
                color: Some(example),
                ..Default::default()
            };
            highlights.push((text.len()..text.len() + part.len(), style));
        }
        text.push_str(part);
    }
    StyledText::new(text).with_highlights(highlights)
}

/// Builds the menu, side by side or, with `true`, stacked.
type BuildMenu = Box<dyn Fn(bool, &App) -> AnyElement>;

/// The menu placed under the command's `/`, from the editor's caret in the
/// frame being drawn.
///
/// The editor's prepaint records its caret, and a deferred element prepaints
/// after it, so the menu follows the caret as text is typed and the editor
/// scrolls, without trailing a frame behind.
struct CaretPopup {
    editor: WeakEntity<EditorState>,
    /// Where the command's `/` is.
    start: usize,
    /// The editor's font and text size, to measure the command typed.
    font: SharedString,
    text_size: Pixels,
    /// The menu's text size.
    size: Pixels,
    /// Stacked when the window is too narrow for side by side.
    build: BuildMenu,
    content: Option<AnyElement>,
}

impl CaretPopup {
    /// The left end of the caret's line, at the `/`, and its top and bottom.
    fn anchor(
        &self,
        editor: &EditorState,
        window: &mut Window,
    ) -> Option<(Pixels, Pixels, Pixels)> {
        let geometry = editor.prepaint_caret_geometry()?;
        let caret = geometry.cursor_bounds();
        let top = caret.origin.y + geometry.scroll_offset().y;
        let cursor = editor.cursor();
        let typed = if self.start <= cursor {
            editor.text().slice(self.start..cursor).to_string()
        } else {
            String::new()
        };
        let run = TextRun {
            len: typed.len(),
            font: gpui_kit::font(self.font.clone()),
            color: gpui_kit::black(),
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let typed_width = window
            .text_system()
            .shape_line(typed.into(), self.text_size, &[run], None)
            .width;
        let left = (caret.origin.x - typed_width).max(geometry.input_bounds().origin.x);
        Some((left, top, top + geometry.line_height()))
    }
}

impl Element for CaretPopup {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        // The menu is laid out in prepaint, once the caret is known.
        (window.request_layout(Style::default(), [], cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        let Some(editor) = self.editor.upgrade() else {
            return;
        };
        let Some((left, top, bottom)) = self.anchor(editor.read(cx), window) else {
            return;
        };
        let viewport = window.viewport_size();
        let (gap, margin) = (self.size * GAP, self.size * MARGIN);
        let side_by_side = self.size * (LIST_WIDTH.1 + GAP + PREVIEW_WIDTH.1);
        let stacked = side_by_side + margin * 2. > viewport.width;
        let mut content = (self.build)(stacked, cx);
        let size: Size<Pixels> = content.layout_as_root(
            Size {
                width: AvailableSpace::MinContent,
                height: AvailableSpace::MinContent,
            },
            window,
            cx,
        );
        let x = left.min(viewport.width - margin - size.width).max(margin);
        // Under the caret's line, or over it when there is no room below.
        let below = bottom + gap;
        let y = if below + size.height + margin <= viewport.height {
            below
        } else {
            (top - gap - size.height).max(margin)
        };
        content.prepaint_at(point(x, y), window, cx);
        self.content = Some(content);
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        if let Some(content) = self.content.as_mut() {
            content.paint(window, cx);
        }
    }
}

impl IntoElement for CaretPopup {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}
