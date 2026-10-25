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
        if self.selection.as_ref() == Some(&range) {
            return;
        }
        self.selection = Some(range.clone());
        if range.is_empty() {
            self.selection_task = None;
            if self.selected.take().is_some() {
                cx.notify();
            }
            return;
        }
        // Replacing the task drops the count for the previous range.
        self.selection_task = Some(cx.spawn(async move |this, cx| {
            let words = cx
                .background_spawn(async move {
                    let end = range.end.min(source.len());
                    count_chunks(source.slice(range.start.min(end)..end).chunks())
                })
                .await;
            this.update(cx, |this, cx| {
                this.selection_task = None;
                if this.selected != Some(words) {
                    this.selected = Some(words);
                    cx.notify();
                }
            })
            .ok();
        }));
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

fn count_chunks<'a>(chunks: impl Iterator<Item = &'a str>) -> usize {
    let mut counter = WordCounter::default();
    for chunk in chunks {
        counter.push(chunk);
    }
    counter.words()
}

impl Render for WordCount {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let total = super::workspace::format_count(self.words);
        let unit = if self.words == 1 { "word" } else { "words" };
        div().child(match self.selected {
            Some(selected) => format!(
                "{}/{total} {unit}",
                super::workspace::format_count(selected)
            ),
            None => format!("{total} {unit}"),
        })
    }
}
