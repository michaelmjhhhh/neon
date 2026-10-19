//! Slash commands: typing `/` on an empty line opens a menu of Markdown
//! templates, such as a list or a table, and accepting one inserts it in place
//! of the command. Tab instead completes the command's name, so an argument
//! can be typed before the template is inserted.

use std::cell::{Cell, RefCell};
use std::ops::Range;

use gpui_kit::component::input::{CompletionProvider, Rope, RopeExt as _};
use gpui_kit::{App, Result, Task, Window};
use lsp_types::{
    CompletionContext, CompletionItem, CompletionItemKind, CompletionResponse, CompletionTextEdit,
    Documentation, MarkupContent, MarkupKind, TextEdit,
};

const DEFAULT_LIST_ITEMS: usize = 3;
const MAX_LIST_ITEMS: usize = 50;
const DEFAULT_TABLE_SIZE: usize = 3;
const MAX_TABLE_COLUMNS: usize = 20;
const MAX_TABLE_ROWS: usize = 100;
/// Lines of a template shown in the menu's preview before it is cut short.
const PREVIEW_LINES: usize = 8;

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

    /// How to give this command an argument, shown until one is typed.
    fn hint(self) -> Option<&'static str> {
        match self {
            Command::Bullet => Some("Add a count: `/bullet 5`"),
            Command::Numbered => Some("Add a count: `/numbered 5`"),
            Command::Todo => Some("Add a count: `/todo 5`"),
            Command::Code => Some("Add a language: `/code rust`"),
            Command::Table => Some("Add a size: `/table 4x5`, columns x rows"),
            Command::Link | Command::Image => None,
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
            Command::Image if argument.is_empty() => Some(Template {
                // An image right under text joins its paragraph and is not
                // shown as an image of its own.
                blank_line_before: true,
                ..Template::selecting("Image", "![alt text](path)", "alt text")
            }),
            Command::Link | Command::Image => None,
            Command::Table => table(argument),
        }
    }
}

/// Text to insert, and the range within it to select afterwards.
#[derive(Debug, Clone, PartialEq)]
struct Template {
    /// The argument as it will be applied, completed from what was typed:
    /// `/table 4` is listed as `/table 4x3`.
    argument: Option<String>,
    detail: String,
    text: String,
    selection: Range<usize>,
    /// Whether the template needs a blank line above it to stand alone.
    blank_line_before: bool,
}

impl Template {
    fn new(detail: String, text: String, selection: Range<usize>) -> Self {
        Self {
            argument: None,
            detail,
            text,
            selection,
            blank_line_before: false,
        }
    }

    fn selecting(detail: &str, text: &str, selected: &str) -> Self {
        let start = text.find(selected).unwrap_or(0);
        Self::new(detail.into(), text.into(), start..start + selected.len())
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
    let mut template = Template::new(
        format!("{detail}, {count} items"),
        items.join("\n"),
        cursor..cursor,
    );
    template.argument = (!argument.is_empty()).then(|| count.to_string());
    Some(template)
}

/// A fenced code block. Without a language the cursor waits after the
/// opening fence for one; with a language it goes inside the block.
fn code(language: &str) -> Option<Template> {
    if language.contains(char::is_whitespace) || language.contains('`') {
        return None;
    }
    let cursor = if language.is_empty() {
        3
    } else {
        4 + language.len()
    };
    let detail = if language.is_empty() {
        "Code block".into()
    } else {
        format!("Code block, {language}")
    };
    let mut template = Template::new(detail, format!("```{language}\n\n```"), cursor..cursor);
    template.argument = (!language.is_empty()).then(|| language.into());
    Some(template)
}

/// A table of `columns` x `rows` body rows under a header row, with the
/// first header cell selected. Either size may be left out while typing:
/// `4`, `4x` and `4x3` are all four columns and three rows.
fn table(argument: &str) -> Option<Template> {
    let size = |digits: &str| -> Option<usize> {
        if digits.is_empty() {
            Some(DEFAULT_TABLE_SIZE)
        } else {
            digits.parse().ok()
        }
    };
    let (columns, rows) = match argument.split_once(['x', 'X']) {
        Some((columns, rows)) => (columns, rows),
        None => (argument, ""),
    };
    if argument.starts_with(['x', 'X']) {
        return None;
    }
    let (columns, rows) = (size(columns)?, size(rows)?);
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
    template.argument = (!argument.is_empty()).then(|| format!("{columns}x{rows}"));
    Some(template)
}

/// A template on offer in the menu, with the label it is listed under.
struct Match {
    command: Command,
    label: String,
    template: Template,
}

/// The templates matching `query`, the text typed after `/`. The command
/// name matches by prefix; an argument, after a space, needs the full name.
fn matching(query: &str) -> Vec<Match> {
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
            let template = command.template(argument.unwrap_or(""))?;
            let label = match &template.argument {
                Some(argument) => format!("/{} {argument}", command.name()),
                None => format!("/{}", command.name()),
            };
            Some(Match {
                command,
                label,
                template,
            })
        })
        .collect()
}

/// The command written out for `template`, as Tab completes it. Without an
/// argument a space follows the name, ready for one; an argument typed in
/// part is filled in, so `/table 4` becomes `/table 4x3`.
fn command_line(command: Command, template: &Template) -> String {
    match &template.argument {
        Some(argument) => format!("/{} {argument}", command.name()),
        None => format!("/{} ", command.name()),
    }
}

/// The menu's preview of a template: its Markdown source, cut short when
/// long, and how to give the command an argument.
fn preview(command: Command, template: &Template) -> Documentation {
    let mut lines: Vec<&str> = template.text.lines().take(PREVIEW_LINES + 1).collect();
    if lines.len() > PREVIEW_LINES {
        lines.truncate(PREVIEW_LINES);
        lines.push("…");
    }
    // A fence longer than any in the template, so a code block shows whole.
    let mut value = format!("````markdown\n{}\n````", lines.join("\n"));
    if template.argument.is_none()
        && let Some(hint) = command.hint()
    {
        value.push_str("\n\n");
        value.push_str(hint);
    }
    Documentation::MarkupContent(MarkupContent {
        kind: MarkupKind::Markdown,
        value,
    })
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
pub(crate) fn in_code_block(text: &Rope, row: usize) -> bool {
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

/// A template inserted at `start` as `text`, to place the cursor afterwards.
struct Offered {
    start: usize,
    text: String,
    selection: Range<usize>,
    /// What replaces `text` when the template was accepted with Tab: the
    /// command written out, such as `/table `, ready for an argument.
    command: String,
}

/// What to do after an edit inserted one of the offered templates.
#[derive(Debug, PartialEq)]
pub(crate) enum Insertion {
    /// Accepted with Enter: select this range of the template.
    Select(Range<usize>),
    /// Accepted with Tab: replace the template at `range` with `command` and
    /// open the menu again.
    Complete {
        range: Range<usize>,
        command: String,
    },
}

/// The completion provider behind the slash menu.
#[derive(Default)]
pub(crate) struct SlashCommands {
    /// Whether the cursor was last in a slash command, so each further
    /// keystroke refines the menu instead of only `/` opening it.
    active: Cell<bool>,
    /// The cursor offset the menu was last asked about.
    requested_at: Cell<Option<usize>>,
    /// The `/` offset of a command whose menu was closed with Escape, which
    /// stays closed until a `/` is typed again.
    dismissed: Cell<Option<usize>>,
    offered: RefCell<Vec<Offered>>,
    /// Whether the next accepted template completes its command instead.
    completing: Cell<bool>,
}

impl SlashCommands {
    /// What to do after an edit, when the edit inserted one of the offered
    /// templates and left the cursor at its end.
    pub(crate) fn take_insertion(&self, text: &Rope, cursor: usize) -> Option<Insertion> {
        let mut offered = self.offered.borrow_mut();
        let insertion = offered.iter().find_map(|offered| {
            let end = offered.start + offered.text.len();
            (end == cursor && text.slice(offered.start..end) == offered.text.as_str()).then(|| {
                if self.completing.get() {
                    Insertion::Complete {
                        range: offered.start..end,
                        command: offered.command.clone(),
                    }
                } else {
                    Insertion::Select(
                        offered.start + offered.selection.start
                            ..offered.start + offered.selection.end,
                    )
                }
            })
        })?;
        offered.clear();
        self.active.set(false);
        self.completing.set(false);
        Some(insertion)
    }

    /// Make the template accepted next complete its command instead, when
    /// the menu has templates on offer. Returns whether it does.
    pub(crate) fn complete_next(&self) -> bool {
        let offered = !self.offered.borrow().is_empty();
        self.completing.set(offered);
        offered
    }

    /// Whether an edit the menu did not see, such as a deletion, left the
    /// cursor in a slash command, so the menu should be asked again.
    pub(crate) fn needs_refresh(&self, cursor: usize) -> bool {
        self.active.get() && self.requested_at.get() != Some(cursor)
    }

    /// Keep the menu closed after Escape until a new `/` is typed.
    pub(crate) fn dismiss(&self) {
        let mut offered = self.offered.borrow_mut();
        if let Some(offer) = offered.first() {
            self.dismissed.set(Some(offer.start));
            offered.clear();
            self.active.set(false);
        }
        self.completing.set(false);
    }

    /// The menu items for the slash command ending at `offset`, or `None`
    /// when the cursor is not in one.
    fn items_at(&self, text: &Rope, offset: usize) -> Option<Vec<CompletionItem>> {
        let point = text.offset_to_point(offset);
        let line_start = text.line_start_offset(point.row);
        let line = text.slice(line_start..offset).to_string();
        let invocation = invocation(&line, line_start)?;
        let line_end = text.line_end_offset(point.row);
        if self.dismissed.get() == Some(invocation.start)
            || text
                .slice(offset..line_end)
                .chars()
                .any(|ch| !ch.is_whitespace())
            || in_code_block(text, point.row)
        {
            return None;
        }
        let after_text = point.row > 0
            && text
                .slice_line(point.row - 1)
                .chars()
                .any(|ch| !ch.is_whitespace());
        let typed = text.slice(invocation.start..offset).to_string();
        let end = text.offset_to_position(offset);

        let mut offered = Vec::new();
        let items = matching(invocation.query)
            .into_iter()
            .map(
                |Match {
                     command,
                     label,
                     template,
                 }| {
                    let documentation = preview(command, &template);
                    let (start, prefix) = if template.blank_line_before && after_text {
                        (line_start, format!("\n{}", invocation.indent))
                    } else {
                        (invocation.start, String::new())
                    };
                    let indent = format!("\n{}", invocation.indent);
                    let new_text = format!("{prefix}{}", template.text.replace('\n', &indent));
                    // The text from `start` up to the `/` stays as it was.
                    let kept = text.slice(start..invocation.start).to_string();
                    let command = format!("{kept}{}", command_line(command, &template));
                    let selection = indented(&template.text, template.selection, invocation.indent);
                    offered.push(Offered {
                        start,
                        text: new_text.clone(),
                        selection: selection.start + prefix.len()..selection.end + prefix.len(),
                        command,
                    });
                    CompletionItem {
                        label,
                        detail: Some(template.detail),
                        kind: Some(CompletionItemKind::SNIPPET),
                        documentation: Some(documentation),
                        // Highlights what was typed, the `/` included.
                        filter_text: Some(typed.clone()),
                        text_edit: Some(CompletionTextEdit::Edit(TextEdit {
                            range: lsp_types::Range::new(text.offset_to_position(start), end),
                            new_text,
                        })),
                        ..Default::default()
                    }
                },
            )
            .collect();
        *self.offered.borrow_mut() = offered;
        Some(items)
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
        self.requested_at.set(Some(offset));
        self.completing.set(false);
        let items = self.items_at(text, offset);
        self.active.set(items.is_some());
        if items.is_none() {
            self.offered.borrow_mut().clear();
        }
        Task::ready(Ok(CompletionResponse::Array(items.unwrap_or_default())))
    }

    fn is_completion_trigger(&self, _: usize, new_text: &str, _: &mut App) -> bool {
        if new_text == "/" {
            self.dismissed.set(None);
            return true;
        }
        self.active.get() && new_text.chars().count() == 1 && !new_text.contains('\n')
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
        matching(query).into_iter().map(|m| m.label).collect()
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
        assert!(labels("table 0x2").is_empty());
        assert!(labels("table x2").is_empty());
        assert!(labels("link x").is_empty());
    }

    #[test]
    fn partial_sizes_complete_with_defaults() {
        assert_eq!(labels("table 4"), ["/table 4x3"]);
        assert_eq!(labels("table 4x"), ["/table 4x3"]);
        assert_eq!(labels("table "), ["/table"]);
        assert_eq!(labels("bullet 5"), ["/bullet 5"]);
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
    fn preview_cuts_long_templates_and_hints_at_arguments() {
        let Documentation::MarkupContent(content) =
            preview(Command::Table, &Command::Table.template("").unwrap())
        else {
            unreachable!()
        };
        assert!(content.value.ends_with("`/table 4x5`, columns x rows"));
        let Documentation::MarkupContent(content) =
            preview(Command::Bullet, &Command::Bullet.template("20").unwrap())
        else {
            unreachable!()
        };
        assert_eq!(content.value.matches("- ").count(), PREVIEW_LINES);
        assert!(content.value.contains('…') && !content.value.contains("Add a count"));
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
    fn tab_writes_out_the_command() {
        let line = |query: &str| {
            matching(query)
                .into_iter()
                .map(|m| command_line(m.command, &m.template))
                .collect::<Vec<_>>()
        };
        assert_eq!(line("ta"), ["/table "]);
        assert_eq!(line("table 4"), ["/table 4x3"]);
        assert_eq!(line("link "), ["/link "]);
    }

    #[test]
    fn tab_replaces_the_template_with_its_command() {
        let commands = SlashCommands::default();
        let typed = Rope::from("  /ta");
        let items = commands.items_at(&typed, typed.len()).unwrap();
        let Some(CompletionTextEdit::Edit(edit)) = &items[0].text_edit else {
            unreachable!()
        };
        let text = Rope::from(format!("  {}", edit.new_text).as_str());
        assert!(commands.complete_next());
        assert_eq!(
            commands.take_insertion(&text, text.len()),
            Some(Insertion::Complete {
                range: 2..text.len(),
                command: "/table ".into(),
            })
        );
    }

    #[test]
    fn indentation_continues_on_each_line() {
        assert_eq!(indented("- \n- ", 5..5, "  "), 7..7);
        assert_eq!(indented("- \n- ", 2..2, "  "), 2..2);
    }
}
