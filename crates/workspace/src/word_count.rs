use std::ops::Range;
use std::time::Duration;

use gpui_kit::component::input::Rope;
use gpui_kit::{
    AppContext as _, Context, IntoElement, ParentElement as _, Render, Task, Window, div,
};
use markdown::WordCounter;

/// Source statistics do not need the preview's parsed document. This view
/// counts shared rope chunks in the background without flattening the file.
#[derive(Default)]
pub(crate) struct WordCount {
    words: usize,
    pending: Option<Rope>,
    task: Option<Task<()>>,
    /// Words in the editor's selection, shown as `selected/total`; `None`
    /// when nothing is selected.
    selected: Option<usize>,
    /// The range `selected` was counted for. `None` after the source
    /// changes, so the same range is counted again in the new text.
    selection: Option<Range<usize>>,
    /// The latest large selection waiting for a background count.
    pending_selection: Option<(Rope, Range<usize>)>,
    selection_task: Option<Task<()>>,
}

impl WordCount {
    pub(crate) fn set_source(&mut self, source: Rope, cx: &mut Context<Self>) {
        self.selection = None;
        if source.len() == 0 {
            self.pending = None;
            self.task = None;
            if self.words != 0 {
                self.words = 0;
                cx.notify();
            }
            return;
        }
        self.pending = Some(source);
        if self.task.is_none() {
            self.start(cx);
        }
    }

    /// Count the words in `range` of `source`. Unlike the total, this is not
    /// debounced: a selection changes on its own, not on every keystroke, and
    /// the count should follow a drag as it goes.
    pub(crate) fn set_selection(
        &mut self,
        source: Rope,
        range: Range<usize>,
        cx: &mut Context<Self>,
    ) {
        let range = char_range(&source, range);
        if self.selection.as_ref() == Some(&range) {
            return;
        }
        self.selection = Some(range.clone());
        if range.len() > INLINE_SELECTION_BYTES {
            self.pending_selection = Some((source, range));
            if self.selection_task.is_none() {
                self.start_selection(cx);
            }
            return;
        }
        // Dropping the task discards a count for an earlier, larger range.
        self.pending_selection = None;
        self.selection_task = None;
        let selected = (!range.is_empty()).then(|| count_chunks(source.slice(range).chunks()));
        self.show_selected(selected, cx);
    }

    /// Count the pending selection in the background, one count at a time:
    /// a running count cannot be stopped, so a drag across a long file would
    /// otherwise stack up a full count per mouse move.
    fn start_selection(&mut self, cx: &mut Context<Self>) {
        let Some((source, range)) = self.pending_selection.take() else {
            return;
        };
        self.selection_task = Some(cx.spawn(async move |this, cx| {
            let words = cx
                .background_spawn(async move { count_chunks(source.slice(range).chunks()) })
                .await;
            this.update(cx, |this, cx| {
                this.selection_task = None;
                this.show_selected(Some(words), cx);
                this.start_selection(cx);
            })
            .ok();
        }));
    }

    fn show_selected(&mut self, selected: Option<usize>, cx: &mut Context<Self>) {
        if self.selected != selected {
            self.selected = selected;
            cx.notify();
        }
    }

    fn start(&mut self, cx: &mut Context<Self>) {
        self.task = Some(cx.spawn(async move |this, cx| {
            // Coalesce typing, keeping just the latest shared source snapshot.
            cx.background_executor()
                .timer(Duration::from_millis(200))
                .await;
            let Ok(Some(source)) = this.update(cx, |this, _| this.pending.take()) else {
                return;
            };
            let words = cx
                .background_spawn(async move { count_chunks(source.chunks()) })
                .await;
            this.update(cx, |this, cx| {
                this.task = None;
                if this.pending.is_some() {
                    this.start(cx);
                } else if this.words != words {
                    this.words = words;
                    cx.notify();
                }
            })
            .ok();
        }));
    }
}

/// Selections up to this size are counted on the UI thread, so the count
/// changes in the same frame as the selection. Counting 16 KiB takes well
/// under a millisecond.
const INLINE_SELECTION_BYTES: usize = 16 * 1024;

/// Clamp `range` into `source` and widen it to character boundaries, since
/// slicing a rope panics on anything else.
fn char_range(source: &Rope, range: Range<usize>) -> Range<usize> {
    let len = source.len();
    let start = range.start.min(range.end).min(len);
    let end = range.end.max(range.start).min(len);
    source.floor_char_boundary(start)..source.ceil_char_boundary(end)
}

fn count_chunks<'a>(chunks: impl Iterator<Item = &'a str>) -> usize {
    let mut counter = WordCounter::default();
    for chunk in chunks {
        counter.push(chunk);
    }
    counter.words()
}

impl Render for WordCount {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        // A selection never holds more words than the whole text, but the
        // total is debounced: selecting all right after a paste would read
        // "300/0 words" until it catches up.
        let total = self.words.max(self.selected.unwrap_or(0));
        let unit = if total == 1 { "word" } else { "words" };
        let total = super::workspace::format_count(total);
        div().child(match self.selected {
            Some(selected) => format!(
                "{}/{total} {unit}",
                super::workspace::format_count(selected)
            ),
            None => format!("{total} {unit}"),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn char_range_clamps_and_widens_to_characters() {
        // "中" spans bytes 1..4.
        let source = Rope::from_str("a中b");
        assert_eq!(char_range(&source, 2..3), 1..4);
        // A reversed range is read as the same span.
        #[allow(clippy::reversed_empty_ranges)]
        let reversed = 3..1;
        assert_eq!(char_range(&source, reversed), 1..4);
        assert_eq!(char_range(&source, 4..99), 4..5);
        assert_eq!(char_range(&source, 99..99), 5..5);
    }

    #[test]
    fn selection_never_counts_more_than_the_whole() {
        let text = "it's 权力 e-mail, ＡＢＣ　x";
        let source = Rope::from_str(text);
        let total = count_chunks(source.chunks());
        for start in 0..=text.len() {
            for end in start..=text.len() {
                let range = char_range(&source, start..end);
                assert!(count_chunks(source.slice(range).chunks()) <= total);
            }
        }
    }
}
