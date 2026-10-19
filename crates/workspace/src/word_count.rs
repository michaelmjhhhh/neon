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
}

impl WordCount {
    pub(crate) fn set_source(&mut self, source: Rope, cx: &mut Context<Self>) {
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
                .background_spawn(async move {
                    let mut counter = WordCounter::default();
                    for chunk in source.chunks() {
                        counter.push(chunk);
                    }
                    counter.words()
                })
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

impl Render for WordCount {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().child(format!(
            "{} {}",
            super::workspace::format_count(self.words),
            if self.words == 1 { "word" } else { "words" }
        ))
    }
}
