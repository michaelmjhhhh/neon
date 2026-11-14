//! Markdown linting for Neon, independent of any UI framework.
//!
//! A document is parsed once into a [`context::Context`]: its lines, its
//! top-level blocks, the runs of plain text, and the ranges no rule may
//! change, such as code, math, HTML and link targets. Rules read that
//! context and report [`Finding`]s, each with an optional [`Edit`] that
//! fixes it.
//!
//! [`fix`] applies those edits and checks that the document still parses
//! to the same structure: the same blocks and inline tags, the same code,
//! the same link targets. A rule whose fixes would change the structure is
//! dropped for that document rather than trusted.
//!
//! Which rules run comes from [`Config`], merged from the built-in defaults,
//! a global file and `.neonlint.toml` files in the folders above a note;
//! see [`Resolver`].

mod config;
mod context;
mod fix;
mod glob;
mod rules;
mod suppress;

use std::borrow::Cow;
use std::fmt;
use std::ops::Range;
use std::sync::Arc;

pub use config::{CONFIG_FILE_NAME, Config, Level, Resolver};
pub use fix::{FixMode, FixOutcome, fix, minimal_edit};
pub use rules::{RuleInfo, rules};

/// Replace `range` of the source with `text`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Edit {
    pub range: Range<usize>,
    pub text: String,
}

/// Which rule reported a finding: a built-in one, or a pattern from a
/// configuration file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RuleName {
    Builtin(&'static str),
    Custom(Arc<str>),
}

impl RuleName {
    pub fn as_str(&self) -> &str {
        match self {
            RuleName::Builtin(name) => name,
            RuleName::Custom(name) => name,
        }
    }
}

impl fmt::Display for RuleName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One problem in a document.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Finding {
    pub rule: RuleName,
    /// Byte range in the linted source.
    pub range: Range<usize>,
    pub message: Cow<'static, str>,
    pub level: Level,
    /// The edits that fix it, applied together; empty for a finding with
    /// no fix.
    pub fix: Vec<Edit>,
    /// Whether saving applies the fix, when saving fixes at all.
    pub fix_on_save: bool,
    /// Whether the finding goes stale as soon as the writer types on, as a
    /// trailing space does mid-sentence, so an editor may hold it back on
    /// the line being written.
    pub transient: bool,
}

/// Every problem `config` finds in `source`, in source order. Nothing for a
/// file the configuration ignores.
pub fn lint(source: &str, config: &Config) -> Vec<Finding> {
    if config.is_ignored() {
        return Vec::new();
    }
    let context = context::Context::parse(source);
    rules::run(&context, config, None)
}

/// Zero-based lines and columns, in characters, of byte offsets into a
/// source, as editors address positions.
pub struct Positions<'a> {
    source: &'a str,
    lines: context::Lines,
}

impl<'a> Positions<'a> {
    pub fn new(source: &'a str) -> Self {
        Self {
            source,
            lines: context::Lines::new(source),
        }
    }

    /// The line and column of `offset`, in time proportional to the length
    /// of its line.
    pub fn at(&self, offset: usize) -> (u32, u32) {
        let offset = floor_char_boundary(self.source, offset.min(self.source.len()));
        let line = self.lines.of(offset);
        let column = self.source[self.lines.start(line)..offset].chars().count();
        (line as u32, column as u32)
    }
}

pub(crate) fn floor_char_boundary(text: &str, mut offset: usize) -> usize {
    while !text.is_char_boundary(offset) {
        offset -= 1;
    }
    offset
}
