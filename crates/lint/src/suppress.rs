//! Comments that turn rules off for part of a document:
//!
//! ```markdown
//! <!-- neonlint-disable trailing-spaces -->
//! …
//! <!-- neonlint-enable trailing-spaces -->
//! <!-- neonlint-disable-next-line -->
//! <!-- neonlint-disable-line heading-increment -->
//! <!-- neonlint-disable-file proper-ellipsis -->
//! ```
//!
//! A comment without rule names covers every rule. `markdownlint-` works in
//! place of `neonlint-`, so documents written for markdownlint keep their
//! exceptions.

use std::ops::Range;

use crate::context::Lines;

/// The rules a span turns off.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Rules {
    All,
    /// Lowercase names or aliases.
    Named(Vec<String>),
}

impl Rules {
    fn covers(&self, names: &[&str]) -> bool {
        match self {
            Rules::All => true,
            Rules::Named(listed) => listed
                .iter()
                .any(|listed| names.iter().any(|name| listed.eq_ignore_ascii_case(name))),
        }
    }
}

#[derive(Clone, Debug, Default)]
pub(crate) struct Suppressions {
    spans: Vec<(Range<usize>, Rules)>,
}

impl Suppressions {
    pub fn from_comments(source: &str, lines: &Lines, comments: &[Range<usize>]) -> Self {
        let mut spans = Vec::new();
        // Disables still open: where each started and what it covers.
        let mut open: Vec<(usize, Rules)> = Vec::new();
        for comment in comments {
            let Some((directive, rules)) = parse(&source[comment.clone()]) else {
                continue;
            };
            let line = lines.of(comment.start);
            match directive {
                Directive::Disable => open.push((comment.end, rules)),
                Directive::Enable => {
                    let ended = |(_, open): &(usize, Rules)| match (&rules, open) {
                        (Rules::All, _) => true,
                        (Rules::Named(names), Rules::Named(open)) => {
                            open.iter().all(|rule| names.contains(rule))
                        }
                        (Rules::Named(_), Rules::All) => false,
                    };
                    let mut ix = 0;
                    while ix < open.len() {
                        if ended(&open[ix]) {
                            let (start, rules) = open.remove(ix);
                            spans.push((start..comment.start, rules));
                        } else {
                            ix += 1;
                        }
                    }
                }
                Directive::DisableLine => spans.push((lines.full(line), rules)),
                Directive::DisableNextLine => {
                    let last = lines.of(comment.end.saturating_sub(1).max(comment.start));
                    if last + 1 < lines.count() {
                        spans.push((lines.full(last + 1), rules));
                    }
                }
                Directive::DisableFile => spans.push((0..source.len() + 1, rules)),
            }
        }
        for (start, rules) in open {
            spans.push((start..source.len() + 1, rules));
        }
        Self { spans }
    }

    /// Whether a finding of the rule known by `names` at `offset` is turned
    /// off.
    pub fn covers(&self, offset: usize, names: &[&str]) -> bool {
        self.spans
            .iter()
            .any(|(span, rules)| span.contains(&offset) && rules.covers(names))
    }

    pub fn is_empty(&self) -> bool {
        self.spans.is_empty()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Directive {
    Disable,
    Enable,
    DisableLine,
    DisableNextLine,
    DisableFile,
}

fn parse(comment: &str) -> Option<(Directive, Rules)> {
    let body = comment
        .trim()
        .strip_prefix("<!--")?
        .trim_end()
        .strip_suffix("-->")?
        .trim();
    let rest = body
        .strip_prefix("neonlint-")
        .or_else(|| body.strip_prefix("markdownlint-"))?;
    let (word, names) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
    let directive = match word {
        "disable" => Directive::Disable,
        "enable" => Directive::Enable,
        "disable-line" => Directive::DisableLine,
        "disable-next-line" => Directive::DisableNextLine,
        "disable-file" => Directive::DisableFile,
        _ => return None,
    };
    let names: Vec<String> = names
        .split(|ch: char| ch.is_whitespace() || ch == ',')
        .filter(|name| !name.is_empty())
        .map(str::to_ascii_lowercase)
        .collect();
    let rules = if names.is_empty() {
        Rules::All
    } else {
        Rules::Named(names)
    };
    Some((directive, rules))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_directives() {
        assert_eq!(
            parse("<!-- neonlint-disable a, b -->"),
            Some((
                Directive::Disable,
                Rules::Named(vec!["a".into(), "b".into()])
            ))
        );
        assert_eq!(
            parse("<!-- markdownlint-disable-next-line -->"),
            Some((Directive::DisableNextLine, Rules::All))
        );
        assert_eq!(parse("<!-- neonlint-disabled -->"), None);
        assert_eq!(parse("<!-- a note -->"), None);
    }
}
