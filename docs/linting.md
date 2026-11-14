# Linting

Neon checks notes against a set of Markdown rules. It can mark issues in the editor, fix them on request (**Edit › Fix Lint Issues**, <kbd>⌥</kbd> <kbd>⇧</kbd> <kbd>F</kbd>), and fix spacing when you save.

Marking issues and fixing on save are both off until you turn them on, in **Settings › Editor › Linting** or for one folder in its `.neonlint.toml`. Fixing on request always works.

## What fixes never do

- Change code blocks, inline code, math, HTML, front matter or link targets.
- Change how the note reads. Each fix is checked by parsing the note again: if its blocks, lists, emphasis, code or links would come out different, that rule's fixes are left out and the notification names the rule. For example, changing `*` bullets to `-` would join a `*` list to the `-` list above it, so it is not done.
- Take more than one undo. The whole fix is one change.

## Configuration

Settings come in layers. A later layer changes only what it names:

1. Neon's defaults: the spacing rules marked **on** below.
2. `lint.toml` in Neon's settings folder (`~/Library/Application Support/Neon`), for every note.
3. `.neonlint.toml` in each folder from the open folder down to the note's folder. With `root = true`, folders above it are not read.
4. `[[overrides]]` in those files whose `files` patterns match the note.
5. Comments in the note itself.

```toml
# Notes here mark issues and fix spacing on save, whatever Settings say.
show-issues = true
fix-on-save = true

# Not linted at all. Patterns work as in .gitignore: a name alone matches
# at any depth, a path is relative to this file's folder.
ignore = ["Templates", "*.excalidraw.md"]

[rules]
consecutive-blank-lines = { max = 2 }      # options alone turn a rule on
cjk-spacing = true
proper-ellipsis = { level = "info", fix-on-save = true }
heading-increment = "off"                   # off, warn, info, true or false
MD009 = false                               # markdownlint names work too

[[overrides]]
files = ["Journal/**"]
rules = { single-h1 = true }

[[overrides]]
files = ["Drafts/**"]
ignore = true

# Your own rules: a regular expression, and optionally what to replace
# each match with. They never match inside code, math or link targets.
[[custom]]
name = "full-width-colon"
pattern = '(\p{Han}):'
replacement = '$1：'
message = "Use a full-width colon after Chinese"

[[custom]]
name = "todo"
pattern = 'TODO'
message = "Unfinished"                      # no replacement: marked only
level = "info"
```

A mistake in a file, such as an unknown rule or a bad pattern, is reported when you fix the note; the rest of the file still applies.

### Comments

```markdown
<!-- neonlint-disable trailing-spaces proper-ellipsis -->
…
<!-- neonlint-enable trailing-spaces proper-ellipsis -->

<!-- neonlint-disable-next-line -->
<!-- neonlint-disable-line heading-increment -->
<!-- neonlint-disable-file cjk-spacing -->
```

A comment without rule names covers every rule. `markdownlint-disable` and the others work too.

## Rules

**On** rules run by default. **Save** rules are fixed on save when fixing on save is on; others wait for **Fix Lint Issues** unless you set `fix-on-save = true` for them.

| Rule | Also | On | Save | What it checks |
| --- | --- | --- | --- | --- |
| `trailing-spaces` | MD009 | ✓ | ✓ | Spaces at the end of a line. Two spaces that make a line break are kept; more become two. |
| `consecutive-blank-lines` | MD012 | ✓ | ✓ | More than `max` (1) blank lines in a row, and blank lines at the start. |
| `final-newline` | MD047 | ✓ | ✓ | The file ends with exactly one line ending. |
| `blank-lines-around-headings` | MD022 | ✓ | ✓ | A blank line before and after a heading. Front matter may sit right above one. |
| `blank-lines-around-fences` | MD031 | ✓ | ✓ | A blank line before and after a fenced code block. |
| `blank-lines-around-tables` | MD058 | ✓ | ✓ | A blank line before and after a table. |
| `blank-lines-around-blockquotes` | | | ✓ | A blank line before and after a quote or callout. |
| `blank-lines-around-rules` | | | ✓ | A blank line before and after a horizontal rule. |
| `heading-trailing-hashes` | MD020 | ✓ | ✓ | No closing `#`s, as in `## Title ##`. |
| `heading-increment` | MD001 | ✓ | | Heading levels go down one at a time. Marked only. |
| `single-h1` | MD025 | | | At most one top-level heading. Marked only. |
| `heading-punctuation` | MD026 | | | No punctuation at the end of a heading; `punctuation` sets which (`.,;:!。，；：！`). |
| `multiple-spaces` | | | ✓ | One space between words. |
| `unordered-list-marker` | MD004 | | ✓ | One bullet: `style` is `consistent`, `dash`, `asterisk` or `plus`. |
| `ordered-list-numbering` | MD029 | | ✓ | Numbers count up (`style = "ascending"`) or repeat the first (`"lazy"`). |
| `emphasis-style` | MD049 | | ✓ | One italic marker: `consistent`, `asterisk` or `underscore`. |
| `strong-style` | MD050 | | ✓ | One bold marker: `consistent`, `asterisk` or `underscore`. |
| `code-fence-style` | MD048 | | ✓ | One fence: `consistent`, `backtick` or `tilde`. |
| `fence-language` | MD040 | | | Each fenced block names a language; the fix adds `language` (`text`). |
| `cjk-spacing` | | | ✓ | A space between Chinese, Japanese or Korean and Latin letters or digits. |
| `proper-ellipsis` | | | | `...` becomes `…`. |
| `table-alignment` | | | ✓ | Table columns padded to line up; wide characters count double. |
