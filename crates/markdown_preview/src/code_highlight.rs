//! Syntax colors for fenced code blocks.
//!
//! Tokens are found with the same tree-sitter grammars and theme colors as
//! the editor, on the parse's background thread, so rendering a code block
//! only looks its tokens up. A block keeps its tokens across parses while its
//! text and language stay the same; typing elsewhere highlights nothing again.

use std::collections::HashMap;
use std::ops::Range;
use std::sync::{Arc, Once};

use gpui_kit::base::input::Rope;
use gpui_kit::component::highlighter::{HighlightTheme, LanguageRegistry, SyntaxHighlighter};
use gpui_kit::{HighlightStyle, SharedString};
use markdown::{BlockKind, CodeBlock, MarkdownDocument};

/// Longer blocks are shown plain: their tokens would cost more to keep and
/// lay out than the colors are worth.
const MAX_HIGHLIGHT_BYTES: usize = 128 * 1024;

/// Names fences use for the grammars Neon builds in, beyond those GPUI Kit
/// already knows (`sh`, `js`, `py`, `rs`, `ts`, `yml` and the like). Each
/// maps to the grammar's own name.
const ALIASES: &[(&str, &str)] = &[
    ("shell", "bash"),
    ("shellscript", "bash"),
    ("zsh", "bash"),
    ("h", "c"),
    ("cc", "cpp"),
    ("cxx", "cpp"),
    ("hpp", "cpp"),
    ("hh", "cpp"),
    ("patch", "diff"),
    ("golang", "go"),
    ("htm", "html"),
    ("xhtml", "html"),
    ("JavaScript", "javascript"),
    ("jsx", "javascript"),
    ("mjs", "javascript"),
    ("cjs", "javascript"),
    ("TypeScript", "typescript"),
    ("mts", "typescript"),
    ("cts", "typescript"),
    ("mk", "make"),
    ("py3", "python"),
    ("python3", "python"),
    ("pyi", "python"),
    ("postgres", "sql"),
    ("postgresql", "sql"),
    ("mysql", "sql"),
    ("sqlite", "sql"),
];

/// Grammar names fences also write capitalized or in capitals, such as
/// `Python` or `JSON`, which the editor matches as written.
const NAMES: &[&str] = &[
    "bash",
    "c",
    "cpp",
    "css",
    "diff",
    "go",
    "html",
    "java",
    "javascript",
    "json",
    "lua",
    "make",
    "python",
    "rust",
    "sql",
    "toml",
    "tsx",
    "typescript",
    "yaml",
];

/// Teach the shared language registry the names in [`ALIASES`] and the
/// capitalized forms of every grammar. The editor's Markdown highlighter
/// resolves fences through the same registry, so both panes agree on which
/// fences get colors. Runs once; later calls return at once.
pub(crate) fn register_languages() {
    static REGISTER: Once = Once::new();
    REGISTER.call_once(|| {
        let registry = LanguageRegistry::singleton();
        let names = ALIASES
            .iter()
            .copied()
            .chain(NAMES.iter().map(|name| (*name, *name)));
        for (alias, target) in names {
            let Some(config) = registry.language(target) else {
                continue;
            };
            for form in [alias.to_string(), alias.to_uppercase(), capitalized(alias)] {
                if registry.language(&form).is_none() {
                    registry.register(&form, &config);
                }
            }
        }
    });
}

fn capitalized(word: &str) -> String {
    let mut chars = word.chars();
    chars
        .next()
        .map(|first| first.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

/// The grammar a fence's language names, such as `rust` for `Rust` or `rs`,
/// or `None` when Neon has no grammar for it.
fn grammar_for(language: &str) -> Option<SharedString> {
    let registry = LanguageRegistry::singleton();
    registry
        .language(language)
        .or_else(|| registry.language(&language.to_lowercase()))
        .filter(|config| config.has_grammar())
        .map(|config| config.name)
}

/// The tokens of a code block: byte ranges of its code and their styles.
pub(crate) type CodeTokens = Arc<[(Range<usize>, HighlightStyle)]>;

/// The colored code blocks of one document, in one theme. Blocks without a
/// grammar, or too long to color, are absent and render plain.
#[derive(Default)]
pub(crate) struct CodeHighlights {
    theme: Option<Arc<HighlightTheme>>,
    blocks: HashMap<CodeBlock, CodeTokens>,
}

impl CodeHighlights {
    /// Highlight every code block in `document`, reusing the tokens in
    /// `previous` for blocks that did not change when the theme is the
    /// same. `None` when `cancelled` says a newer document replaced this one.
    pub(crate) fn compute(
        document: &MarkdownDocument,
        previous: &CodeHighlights,
        theme: Arc<HighlightTheme>,
        cancelled: impl Fn() -> bool,
    ) -> Option<Self> {
        register_languages();
        let reuse = previous.is_for(&theme);
        let mut blocks = HashMap::new();
        let mut highlighters = HashMap::<SharedString, Highlighter>::new();
        let mut pending = document
            .blocks()
            .iter()
            .map(|block| block.kind())
            .collect::<Vec<_>>();
        while let Some(kind) = pending.pop() {
            let code = match kind {
                BlockKind::Code(code) => code,
                BlockKind::Quote(children) | BlockKind::Footnote { children, .. } => {
                    pending.extend(children);
                    continue;
                }
                BlockKind::Callout(callout) => {
                    pending.extend(callout.children());
                    continue;
                }
                BlockKind::List(list) => {
                    pending.extend(list.items().iter().flat_map(|item| item.children()));
                    continue;
                }
                _ => continue,
            };
            if blocks.contains_key(code) {
                continue;
            }
            if reuse && let Some(tokens) = previous.blocks.get(code) {
                blocks.insert(code.clone(), tokens.clone());
                continue;
            }
            if code.code().len() > MAX_HIGHLIGHT_BYTES {
                continue;
            }
            let Some(grammar) = code.language().and_then(|language| grammar_for(language)) else {
                continue;
            };
            if cancelled() {
                return None;
            }
            let tokens = highlighters
                .entry(grammar.clone())
                .or_insert_with(|| Highlighter::new(&grammar))
                .highlight(code.code(), &theme);
            if !tokens.is_empty() {
                blocks.insert(code.clone(), tokens.into());
            }
        }
        Some(Self {
            theme: Some(theme),
            blocks,
        })
    }

    /// Whether these tokens were styled with `theme`.
    pub(crate) fn is_for(&self, theme: &Arc<HighlightTheme>) -> bool {
        self.theme
            .as_ref()
            .is_some_and(|own| Arc::ptr_eq(own, theme) || **own == **theme)
    }

    pub(crate) fn get(&self, code: &CodeBlock) -> Option<&CodeTokens> {
        self.blocks.get(code)
    }
}

/// One grammar's highlighter, reused from block to block so its queries are
/// compiled once per document rather than once per block.
struct Highlighter {
    highlighter: SyntaxHighlighter,
    /// Where the text it last parsed ends.
    len: usize,
    end: tree_sitter::Point,
}

impl Highlighter {
    fn new(grammar: &str) -> Self {
        Self {
            highlighter: SyntaxHighlighter::new(grammar),
            len: 0,
            end: tree_sitter::Point::new(0, 0),
        }
    }

    fn highlight(
        &mut self,
        code: &str,
        theme: &HighlightTheme,
    ) -> Vec<(Range<usize>, HighlightStyle)> {
        // The new block replaces all of the last one's text, so no part of
        // the old tree is reused for unrelated code.
        let end = end_point(code);
        let edit = tree_sitter::InputEdit {
            start_byte: 0,
            old_end_byte: self.len,
            new_end_byte: code.len(),
            start_position: tree_sitter::Point::new(0, 0),
            old_end_position: self.end,
            new_end_position: end,
        };
        self.highlighter.update(Some(edit), &Rope::from(code), None);
        self.len = code.len();
        self.end = end;
        self.highlighter
            .styles(&(0..code.len()), theme)
            .into_iter()
            .filter(|(_, style)| *style != HighlightStyle::default())
            .collect()
    }
}

/// The row and byte column just past the end of `text`.
fn end_point(text: &str) -> tree_sitter::Point {
    let row = text.bytes().filter(|byte| *byte == b'\n').count();
    let column = text.len() - text.rfind('\n').map_or(0, |ix| ix + 1);
    tree_sitter::Point::new(row, column)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn highlights(source: &str) -> CodeHighlights {
        CodeHighlights::compute(
            &MarkdownDocument::parse(source),
            &CodeHighlights::default(),
            HighlightTheme::default_dark(),
            || false,
        )
        .unwrap()
    }

    fn code_blocks(document: &MarkdownDocument) -> Vec<CodeBlock> {
        document
            .blocks()
            .iter()
            .filter_map(|block| match block.kind() {
                BlockKind::Code(code) => Some(code.clone()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn resolves_fence_names_in_any_case_and_by_alias() {
        register_languages();
        for name in [
            "rust",
            "rs",
            "Rust",
            "RUST",
            "py",
            "Python",
            "shell",
            "JSON",
            "c++",
            "TypeScript",
        ] {
            assert!(grammar_for(name).is_some(), "{name} has no grammar");
        }
        assert_eq!(grammar_for("zsh").as_deref(), Some("bash"));
        assert_eq!(grammar_for("text"), None);
        assert_eq!(grammar_for("no-such-language"), None);
    }

    #[test]
    fn colors_known_languages_and_leaves_others_plain() {
        let source = "```rust\nfn main() {}\n```\n\n```\nplain\n```\n\n```klingon\nqapla'\n```\n";
        let document = MarkdownDocument::parse(source);
        let highlights = highlights(source);
        let blocks = code_blocks(&document);
        let tokens = highlights.get(&blocks[0]).expect("rust is colored");
        let code = blocks[0].code();
        assert!(tokens.iter().any(|(range, _)| &code[range.clone()] == "fn"));
        assert!(highlights.get(&blocks[1]).is_none());
        assert!(highlights.get(&blocks[2]).is_none());
    }

    #[test]
    fn colors_code_inside_lists_and_quotes() {
        let source = "- item\n\n  ```python\n  x = 1\n  ```\n\n> ```go\n> package main\n> ```\n";
        assert_eq!(highlights(source).blocks.len(), 2);
    }

    #[test]
    fn a_reused_highlighter_does_not_carry_tokens_between_blocks() {
        let source =
            "```rust\nfn first() { let a = \"long string here\"; }\n```\n\n```rust\nx\n```\n";
        let document = MarkdownDocument::parse(source);
        let highlights = highlights(source);
        let blocks = code_blocks(&document);
        // The second block is shorter than the first; tokens from the first
        // parse must not reach past its end.
        if let Some(tokens) = highlights.get(&blocks[1]) {
            assert!(
                tokens
                    .iter()
                    .all(|(range, _)| range.end <= blocks[1].code().len())
            );
        }
        let first = highlights.get(&blocks[0]).unwrap();
        assert!(
            first
                .iter()
                .any(|(range, _)| &blocks[0].code()[range.clone()] == "fn")
        );
    }

    #[test]
    fn keeps_tokens_of_unchanged_blocks_in_the_same_theme() {
        let source = "```rust\nfn main() {}\n```\n";
        let theme = HighlightTheme::default_dark();
        let document = MarkdownDocument::parse(source);
        let first =
            CodeHighlights::compute(&document, &CodeHighlights::default(), theme.clone(), || {
                false
            })
            .unwrap();
        let block = &code_blocks(&document)[0];
        let second = CodeHighlights::compute(&document, &first, theme, || false).unwrap();
        assert!(Arc::ptr_eq(
            first.get(block).unwrap(),
            second.get(block).unwrap()
        ));

        let light = HighlightTheme::default_light();
        let third = CodeHighlights::compute(&document, &second, light.clone(), || false).unwrap();
        assert!(third.is_for(&light));
        assert!(!Arc::ptr_eq(
            second.get(block).unwrap(),
            third.get(block).unwrap()
        ));
    }

    #[test]
    fn skips_blocks_past_the_size_limit() {
        let long = "let x = 1;\n".repeat(MAX_HIGHLIGHT_BYTES / 10);
        let source = format!("```rust\n{long}```\n");
        assert!(highlights(&source).blocks.is_empty());
    }

    #[test]
    fn stops_when_cancelled() {
        let document = MarkdownDocument::parse("```rust\nfn main() {}\n```\n");
        let result = CodeHighlights::compute(
            &document,
            &CodeHighlights::default(),
            HighlightTheme::default_dark(),
            || true,
        );
        assert!(result.is_none());
    }

    #[test]
    fn end_point_counts_rows_and_bytes() {
        assert_eq!(end_point(""), tree_sitter::Point::new(0, 0));
        assert_eq!(end_point("ab"), tree_sitter::Point::new(0, 2));
        assert_eq!(end_point("a\nbé"), tree_sitter::Point::new(1, 3));
        assert_eq!(end_point("a\n"), tree_sitter::Point::new(1, 0));
    }
}
