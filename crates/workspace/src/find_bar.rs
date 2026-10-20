use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{
    EditorState, Escape, IndentInline, Input, InputEvent, InputState, OutdentInline, Replace,
};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, IconName, Selectable as _, Sizable as _, ThemeStyled as _,
};
use gpui_kit::{
    AppContext as _, Context, Entity, Focusable as _, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, SharedString, Styled as _, Subscription, Window, div, rems,
};

const KEY_CONTEXT: &str = "FindBar";
/// The key context the input binds Find and Replace in; tooltips read the
/// shortcut from there.
const INPUT_KEY_CONTEXT: &str = "Input";

/// Find, and optionally replace, in one editor. It floats over the editor's
/// top trailing corner, so opening it does not move the text.
///
/// The editor's own search panel is turned off; this drives the same search
/// engine, so matches are highlighted as before.
pub(crate) struct FindBar {
    editor: Entity<EditorState>,
    query: Entity<InputState>,
    replacement: Entity<InputState>,
    open: bool,
    replacing: bool,
    match_case: bool,
    /// The editor is read-only for now, so replacing is off.
    readonly: bool,
    /// The count names a current match the editor has not scrolled to yet:
    /// the query changed since the last move. The next move goes to it.
    unrevealed: bool,
    _subscriptions: [Subscription; 3],
}

impl FindBar {
    pub(crate) fn new(
        editor: Entity<EditorState>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let query = cx.new(|cx| InputState::new(window, cx).placeholder("Find"));
        let replacement = cx.new(|cx| InputState::new(window, cx).placeholder("Replace with"));
        let subscriptions = [
            cx.subscribe(&query, |this, _, event: &InputEvent, cx| match event {
                InputEvent::Change => this.search(cx),
                InputEvent::PressEnter { shift: true, .. } => this.previous(cx),
                InputEvent::PressEnter { .. } => this.next(cx),
                _ => {}
            }),
            cx.subscribe_in(
                &replacement,
                window,
                |this, _, event: &InputEvent, window, cx| {
                    if matches!(event, InputEvent::PressEnter { .. }) {
                        this.replace_one(window, cx);
                    }
                },
            ),
            // The count follows edits to the document.
            cx.observe(&editor, |this, _, cx| {
                if this.open {
                    cx.notify();
                }
            }),
        ];
        Self {
            editor,
            query,
            replacement,
            open: false,
            replacing: false,
            match_case: false,
            readonly: false,
            unrevealed: false,
            _subscriptions: subscriptions,
        }
    }

    pub(crate) fn set_readonly(&mut self, readonly: bool, cx: &mut Context<Self>) {
        self.readonly = readonly;
        cx.notify();
    }

    fn can_replace(&self, cx: &gpui_kit::App) -> bool {
        !self.readonly && self.editor.read(cx).is_replaceable()
    }

    /// Show the bar with the editor's selection as the query, or the last
    /// query, selected so typing replaces it.
    pub(crate) fn show(&mut self, replacing: bool, window: &mut Window, cx: &mut Context<Self>) {
        let selection = self.editor.read(cx).selected_text().to_string();
        if !selection.is_empty() && !selection.contains('\n') {
            self.query
                .update(cx, |query, cx| query.set_value(selection, window, cx));
        }
        self.open = true;
        self.replacing = replacing && self.can_replace(cx);
        self.query.update(cx, |query, cx| {
            query.focus(window, cx);
            query.select_all(window, cx);
        });
        self.search(cx);
    }

    fn hide(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open = false;
        self.editor.update(cx, |editor, cx| {
            editor.close_search(cx);
            editor.focus(window, cx);
        });
        cx.notify();
    }

    fn search(&mut self, cx: &mut Context<Self>) {
        let query = self.query.read(cx).value().to_string();
        let case_insensitive = !self.match_case;
        self.editor.update(cx, |editor, cx| {
            editor.set_search_query(query, case_insensitive, cx)
        });
        self.unrevealed = true;
        cx.notify();
    }

    fn next(&mut self, cx: &mut Context<Self>) {
        let reveal = std::mem::take(&mut self.unrevealed);
        self.editor.update(cx, |editor, cx| {
            // Step back first so the move lands on the match the count names.
            if reveal {
                editor.previous_search_match(cx);
            }
            editor.next_search_match(cx);
        });
    }

    fn previous(&mut self, cx: &mut Context<Self>) {
        self.unrevealed = false;
        self.editor
            .update(cx, |editor, cx| editor.previous_search_match(cx));
    }

    fn replace_one(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.can_replace(cx) {
            return;
        }
        self.unrevealed = false;
        let replacement = self.replacement.read(cx).value();
        self.editor.update(cx, |editor, cx| {
            editor.replace_current_search_match(&replacement, window, cx)
        });
    }

    fn replace_all(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.can_replace(cx) {
            return;
        }
        let replacement = self.replacement.read(cx).value();
        self.editor.update(cx, |editor, cx| {
            editor.replace_all_search_matches(&replacement, window, cx)
        });
    }

    fn toggle_replacing(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.can_replace(cx) {
            return;
        }
        self.replacing = !self.replacing;
        let field = if self.replacing {
            &self.replacement
        } else {
            &self.query
        };
        field.update(cx, |field, cx| field.focus(window, cx));
        cx.notify();
    }

    /// Tab moves between the two fields and stays in the bar.
    fn switch_field(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.replacing {
            return;
        }
        let in_query = self.query.read(cx).focus_handle(cx).is_focused(window);
        let field = if in_query {
            &self.replacement
        } else {
            &self.query
        };
        field.update(cx, |field, cx| field.focus(window, cx));
    }

    /// `3 of 12`, or why there is no count.
    fn count(&self, cx: &gpui_kit::App) -> Option<SharedString> {
        if self.query.read(cx).value().is_empty() {
            return None;
        }
        let matcher = &self.editor.read(cx).search_session().matcher;
        Some(match matcher.current() {
            Some(ix) => format!("{} of {}", ix + 1, matcher.len()).into(),
            None => "No results".into(),
        })
    }
}

impl Render for FindBar {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.open {
            return div().into_any_element();
        }
        let can_replace = self.can_replace(cx);
        let replacing = self.replacing && can_replace;
        let has_matches = !self.editor.read(cx).search_session().matcher.is_empty();
        let count = self.count(cx);
        let theme = cx.theme();
        // The controls beside each field share one column, so the replace
        // buttons line up under the count and the arrows.
        let controls = || {
            div()
                .flex()
                .flex_none()
                .items_center()
                .gap_0p5()
                .w(rems(9.5))
        };
        // The disclosure's column; the replace row leaves it empty so both
        // fields start at the same edge.
        let lead = || div().flex().flex_none().justify_center().w_5();
        let replace_label = if replacing {
            "Hide replace"
        } else {
            "Show replace"
        };

        let find_row = div()
            .flex()
            .items_center()
            .gap_1p5()
            .child(
                lead().child(
                    Button::new("toggle-replace")
                        .xsmall()
                        .ghost()
                        .icon(if replacing {
                            IconName::ChevronDown
                        } else {
                            IconName::ChevronRight
                        })
                        .disabled(!can_replace)
                        .accessibility_label(replace_label)
                        .tooltip_with_action(replace_label, &Replace, Some(INPUT_KEY_CONTEXT))
                        .on_click(
                            cx.listener(|this, _, window, cx| this.toggle_replacing(window, cx)),
                        ),
                ),
            )
            .child(
                Input::new(&self.query)
                    .small()
                    .flex_1()
                    .min_w_0()
                    .aria_label("Find")
                    .suffix(
                        Button::new("match-case")
                            .xsmall()
                            .ghost()
                            .icon(IconName::CaseSensitive)
                            .selected(self.match_case)
                            .accessibility_label("Match case")
                            .tooltip("Match case")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.match_case = !this.match_case;
                                this.search(cx);
                            })),
                    ),
            )
            .child(
                controls()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .children(count),
                    )
                    .child(
                        Button::new("previous-match")
                            .xsmall()
                            .ghost()
                            .icon(IconName::ChevronUp)
                            .disabled(!has_matches)
                            .accessibility_label("Previous match")
                            .tooltip("Previous match")
                            .on_click(cx.listener(|this, _, _, cx| this.previous(cx))),
                    )
                    .child(
                        Button::new("next-match")
                            .xsmall()
                            .ghost()
                            .icon(IconName::ChevronDown)
                            .disabled(!has_matches)
                            .accessibility_label("Next match")
                            .tooltip("Next match")
                            .on_click(cx.listener(|this, _, _, cx| this.next(cx))),
                    )
                    .child(
                        Button::new("close-find")
                            .xsmall()
                            .ghost()
                            .icon(IconName::Close)
                            .accessibility_label("Close")
                            .on_click(cx.listener(|this, _, window, cx| this.hide(window, cx))),
                    ),
            );

        let replace_row = replacing.then(|| {
            div()
                .flex()
                .items_center()
                .gap_1p5()
                .child(lead())
                .child(
                    Input::new(&self.replacement)
                        .small()
                        .flex_1()
                        .min_w_0()
                        .aria_label("Replace with"),
                )
                .child(
                    controls()
                        .gap_1()
                        .child(
                            Button::new("replace-one")
                                .xsmall()
                                .outline()
                                .flex_1()
                                .label("Replace")
                                .disabled(!has_matches)
                                .on_click(
                                    cx.listener(|this, _, window, cx| this.replace_one(window, cx)),
                                ),
                        )
                        .child(
                            Button::new("replace-all")
                                .xsmall()
                                .outline()
                                .flex_1()
                                .label("Replace all")
                                .disabled(!has_matches)
                                .on_click(
                                    cx.listener(|this, _, window, cx| this.replace_all(window, cx)),
                                ),
                        ),
                )
        });

        // A row at the top of the editor that only lays the bar out; the bar
        // alone takes the pointer.
        div()
            .absolute()
            .top_0()
            .left_0()
            .right_0()
            .flex()
            .justify_end()
            .pt_2()
            // Clear of the editor's scrollbar.
            .px_4()
            .child(
                div()
                    .id("find-bar")
                    .key_context(KEY_CONTEXT)
                    .occlude()
                    .on_action(cx.listener(|this, _: &Escape, window, cx| this.hide(window, cx)))
                    .on_action(cx.listener(|this, _: &IndentInline, window, cx| {
                        this.switch_field(window, cx)
                    }))
                    .on_action(cx.listener(|this, _: &OutdentInline, window, cx| {
                        this.switch_field(window, cx)
                    }))
                    .popover_style(cx)
                    .w(rems(28.))
                    .max_w_full()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .p_1p5()
                    .child(find_row)
                    .children(replace_row),
            )
            .into_any_element()
    }
}
