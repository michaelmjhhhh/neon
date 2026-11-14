//! The built-in rules and how they run.

mod headings;
mod lists;
mod spacing;
mod style;
mod tables;
mod text;

use std::borrow::Cow;
use std::ops::Range;

use crate::config::{ActiveRule, Config, Level};
use crate::context::Context;
use crate::{Edit, Finding, RuleName};

/// What a built-in rule is and how it behaves by default.
pub struct RuleInfo {
    /// The name configuration files and comments use.
    pub name: &'static str,
    /// The matching markdownlint rule, accepted in place of the name.
    pub alias: Option<&'static str>,
    pub description: &'static str,
    /// [`Level::Off`] for a rule that runs only when configured.
    pub default_level: Level,
    /// Whether saving applies the rule's fixes unless configured otherwise.
    /// Only rules that change spacing do: others change what the text says.
    pub fix_on_save: bool,
    pub fixable: bool,
    /// See [`Finding::transient`].
    pub transient: bool,
    pub(crate) options: fn(&toml::Table) -> Result<Options, String>,
    pub(crate) check: fn(&Context<'_>, &Options, &mut Sink<'_, '_>),
}

impl RuleInfo {
    pub(crate) fn names(&self) -> [&'static str; 2] {
        [self.name, self.alias.unwrap_or(self.name)]
    }
}

/// Every built-in rule, in the order they run.
pub fn rules() -> &'static [RuleInfo] {
    RULES
}

/// A rule's options, read once when its configuration is resolved.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Options {
    None,
    Max(usize),
    Chars(Vec<char>),
    /// A marker style: the document's first marker, or the one given.
    Marker(Option<char>),
    Ascending(bool),
    Language(String),
}

/// Collects one rule's findings, dropping those comments turn off and
/// fixes that would touch protected text.
pub(crate) struct Sink<'a, 'c> {
    context: &'a Context<'c>,
    rule: &'a ActiveRule,
    out: &'a mut Vec<Finding>,
}

impl Sink<'_, '_> {
    pub fn report(
        &mut self,
        range: Range<usize>,
        message: impl Into<Cow<'static, str>>,
        fix: Vec<Edit>,
    ) {
        let info = self.rule.info;
        if !self.context.suppressions.is_empty()
            && self.context.suppressions.covers(range.start, &info.names())
        {
            return;
        }
        if fix
            .iter()
            .any(|edit| self.context.is_protected(&edit.range))
        {
            // A rule's own checks should keep it off protected text; a fix
            // that reaches it anyway is a finding to distrust.
            return;
        }
        self.out.push(Finding {
            rule: RuleName::Builtin(info.name),
            range,
            message: message.into(),
            level: self.rule.level,
            fix,
            fix_on_save: self.rule.fix_on_save,
            transient: info.transient,
        });
    }
}

/// Run `config`'s rules over `context`, or only the rule named `only`.
pub(crate) fn run(context: &Context<'_>, config: &Config, only: Option<&str>) -> Vec<Finding> {
    let mut findings = Vec::new();
    for rule in config.active_rules() {
        if only.is_some_and(|only| only != rule.info.name) {
            continue;
        }
        let mut sink = Sink {
            context,
            rule,
            out: &mut findings,
        };
        (rule.info.check)(context, &rule.options, &mut sink);
    }
    for custom in config.custom_rules() {
        if only.is_some_and(|only| only != &*custom.name) {
            continue;
        }
        custom.check(context, &mut findings);
    }
    findings.sort_by_key(|finding| (finding.range.start, finding.range.end));
    findings
}

const fn rule(
    name: &'static str,
    alias: Option<&'static str>,
    description: &'static str,
    default_level: Level,
    check: fn(&Context<'_>, &Options, &mut Sink<'_, '_>),
) -> RuleInfo {
    RuleInfo {
        name,
        alias,
        description,
        default_level,
        fix_on_save: true,
        fixable: true,
        transient: false,
        options: no_options,
        check,
    }
}

impl RuleInfo {
    const fn options(mut self, options: fn(&toml::Table) -> Result<Options, String>) -> Self {
        self.options = options;
        self
    }

    /// Fixes that change what the text says wait for an explicit fix.
    const fn content(mut self) -> Self {
        self.fix_on_save = false;
        self
    }

    const fn report_only(mut self) -> Self {
        self.fix_on_save = false;
        self.fixable = false;
        self
    }

    const fn transient(mut self) -> Self {
        self.transient = true;
        self
    }
}

static RULES: &[RuleInfo] = &[
    rule(
        "trailing-spaces",
        Some("MD009"),
        "Spaces or tabs at the end of a line, except a two-space line break.",
        Level::Warning,
        spacing::trailing_spaces,
    )
    .transient(),
    rule(
        "consecutive-blank-lines",
        Some("MD012"),
        "More blank lines in a row than `max` (1), or blank lines at the start.",
        Level::Warning,
        spacing::consecutive_blank_lines,
    )
    .options(spacing::max_options)
    .transient(),
    rule(
        "final-newline",
        Some("MD047"),
        "The file ends with exactly one line ending.",
        Level::Warning,
        spacing::final_newline,
    )
    .transient(),
    rule(
        "blank-lines-around-headings",
        Some("MD022"),
        "A blank line before and after each heading.",
        Level::Warning,
        spacing::around_headings,
    ),
    rule(
        "blank-lines-around-fences",
        Some("MD031"),
        "A blank line before and after each fenced code block.",
        Level::Warning,
        spacing::around_fences,
    ),
    rule(
        "blank-lines-around-tables",
        Some("MD058"),
        "A blank line before and after each table.",
        Level::Warning,
        spacing::around_tables,
    ),
    rule(
        "blank-lines-around-blockquotes",
        None,
        "A blank line before and after each block quote and callout.",
        Level::Off,
        spacing::around_blockquotes,
    ),
    rule(
        "blank-lines-around-rules",
        None,
        "A blank line before and after each horizontal rule.",
        Level::Off,
        spacing::around_rules,
    ),
    rule(
        "heading-trailing-hashes",
        Some("MD020"),
        "No closing `#`s after a heading's text.",
        Level::Warning,
        headings::trailing_hashes,
    ),
    rule(
        "heading-increment",
        Some("MD001"),
        "Heading levels go down one at a time, as from H2 to H3.",
        Level::Warning,
        headings::increment,
    )
    .report_only(),
    rule(
        "single-h1",
        Some("MD025"),
        "At most one top-level heading.",
        Level::Off,
        headings::single_h1,
    )
    .report_only(),
    rule(
        "heading-punctuation",
        Some("MD026"),
        "No punctuation at the end of a heading; `punctuation` lists which.",
        Level::Off,
        headings::punctuation,
    )
    .options(headings::punctuation_options)
    .content(),
    rule(
        "multiple-spaces",
        None,
        "One space between words.",
        Level::Off,
        text::multiple_spaces,
    ),
    rule(
        "unordered-list-marker",
        Some("MD004"),
        "One bullet: `style` is consistent, dash, asterisk or plus.",
        Level::Off,
        lists::unordered_marker,
    )
    .options(lists::marker_options),
    rule(
        "ordered-list-numbering",
        Some("MD029"),
        "Numbered items count up (`style = \"ascending\"`) or repeat the first number (`\"lazy\"`).",
        Level::Off,
        lists::ordered_numbering,
    )
    .options(lists::numbering_options),
    rule(
        "emphasis-style",
        Some("MD049"),
        "One emphasis marker: `style` is consistent, asterisk or underscore.",
        Level::Off,
        style::emphasis,
    )
    .options(style::delimiter_options),
    rule(
        "strong-style",
        Some("MD050"),
        "One bold marker: `style` is consistent, asterisk or underscore.",
        Level::Off,
        style::strong,
    )
    .options(style::delimiter_options),
    rule(
        "code-fence-style",
        Some("MD048"),
        "One code fence: `style` is consistent, backtick or tilde.",
        Level::Off,
        style::fence,
    )
    .options(style::fence_options),
    rule(
        "fence-language",
        Some("MD040"),
        "Every fenced code block names a language; the fix adds `language`.",
        Level::Off,
        style::fence_language,
    )
    .options(style::language_options)
    .content(),
    rule(
        "cjk-spacing",
        None,
        "A space between Chinese, Japanese or Korean and Latin letters or digits.",
        Level::Off,
        text::cjk_spacing,
    ),
    rule(
        "proper-ellipsis",
        None,
        "Three periods become an ellipsis character.",
        Level::Off,
        text::proper_ellipsis,
    )
    .content(),
    rule(
        "table-alignment",
        None,
        "Table columns padded to line up in the source.",
        Level::Off,
        tables::alignment,
    ),
];

pub(crate) fn no_options(table: &toml::Table) -> Result<Options, String> {
    reject_unknown(table, &[])?;
    Ok(Options::None)
}

/// An error naming the first key not in `known`.
pub(crate) fn reject_unknown(table: &toml::Table, known: &[&str]) -> Result<(), String> {
    match table.keys().find(|key| !known.contains(&key.as_str())) {
        Some(key) => Err(format!("unknown option `{key}`")),
        None => Ok(()),
    }
}

/// A `style` option naming one of `choices`, or `consistent`.
pub(crate) fn marker_style(
    table: &toml::Table,
    choices: &[(&str, char)],
) -> Result<Options, String> {
    reject_unknown(table, &["style"])?;
    let Some(value) = table.get("style") else {
        return Ok(Options::Marker(None));
    };
    let style = value.as_str().ok_or("`style` must be a string")?;
    if style == "consistent" {
        return Ok(Options::Marker(None));
    }
    choices
        .iter()
        .find(|(name, _)| *name == style)
        .map(|(_, marker)| Options::Marker(Some(*marker)))
        .ok_or_else(|| {
            let names: Vec<&str> = choices.iter().map(|(name, _)| *name).collect();
            format!("`style` must be consistent, {}", names.join(" or "))
        })
}

#[cfg(test)]
pub(crate) mod test_support {
    use super::*;
    use crate::config::Config;

    /// The findings of the rule `name` alone, with `options`.
    pub fn findings(name: &str, options: &str, source: &str) -> Vec<Finding> {
        let config = Config::only(name, options);
        crate::lint(source, &config)
    }

    /// `source` with the rule `name`'s fixes applied, alone.
    pub fn fixed(name: &str, options: &str, source: &str) -> String {
        let config = Config::only(name, options);
        match crate::fix(source, &config, crate::FixMode::All) {
            Some(outcome) => {
                assert!(
                    outcome.rejected.is_empty(),
                    "{name} was rejected on {source:?}"
                );
                outcome.text
            }
            None => source.to_string(),
        }
    }

    #[test]
    fn rule_names_are_unique() {
        let mut names: Vec<&str> = RULES.iter().flat_map(|rule| rule.names()).collect();
        names.sort_unstable();
        let before = names.len();
        names.dedup();
        // A rule without an alias lists its name twice.
        let aliasless = RULES.iter().filter(|rule| rule.alias.is_none()).count();
        assert_eq!(names.len(), before - aliasless);
    }
}
