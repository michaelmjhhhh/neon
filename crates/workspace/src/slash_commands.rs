//! Slash commands: typing `/` on an empty line opens a menu of Markdown
//! templates, such as a list or a table, and accepting one inserts it in place
//! of the command. Tab instead completes the command's name, so an argument
//! can be typed before the template is inserted.
//!
//! This module holds the menu's state; [`crate::slash_menu`] draws it.

use std::ops::Range;

use gpui_kit::EntityId;
use gpui_kit::component::input::{Rope, RopeExt as _};

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
/// long.
fn preview(template: &Template) -> String {
    let mut lines: Vec<&str> = template.text.lines().take(PREVIEW_LINES + 1).collect();
    if lines.len() > PREVIEW_LINES {
        lines.truncate(PREVIEW_LINES);
        lines.push("…");
    }
    lines.join("\n")
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

/// A template on offer in the menu.
#[derive(Debug, Clone)]
pub(crate) struct MenuItem {
    pub(crate) label: String,
    /// The length of the label's start that matches what was typed.
    pub(crate) matched: usize,
    pub(crate) detail: String,
    /// The template's Markdown source, cut short when long.
    pub(crate) preview: String,
    /// How to give the command an argument, until one is typed. Text in
    /// backticks is an example to type.
    pub(crate) hint: Option<&'static str>,
    /// The text accepting the item replaces: the command typed, and the line
    /// break before it when the template needs a blank line above.
    range: Range<usize>,
    /// The template, indented like the command.
    text: String,
    /// The range of the inserted template to select.
    selection: Range<usize>,
    /// What replaces `range` when the item is accepted with Tab: the command
    /// written out, such as `/table `, ready for an argument.
    command: String,
}

/// The edit accepting a menu item makes.
#[derive(Debug, PartialEq)]
pub(crate) struct Edit {
    pub(crate) range: Range<usize>,
    pub(crate) text: String,
    /// The range to select afterwards; `None` leaves the cursor after `text`.
    pub(crate) selection: Option<Range<usize>>,
}

/// The menu open over an editor.
struct OpenMenu {
    editor: EntityId,
    /// Where the command's `/` is.
    start: usize,
    /// Empty when nothing matches; the menu stays open, hidden, so further
    /// typing or a deletion can bring matches back.
    items: Vec<MenuItem>,
    selected: usize,
}

/// The slash menu's state: the templates on offer for the command being
/// typed, and which one is highlighted.
#[derive(Default)]
pub(crate) struct SlashMenu {
    open: Option<OpenMenu>,
}

impl SlashMenu {
    /// Follow an edit or a cursor move in `editor`. An edit that leaves a
    /// lone `/` before the cursor opens the menu; while it is open, each
    /// edit or move refines it, and leaving the command closes it. Returns
    /// whether the menu changed.
    pub(crate) fn update(
        &mut self,
        editor: EntityId,
        text: &Rope,
        cursor: usize,
        edited: bool,
    ) -> bool {
        let was_open = self.open.is_some();
        let open_here = self.open.as_ref().is_some_and(|menu| menu.editor == editor);
        // Closed, only a `/` just typed can open it, so most keystrokes and
        // cursor moves skip reading the line. `/` is ASCII, so its byte is a
        // whole character.
        let slash_before = cursor > 0 && text.get_byte(cursor - 1) == Some(b'/');
        if !open_here && !(edited && slash_before) {
            return self.open.take().is_some();
        }
        self.open = match items_at(text, cursor) {
            Some((start, fresh, items)) if open_here || (edited && fresh) => Some(OpenMenu {
                editor,
                start,
                items,
                selected: 0,
            }),
            _ => None,
        };
        was_open || self.open.is_some()
    }

    /// Close the menu. Returns whether it was open.
    pub(crate) fn close(&mut self) -> bool {
        self.open.take().is_some()
    }

    /// The menu shown over `editor`: where the command's `/` is, the items,
    /// and which one is highlighted. `None` when it is closed or has nothing
    /// to offer.
    pub(crate) fn shown(&self, editor: EntityId) -> Option<(usize, &[MenuItem], usize)> {
        self.open
            .as_ref()
            .filter(|menu| menu.editor == editor && !menu.items.is_empty())
            .map(|menu| (menu.start, menu.items.as_slice(), menu.selected))
    }

    /// Highlight the item `delta` rows away, wrapping around the ends.
    pub(crate) fn move_selection(&mut self, delta: isize) {
        if let Some(menu) = self.open.as_mut().filter(|menu| !menu.items.is_empty()) {
            let len = menu.items.len() as isize;
            menu.selected = (menu.selected as isize + delta).rem_euclid(len) as usize;
        }
    }

    pub(crate) fn select(&mut self, ix: usize) {
        if let Some(menu) = self.open.as_mut().filter(|menu| ix < menu.items.len()) {
            menu.selected = ix;
        }
    }

    /// The edit accepting the highlighted item makes. Enter inserts its
    /// template and closes the menu; Tab, with `complete`, writes out its
    /// command and keeps the menu open for an argument.
    pub(crate) fn accept(&mut self, complete: bool) -> Option<Edit> {
        let menu = self.open.as_ref()?;
        let item = menu.items.get(menu.selected)?;
        let edit = if complete {
            Edit {
                range: item.range.clone(),
                text: item.command.clone(),
                selection: None,
            }
        } else {
            let start = item.range.start;
            Edit {
                range: item.range.clone(),
                text: item.text.clone(),
                selection: Some(start + item.selection.start..start + item.selection.end),
            }
        };
        if !complete {
            self.open = None;
        }
        Some(edit)
    }
}

/// The menu items for the slash command ending at `cursor`, with where its
/// `/` is and whether nothing follows the `/` yet, or `None` when the cursor
/// is not in a slash command.
fn items_at(text: &Rope, cursor: usize) -> Option<(usize, bool, Vec<MenuItem>)> {
    let point = text.offset_to_point(cursor);
    let line_start = text.line_start_offset(point.row);
    let line = text.slice(line_start..cursor).to_string();
    let invocation = invocation(&line, line_start)?;
    let line_end = text.line_end_offset(point.row);
    if text
        .slice(cursor..line_end)
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
    let typed = text.slice(invocation.start..cursor).to_string();

    let items = matching(invocation.query)
        .into_iter()
        .map(
            |Match {
                 command,
                 label,
                 template,
             }| {
                let (start, prefix) = if template.blank_line_before && after_text {
                    (line_start, format!("\n{}", invocation.indent))
                } else {
                    (invocation.start, String::new())
                };
                let indent = format!("\n{}", invocation.indent);
                let new_text = format!("{prefix}{}", template.text.replace('\n', &indent));
                // The text from `start` up to the `/` stays as it was.
                let kept = text.slice(start..invocation.start).to_string();
                let selection = indented(
                    &template.text,
                    template.selection.clone(),
                    invocation.indent,
                );
                let matched = label
                    .char_indices()
                    .zip(typed.chars())
                    .take_while(|((_, a), b)| a == b)
                    .last()
                    .map_or(0, |((ix, ch), _)| ix + ch.len_utf8());
                MenuItem {
                    matched,
                    detail: template.detail.clone(),
                    preview: preview(&template),
                    hint: template
                        .argument
                        .is_none()
                        .then(|| command.hint())
                        .flatten(),
                    range: start..cursor,
                    text: new_text,
                    selection: selection.start + prefix.len()..selection.end + prefix.len(),
                    command: format!("{kept}{}", command_line(command, &template)),
                    label,
                }
            },
        )
        .collect();
    Some((invocation.start, invocation.query.is_empty(), items))
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
        let preview = preview(&Command::Bullet.template("20").unwrap());
        assert_eq!(preview.matches("- ").count(), PREVIEW_LINES);
        assert!(preview.ends_with('…'));
        let rope = Rope::from("/table");
        let (_, _, items) = items_at(&rope, rope.len()).unwrap();
        assert_eq!(items[0].hint, Command::Table.hint());
        let rope = Rope::from("/table 4");
        let (_, _, items) = items_at(&rope, rope.len()).unwrap();
        assert_eq!(items[0].hint, None);
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

    fn editor() -> EntityId {
        EntityId::from(1u64)
    }

    /// A menu opened by typing `/` at the end of `text`'s first part, then
    /// refined by typing the rest.
    fn typed(before: &str, after: &str) -> SlashMenu {
        let mut menu = SlashMenu::default();
        let text = Rope::from(before);
        menu.update(editor(), &text, text.len(), true);
        let text = Rope::from(format!("{before}{after}").as_str());
        menu.update(editor(), &text, text.len(), true);
        menu
    }

    #[test]
    fn a_typed_slash_opens_the_menu_and_leaving_closes_it() {
        let mut menu = SlashMenu::default();
        let text = Rope::from("/");
        // A cursor move onto a lone `/` does not open it; typing one does.
        assert!(!menu.update(editor(), &text, 1, false));
        assert!(menu.update(editor(), &text, 1, true));
        assert_eq!(menu.shown(editor()).unwrap().1.len(), Command::ALL.len());

        let text = Rope::from("/x");
        menu.update(editor(), &text, 2, true);
        assert!(menu.shown(editor()).is_none(), "no match hides the menu");
        let text = Rope::from("/t");
        menu.update(editor(), &text, 2, true);
        assert_eq!(
            menu.shown(editor()).unwrap().1.len(),
            2,
            "and it comes back"
        );

        let text = Rope::from("/t b c");
        menu.update(editor(), &text, 6, true);
        assert!(menu.shown(editor()).is_none());
        menu.update(editor(), &Rope::from("/t"), 2, true);
        assert!(menu.shown(editor()).is_none(), "closed until a new `/`");
    }

    #[test]
    fn selection_wraps_around() {
        let mut menu = typed("/", "t");
        menu.move_selection(-1);
        assert_eq!(menu.shown(editor()).unwrap().2, 1);
        menu.move_selection(1);
        assert_eq!(menu.shown(editor()).unwrap().2, 0);
    }

    #[test]
    fn enter_inserts_the_template_and_tab_writes_out_the_command() {
        let mut menu = typed("  /", "li");
        assert_eq!(
            menu.accept(true),
            Some(Edit {
                range: 2..5,
                text: "/link ".into(),
                selection: None,
            })
        );
        assert!(menu.shown(editor()).is_some(), "Tab keeps the menu open");
        assert_eq!(
            menu.accept(false),
            Some(Edit {
                range: 2..5,
                text: "[text](url)".into(),
                selection: Some(3..7),
            })
        );
        assert!(menu.shown(editor()).is_none());
    }

    #[test]
    fn images_after_text_start_a_paragraph() {
        let mut menu = typed("prose\n/", "image");
        assert_eq!(
            menu.accept(false),
            Some(Edit {
                range: 6..12,
                text: "\n![alt text](path)".into(),
                selection: Some(9..17),
            })
        );
    }

    #[test]
    fn indentation_continues_on_each_line() {
        assert_eq!(indented("- \n- ", 5..5, "  "), 7..7);
        assert_eq!(indented("- \n- ", 2..2, "  "), 2..2);
    }
}
