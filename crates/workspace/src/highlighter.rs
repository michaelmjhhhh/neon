//! Syntax highlighting that keeps parsing off the keystroke path.
//!
//! gpui-component's default highlighter tries a synchronous tree-sitter parse
//! of up to 2 ms on every edit, even while a background parse is already
//! pending; in a long document each keystroke pays the whole budget for a
//! parse that then gives up. This follows Zed's `Buffer::reparse` instead:
//!
//! - With no parse pending, try an incremental parse within [`SYNC_BUDGET`].
//! - If that runs out, or a parse is already pending, only shift the existing
//!   tree by the edit (cheap, and highlights stay in place) and let a
//!   background parse catch up.
//! - When the background parse finishes, replay the edits made meanwhile onto
//!   its tree, swap it in, and parse again if there were any.

use std::cell::RefCell;
use std::ops::Range;
use std::rc::Rc;
use std::time::Duration;

use gpui_kit::base::input::{
    EditorState, FoldRange, HighlightStyleResolver, InputEdit, InputHighlighter,
    InputHighlighterFactory, Rope,
};
use gpui_kit::component::highlighter::SyntaxHighlighter;
use gpui_kit::{AppContext as _, Context, HighlightStyle, SharedString, Task, Window};

/// Zed's synchronous parse budget.
const SYNC_BUDGET: Duration = Duration::from_millis(1);

/// The editor only ever holds Markdown.
const LANGUAGE: &str = "markdown";

/// A highlighter for Markdown; other languages are left plain.
pub(crate) fn factory() -> InputHighlighterFactory {
    Rc::new(|language| {
        (language == LANGUAGE)
            .then(|| Box::new(DeferredHighlighter::new(language)) as Box<dyn InputHighlighter>)
    })
}

struct DeferredHighlighter {
    language: SharedString,
    state: Rc<RefCell<State>>,
}

struct State {
    highlighter: SyntaxHighlighter,
    /// The background parse in flight.
    reparse: Option<Task<()>>,
    /// Ordered edits since the background parse took its text. Tree edits only
    /// need their coordinates; the foreground highlighter owns the latest text.
    edits_since_reparse: Vec<tree_sitter::InputEdit>,
}

impl DeferredHighlighter {
    fn new(language: &str) -> Self {
        Self {
            language: language.to_string().into(),
            state: Rc::new(RefCell::new(State {
                highlighter: SyntaxHighlighter::new(language),
                reparse: None,
                edits_since_reparse: Vec::new(),
            })),
        }
    }

    fn start_reparse(
        &self,
        text: &Rope,
        folding: bool,
        window: &mut Window,
        cx: &mut Context<EditorState>,
    ) {
        let language = self.language.clone();
        let text = text.clone();
        let parse = cx.background_spawn(async move {
            let mut highlighter = SyntaxHighlighter::new(&language);
            highlighter.update(None, &text, None);
            let folds = if folding {
                highlighter
                    .tree()
                    .map(|tree| fold_ranges_in(tree, 0..usize::MAX))
                    .unwrap_or_default()
            } else {
                Vec::new()
            };
            (highlighter, folds)
        });
        // State owns this task, so the completion must not keep State alive.
        // Dropping the highlighter then cancels its task and releases its text.
        let state = Rc::downgrade(&self.state);
        let language = self.language.clone();
        let mut state_ref = self.state.borrow_mut();
        state_ref.edits_since_reparse.clear();
        state_ref.reparse = Some(cx.spawn_in(window, async move |editor, cx| {
            let (mut parsed, folds) = parse.await;
            let Some(state) = state.upgrade() else {
                return;
            };
            let caught_up = {
                let mut state = state.borrow_mut();
                let text = state.highlighter.text().clone();
                let edits = std::mem::take(&mut state.edits_since_reparse);
                // edit_tree shifts tree coordinates without reading the text,
                // so intermediate Rope snapshots need not be retained.
                for edit in &edits {
                    parsed.edit_tree(Some(*edit), &text);
                }
                state.highlighter = parsed;
                state.reparse = None;
                edits.is_empty()
            };
            let this = Self { language, state };
            editor
                .update_in(cx, |editor, window, cx| {
                    if !caught_up {
                        let text = this.state.borrow().highlighter.text().clone();
                        this.start_reparse(&text, folding, window, cx);
                        cx.notify();
                    } else {
                        // These folds describe the background snapshot, before
                        // replayed edits. Publish them only after catching up.
                        editor.apply_highlighter_fold_candidates(folds, cx);
                    }
                })
                .ok();
        }));
    }
}

impl InputHighlighter for DeferredHighlighter {
    fn language(&self) -> SharedString {
        self.language.clone()
    }

    fn update(
        &mut self,
        edit: Option<InputEdit>,
        text: &Rope,
        folding: bool,
        window: &mut Window,
        cx: &mut Context<EditorState>,
    ) {
        let edit = edit.map(to_tree_sitter_edit);
        let needs_reparse = {
            let mut state = self.state.borrow_mut();
            if state.reparse.is_some() {
                state.highlighter.edit_tree(edit, text);
                match edit {
                    Some(edit) => {
                        state.edits_since_reparse.push(edit);
                        false
                    }
                    // An unknown change can't be replayed: parse it afresh.
                    None => true,
                }
            } else {
                !state.highlighter.update(edit, text, Some(SYNC_BUDGET))
            }
        };
        if needs_reparse {
            self.start_reparse(text, folding, window, cx);
        }
    }

    fn styles(
        &self,
        range: &Range<usize>,
        resolver: &dyn HighlightStyleResolver,
    ) -> Vec<(Range<usize>, HighlightStyle)> {
        self.state.borrow().highlighter.styles(range, resolver)
    }

    fn fold_ranges(&self, _: &Rope) -> Vec<FoldRange> {
        self.fold_ranges_for_edit(0..usize::MAX, &Rope::new())
    }

    /// Only the nodes around an edit, so typing does not walk the whole tree.
    fn fold_ranges_for_edit(&self, range: Range<usize>, _: &Rope) -> Vec<FoldRange> {
        self.state
            .borrow()
            .highlighter
            .tree()
            .map(|tree| fold_ranges_in(tree, range))
            .unwrap_or_default()
    }
}

fn to_tree_sitter_edit(edit: InputEdit) -> tree_sitter::InputEdit {
    let point =
        |point: gpui_kit::base::input::Point| tree_sitter::Point::new(point.row, point.column);
    tree_sitter::InputEdit {
        start_byte: edit.start_byte,
        old_end_byte: edit.old_end_byte,
        new_end_byte: edit.new_end_byte,
        start_position: point(edit.start_position),
        old_end_position: point(edit.old_end_position),
        new_end_position: point(edit.new_end_position),
    }
}

/// Nodes overlapping `bytes` that span three or more lines, as gpui-component
/// folds them.
fn fold_ranges_in(tree: &tree_sitter::Tree, bytes: Range<usize>) -> Vec<FoldRange> {
    fn collect(node: tree_sitter::Node, bytes: &Range<usize>, ranges: &mut Vec<FoldRange>) {
        if node.end_byte() <= bytes.start || node.start_byte() >= bytes.end {
            return;
        }
        let start = node.start_position().row;
        let end = node.end_position().row;
        if end.saturating_sub(start) < 2 {
            return;
        }
        ranges.push(FoldRange::new(start, end));
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            collect(child, bytes, ranges);
        }
    }

    let root = tree.root_node();
    let mut ranges = Vec::new();
    let mut cursor = root.walk();
    for child in root.named_children(&mut cursor) {
        collect(child, &bytes, &mut ranges);
    }
    ranges.sort_by_key(|range| range.start_line);
    ranges.dedup_by_key(|range| range.start_line);
    ranges
}

#[cfg(test)]
mod tests {
    use gpui_kit::{TestAppContext, VisualTestContext};

    use super::*;

    /// Styles every capture, so a styled run means the parse found syntax.
    struct AnyCapture;

    impl HighlightStyleResolver for AnyCapture {
        fn style(&self, _: &str) -> Option<HighlightStyle> {
            Some(HighlightStyle {
                font_weight: Some(gpui_kit::FontWeight::BOLD),
                ..Default::default()
            })
        }
    }

    fn styled(highlighter: &DeferredHighlighter, text: &str, needle: &str) -> bool {
        let start = text.find(needle).unwrap();
        highlighter
            .styles(&(start..start + needle.len()), &AnyCapture)
            .iter()
            .any(|(_, style)| style.font_weight.is_some())
    }

    #[gpui_kit::test]
    async fn highlights_after_the_background_parse_and_keeps_up_with_edits(
        cx: &mut TestAppContext,
    ) {
        cx.update(gpui_kit::init);
        let window = cx.add_window(EditorState::new);
        let editor = window.root(cx).unwrap();
        let cx = &mut VisualTestContext::from_window(*window, cx);

        // Small enough that the inline Markdown injection parse, which
        // gpui-component abandons after 20 ms, finishes on a slow CI runner.
        let mut text = "## Heading\n\nSome **bold** text and `code`.\n\n".repeat(30);
        let mut highlighter = DeferredHighlighter::new(LANGUAGE);
        let rope = Rope::from(text.as_str());
        editor.update_in(cx, |_, window, cx| {
            highlighter.update(None, &rope, false, window, cx);
        });
        cx.run_until_parked();
        assert!(styled(&highlighter, &text, "## Heading"));
        assert!(styled(&highlighter, &text, "**bold**"));

        // Edit while a background parse is pending: the edits only shift the
        // tree, so highlights stay on the text that moved.
        editor.update_in(cx, |_, window, cx| {
            highlighter.start_reparse(&rope, false, window, cx);
        });
        for insert in ["# Title\n\n", "Intro.\n\n"] {
            text.insert_str(0, insert);
            let zero = gpui_kit::base::input::Point { row: 0, column: 0 };
            let edit = InputEdit {
                start_byte: 0,
                old_end_byte: 0,
                new_end_byte: insert.len(),
                start_position: zero,
                old_end_position: zero,
                new_end_position: gpui_kit::base::input::Point {
                    row: insert.matches('\n').count(),
                    column: 0,
                },
            };
            let after = Rope::from(text.as_str());
            editor.update_in(cx, |_, window, cx| {
                highlighter.update(Some(edit), &after, false, window, cx);
            });
            assert!(highlighter.state.borrow().reparse.is_some());
            assert!(styled(&highlighter, &text, "## Heading"));
        }

        // The parse replays the edits and parses again until it is current.
        cx.run_until_parked();
        let state = highlighter.state.borrow();
        assert!(state.reparse.is_none());
        assert_eq!(state.highlighter.text().to_string(), text);
        drop(state);
        assert!(styled(&highlighter, &text, "# Title"));
        assert!(styled(&highlighter, &text, "**bold**"));
    }

    /// Styles only keywords, which Markdown has none of.
    struct Keywords;

    impl HighlightStyleResolver for Keywords {
        fn style(&self, name: &str) -> Option<HighlightStyle> {
            (name == "keyword").then(|| HighlightStyle {
                font_weight: Some(gpui_kit::FontWeight::BOLD),
                ..Default::default()
            })
        }
    }

    #[gpui_kit::test]
    async fn colors_code_in_fences_with_the_named_grammar(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let window = cx.add_window(EditorState::new);
        let editor = window.root(cx).unwrap();
        let cx = &mut VisualTestContext::from_window(*window, cx);

        let text = "Intro.\n\n```rust\nfn main() {}\n```\n\n```\nfn plain\n```\n";
        let highlighter = DeferredHighlighter::new(LANGUAGE);
        let rope = Rope::from(text);
        editor.update_in(cx, |_, window, cx| {
            highlighter.start_reparse(&rope, false, window, cx);
        });
        cx.run_until_parked();
        let keyword = |at: usize| {
            highlighter
                .styles(&(at..at + 2), &Keywords)
                .iter()
                .any(|(_, style)| style.font_weight.is_some())
        };
        assert!(keyword(text.find("fn main").unwrap()));
        assert!(!keyword(text.find("fn plain").unwrap()));
    }
}
