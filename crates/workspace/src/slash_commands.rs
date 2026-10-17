//! Slash commands: typing `/` at the start of a line opens a menu of Markdown
//! templates, such as a list or a table, and accepting one inserts it in place
//! of the command.

use std::cell::{Cell, RefCell};
use std::ops::Range;

use gpui_kit::component::input::{CompletionProvider, Rope, RopeExt as _};
use gpui_kit::{App, Result, Task, Window};
use lsp_types::{
    CompletionContext, CompletionItem, CompletionItemKind, CompletionResponse, CompletionTextEdit,
    TextEdit,
};

const DEFAULT_LIST_ITEMS: usize = 3;
const MAX_LIST_ITEMS: usize = 50;
const DEFAULT_TABLE: (usize, usize) = (3, 3);
const MAX_TABLE_COLUMNS: usize = 20;
const MAX_TABLE_ROWS: usize = 100;

#[derive(Clone, Copy)]
enum Command {
    Bullet,
    Numbered,
    Todo,
    Code,
    Link,
    Image,
    Table,
}

impl Command {
    const ALL: [Command; 7] = [
        Command::Bullet,
        Command::Numbered,
        Command::Todo,
        Command::Code,
        Command::Link,
        Command::Image,
        Command::Table,
    ];

    fn name(self) -> &'static str {
        match self {
            Command::Bullet => "bullet",
            Command::Numbered => "numbered",
            Command::Todo => "todo",
            Command::Code => "code",
            Command::Link => "link",
            Command::Image => "image",
            Command::Table => "table",
        }
    }

    /// The template for this command with `argument`, the text after the
    /// command name, or `None` when the argument is not valid for it.
    fn template(self, argument: &str) -> Option<Template> {
        match self {
            Command::Bullet => list(argument, "Bulleted list", |_| "- ".into()),
            Command::Numbered => list(argument, "Numbered list", |ix| format!("{}. ", ix + 1)),
            Command::Todo => list(argument, "Task list", |_| "- [ ] ".into()),
            Command::Code => code(argument),
            Command::Link if argument.is_empty() => {
                Some(Template::selecting("Link", "[text](url)", "text"))
            }
            Command::Image if argument.is_empty() => Some(Template::selecting(
                "Image",
                "![alt text](path)",
                "alt text",
            )),
            Command::Link | Command::Image => None,
            Command::Table => table(argument),
        }
    }
}

/// Text to insert, and the range within it to select afterwards.
#[derive(Debug, Clone, PartialEq)]
struct Template {
    label: String,
    detail: String,
    text: String,
    selection: Range<usize>,
}

impl Template {
    fn selecting(detail: &str, text: &str, selected: &str) -> Self {
        let start = text.find(selected).unwrap_or(0);
        Self {
            label: String::new(),
            detail: detail.into(),
            text: text.into(),
            selection: start..start + selected.len(),
        }
    }
}

/// `count` list items, three by default, with the cursor in the first.
fn list(argument: &str, detail: &str, marker: impl Fn(usize) -> String) -> Option<Template> {
    let count = if argument.is_empty() {
        DEFAULT_LIST_ITEMS
    } else {
        argument
            .parse()
            .ok()
            .filter(|count| (1..=MAX_LIST_ITEMS).contains(count))?
    };
    let items: Vec<String> = (0..count).map(marker).collect();
    let cursor = items[0].len();
    Some(Template {
        label: String::new(),
        detail: format!("{detail}, {count} items"),
        text: items.join("\n"),
        selection: cursor..cursor,
    })
}

/// A fenced code block. Without a language the cursor waits after the
/// opening fence for one; with a language it goes inside the block.
fn code(language: &str) -> Option<Template> {
    if language.contains(char::is_whitespace) || language.contains('`') {
        return None;
    }
    let text = format!("```{language}\n\n```");
    let cursor = if language.is_empty() {
        3
    } else {
        4 + language.len()
    };
    Some(Template {
        label: String::new(),
        detail: "Code block".into(),
        text,
        selection: cursor..cursor,
    })
}

/// A table of `columns` x `rows` body rows under a header row, `3x3` by
/// default, with the first header cell selected.
fn table(argument: &str) -> Option<Template> {
    let (columns, rows) = if argument.is_empty() {
        DEFAULT_TABLE
    } else {
        let (columns, rows) = argument.split_once(['x', 'X'])?;
        (columns.parse().ok()?, rows.parse().ok()?)
    };
    if !(1..=MAX_TABLE_COLUMNS).contains(&columns) || !(1..=MAX_TABLE_ROWS).contains(&rows) {
        return None;
    }
    let row = |cell: &dyn Fn(usize) -> String| {
        let cells: Vec<String> = (0..columns).map(cell).collect();
        format!("| {} |", cells.join(" | "))
    };
    let mut lines = vec![
        row(&|ix| format!("Column {}", ix + 1)),
        row(&|_| "---".into()),
    ];
    lines.extend((0..rows).map(|_| row(&|_| " ".into())));
    let mut template = Template::selecting("", &lines.join("\n"), "Column 1");
    template.detail = format!("Table, {columns} columns x {rows} rows");
    Some(template)
}

/// The templates matching `query`, the text typed after `/`. The command
/// name matches by prefix; an argument, after a space, needs the full name.
fn matching(query: &str) -> Vec<Template> {
    let (name, argument) = match query.split_once(' ') {
        Some((name, argument)) => (name, Some(argument.trim())),
        None => (query, None),
    };
    Command::ALL
        .into_iter()
        .filter(|command| match argument {
            Some(_) => command.name() == name,
            None => command.name().starts_with(name),
        })
        .filter_map(|command| {
            let argument = argument.unwrap_or("");
            let mut template = command.template(argument)?;
            template.label = if argument.is_empty() {
                format!("/{}", command.name())
            } else {
                format!("/{} {argument}", command.name())
            };
            Some(template)
        })
        .collect()
}

/// A slash command being typed: where its `/` is, and the indentation before
/// it, which continues on every line of the inserted template.
struct Invocation<'a> {
    start: usize,
    indent: &'a str,
    query: &'a str,
}

/// The slash command `line_before_cursor` ends in, if any. Only a `/` that
/// follows nothing but indentation counts, so paths and URLs in prose do not
/// open the menu.
fn invocation(line_before_cursor: &str, line_start: usize) -> Option<Invocation<'_>> {
    let trimmed = line_before_cursor.trim_start();
    let indent = &line_before_cursor[..line_before_cursor.len() - trimmed.len()];
    let query = trimmed.strip_prefix('/')?;
    // The menu filters by name, then takes one argument: `/table 4x5`.
    if query.starts_with(' ') || query.matches(' ').count() > 1 || query.len() > 32 {
        return None;
    }
    Some(Invocation {
        start: line_start + indent.len(),
        indent,
        query,
    })
}

/// Whether line `row` of `text` is inside a fenced code block, where `/` is
/// code rather than a command.
fn in_code_block(text: &Rope, row: usize) -> bool {
    let mut fence: Option<(char, usize)> = None;
    for line in text.iter_lines().take(row) {
        let mut chars = line
            .chars()
            .skip_while(|ch| *ch == ' ' || *ch == '\t')
            .peekable();
        let marker = match chars.peek() {
            Some(&ch @ ('`' | '~')) => ch,
            _ => continue,
        };
        let mut len = 0;
        while chars.next_if_eq(&marker).is_some() {
            len += 1;
        }
        if len < 3 {
            continue;
        }
        match fence {
            None => fence = Some((marker, len)),
            Some((open, open_len)) if open == marker && len >= open_len => {
                if chars.all(char::is_whitespace) {
                    fence = None;
                }
            }
            Some(_) => {}
        }
    }
    fence.is_some()
}

/// Templates offered at `start`, to place the cursor once one is inserted.
struct Offer {
    start: usize,
    templates: Vec<(String, Range<usize>)>,
}

/// The completion provider behind the slash menu.
#[derive(Default)]
pub(crate) struct SlashCommands {
    /// Whether the cursor was last in a slash command, so each further
    /// keystroke refines the menu instead of only `/` opening it.
    active: Cell<bool>,
    offer: RefCell<Option<Offer>>,
}

impl SlashCommands {
    /// The range to select after an edit, when the edit inserted one of the
    /// offered templates and left the cursor at its end.
    pub(crate) fn take_selection(&self, text: &Rope, cursor: usize) -> Option<Range<usize>> {
        let mut offer = self.offer.borrow_mut();
        let start = offer.as_ref()?.start;
        let inserted = text.slice(start.min(cursor)..cursor).to_string();
        let selection = offer
            .as_ref()?
            .templates
            .iter()
            .find(|(text, _)| *text == inserted)
            .map(|(_, selection)| start + selection.start..start + selection.end)?;
        *offer = None;
        Some(selection)
    }

    /// The `/` offset, indentation and matching templates of the slash
    /// command ending at `offset`.
    fn templates_at(&self, text: &Rope, offset: usize) -> Option<(usize, String, Vec<Template>)> {
        let point = text.offset_to_point(offset);
        let line_start = text.line_start_offset(point.row);
        let line = text.slice(line_start..offset).to_string();
        let invocation = invocation(&line, line_start)?;
        if in_code_block(text, point.row) {
            return None;
        }
        Some((
            invocation.start,
            invocation.indent.to_owned(),
            matching(invocation.query),
        ))
    }
}

impl CompletionProvider for SlashCommands {
    fn completions(
        &self,
        text: &Rope,
        offset: usize,
        _: CompletionContext,
        _: &mut Window,
        _: &mut App,
    ) -> Task<Result<CompletionResponse>> {
        let Some((start, indent, templates)) = self.templates_at(text, offset) else {
            self.active.set(false);
            *self.offer.borrow_mut() = None;
            return Task::ready(Ok(CompletionResponse::Array(Vec::new())));
        };
        self.active.set(true);

        let range = lsp_types::Range::new(
            text.offset_to_position(start),
            text.offset_to_position(offset),
        );
        let mut offered = Vec::with_capacity(templates.len());
        let items = templates
            .into_iter()
            .map(|template| {
                let text = template.text.replace('\n', &format!("\n{indent}"));
                let selection = indented(&template.text, template.selection, &indent);
                offered.push((text.clone(), selection));
                CompletionItem {
                    label: template.label,
                    detail: Some(template.detail),
                    kind: Some(CompletionItemKind::SNIPPET),
                    text_edit: Some(CompletionTextEdit::Edit(TextEdit {
                        range,
                        new_text: text,
                    })),
                    ..Default::default()
                }
            })
            .collect();
        *self.offer.borrow_mut() = Some(Offer {
            start,
            templates: offered,
        });
        Task::ready(Ok(CompletionResponse::Array(items)))
    }

    fn is_completion_trigger(&self, _: usize, new_text: &str, _: &mut App) -> bool {
        new_text == "/"
            || (self.active.get() && new_text.chars().count() == 1 && !new_text.contains('\n'))
    }
}

/// `selection` in `text`, moved past the indentation added to each line
/// before it.
fn indented(text: &str, selection: Range<usize>, indent: &str) -> Range<usize> {
    let shift = text[..selection.start].matches('\n').count() * indent.len();
    selection.start + shift..selection.end + shift
}

#[cfg(test)]
mod tests {
    use super::*;

    fn labels(query: &str) -> Vec<String> {
        matching(query).into_iter().map(|t| t.label).collect()
    }

    #[test]
    fn names_match_by_prefix() {
        assert_eq!(labels("t"), ["/todo", "/table"]);
        assert_eq!(labels("").len(), Command::ALL.len());
        assert!(labels("x").is_empty());
    }

    #[test]
    fn arguments_need_the_full_name() {
        assert_eq!(labels("table 4x5"), ["/table 4x5"]);
        assert!(labels("tab 4x5").is_empty());
        assert!(labels("table 4x").is_empty());
        assert!(labels("table 0x2").is_empty());
        assert!(labels("link x").is_empty());
    }

    #[test]
    fn lists_insert_several_items() {
        let template = Command::Numbered.template("").unwrap();
        assert_eq!(template.text, "1. \n2. \n3. ");
        assert_eq!(template.selection, 3..3);
        assert_eq!(Command::Todo.template("2").unwrap().text, "- [ ] \n- [ ] ");
        assert!(Command::Bullet.template("0").is_none());
    }

    #[test]
    fn table_selects_the_first_header() {
        let template = Command::Table.template("2x1").unwrap();
        assert_eq!(
            template.text,
            "| Column 1 | Column 2 |\n| --- | --- |\n|   |   |"
        );
        assert_eq!(&template.text[template.selection], "Column 1");
    }

    #[test]
    fn code_cursor_waits_for_language_or_content() {
        assert_eq!(Command::Code.template("").unwrap().selection, 3..3);
        let template = Command::Code.template("rust").unwrap();
        assert_eq!(template.text, "```rust\n\n```");
        assert_eq!(template.selection, 8..8);
    }

    #[test]
    fn only_a_slash_after_indentation_invokes() {
        assert_eq!(
            invocation("  /tab", 10).map(|i| (i.start, i.indent, i.query)),
            Some((12, "  ", "tab"))
        );
        assert!(invocation("see a/b", 0).is_none());
        assert!(invocation("/ table", 0).is_none());
    }

    #[test]
    fn fenced_code_blocks_are_skipped() {
        let text = Rope::from("```\n/tab\n```\n/tab");
        assert!(in_code_block(&text, 1));
        assert!(!in_code_block(&text, 3));
    }

    #[test]
    fn indentation_continues_on_each_line() {
        assert_eq!(indented("- \n- ", 5..5, "  "), 7..7);
        assert_eq!(indented("- \n- ", 2..2, "  "), 2..2);
    }
}
