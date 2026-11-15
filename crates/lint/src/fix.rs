//! Applying fixes without breaking the document.

use std::ops::Range;

use crate::config::Config;
use crate::context::Context;
use crate::{Edit, Finding, RuleName, rules};

/// Which fixes to apply.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FixMode {
    /// Every fix of every rule that runs, as for an explicit command.
    All,
    /// Only fixes of rules set to fix on save.
    OnSave,
}

/// A fixed document.
#[derive(Clone, Debug)]
pub struct FixOutcome {
    pub text: String,
    /// How many findings were fixed.
    pub fixed: usize,
    /// Rules with a fix that would have changed the document's structure,
    /// so that fix was left out. Their other fixes were applied.
    pub rejected: Vec<RuleName>,
    /// The edits of each pass, as ranges in that pass's text and the
    /// length of what replaced them.
    passes: Vec<Vec<(Range<usize>, usize)>>,
}

impl FixOutcome {
    /// Where an offset in the original text is in the fixed text: shifted by
    /// the edits before it, and kept within an edit around it.
    pub fn map_offset(&self, offset: usize) -> usize {
        self.passes
            .iter()
            .fold(offset, |offset, pass| shift(offset, pass))
    }
}

/// Where `offset` is after one pass of edits.
fn shift(offset: usize, pass: &[(Range<usize>, usize)]) -> usize {
    let mut shifted = offset as isize;
    for (range, new_len) in pass {
        if range.end <= offset {
            shifted += *new_len as isize - range.len() as isize;
        } else {
            if range.start < offset {
                shifted = shifted - (offset - range.start) as isize
                    + (offset - range.start).min(*new_len) as isize;
            }
            break;
        }
    }
    shifted.max(0) as usize
}

/// Passes before giving up on a document whose fixes keep finding more.
/// Most documents settle in one; a fix that makes room for another, as a
/// blank line inserted between two blocks, takes two.
const MAX_PASSES: usize = 8;

/// Parses spent finding the fixes that break a document's structure, in
/// one call. Rules are searched first, about eight parses for twenty rules,
/// so the rules whose fixes are all fine are applied whole; what is left goes
/// to finding the bad fixes within the others. Past this, the fixes still
/// in doubt are left out.
const SEARCH_BUDGET: usize = 24;

/// `source` with its findings fixed, or `None` when there is nothing to fix.
///
/// Each pass applies every fix that does not overlap another, then parses
/// the result. If its structure differs from the original's, the fixes are
/// halved until the ones that change it are found; those are left out, and
/// the rest applied.
pub fn fix(source: &str, config: &Config, mode: FixMode) -> Option<FixOutcome> {
    if config.is_ignored() {
        return None;
    }
    let collect = |context: &Context<'_>| {
        let mut findings = rules::run(context, config);
        findings.retain(|finding| {
            !finding.fix.is_empty() && (mode == FixMode::All || finding.fix_on_save)
        });
        findings
    };

    let (signature, mut findings) = {
        let context = Context::parse_checked(source);
        (context.signature, collect(&context))
    };
    if findings.is_empty() {
        return None;
    }
    let mut text = source.to_string();
    let mut fixed = 0;
    let mut passes = Vec::new();
    let mut budget = SEARCH_BUDGET;
    for _ in 0..MAX_PASSES {
        let mut chosen = select(&findings);
        if chosen.is_empty() {
            break;
        }
        let mut edits = edits_of(&findings, &chosen);
        let mut candidate = apply(&text, &edits);
        let mut next = findings_if_kept(&candidate, signature, &collect);
        if next.is_none() {
            // Some fix in this pass changes the structure: find which, and
            // leave it out. It is tried again next pass: it may only work
            // once another has landed, as `**` inside `***` turning to `__`
            // once the outer `*` is `_`.
            let good = keep_structure(&text, signature, &findings, &chosen, &mut budget);
            if good.is_empty() {
                break;
            }
            chosen = good;
            edits = edits_of(&findings, &chosen);
            candidate = apply(&text, &edits);
            // Fixes fine alone may still clash together; stop rather than
            // guess.
            next = findings_if_kept(&candidate, signature, &collect);
            if next.is_none() {
                break;
            }
        }
        passes.push(lengths(&edits));
        text = candidate;
        fixed += chosen.len();
        findings = next.unwrap_or_default();
    }
    // What is left could not be fixed without changing the structure.
    let mut rejected: Vec<RuleName> = Vec::new();
    for finding in &findings {
        if !rejected.contains(&finding.rule) {
            rejected.push(finding.rule.clone());
        }
    }
    if fixed == 0 && rejected.is_empty() {
        return None;
    }
    Some(FixOutcome {
        text,
        fixed,
        rejected,
        passes,
    })
}

/// The findings in `candidate`, from the same parse that checks it keeps
/// the structure, or `None` if it does not.
fn findings_if_kept(
    candidate: &str,
    signature: u64,
    collect: &impl Fn(&Context<'_>) -> Vec<Finding>,
) -> Option<Vec<Finding>> {
    let context = Context::parse_checked(candidate);
    (context.signature == signature).then(|| collect(&context))
}

/// Which of the `chosen` findings, whose fixes together change the
/// structure of `text`, keep it: whole rules first, then single fixes
/// within the rules that do not.
fn keep_structure(
    text: &str,
    signature: u64,
    findings: &[Finding],
    chosen: &[usize],
    budget: &mut usize,
) -> Vec<usize> {
    let mut rules: Vec<Vec<usize>> = Vec::new();
    for &ix in chosen {
        match rules
            .iter_mut()
            .find(|rule| findings[rule[0]].rule == findings[ix].rule)
        {
            Some(rule) => rule.push(ix),
            None => rules.push(vec![ix]),
        }
    }
    let mut good = Vec::new();
    let mut bad_rules = Vec::new();
    search(
        text,
        signature,
        findings,
        &rules,
        budget,
        &mut good,
        &mut bad_rules,
    );
    for rule in bad_rules {
        let singles: Vec<Vec<usize>> = rule.iter().map(|ix| vec![*ix]).collect();
        search(
            text,
            signature,
            findings,
            &singles,
            budget,
            &mut good,
            &mut Vec::new(),
        );
    }
    good
}

/// Sort `groups` of findings, known together to change the structure of
/// `text`, by halving: groups whose fixes keep it go to `good`, groups
/// that break it alone, or are still in doubt when the budget runs out, to
/// `bad`.
fn search(
    text: &str,
    signature: u64,
    findings: &[Finding],
    groups: &[Vec<usize>],
    budget: &mut usize,
    good: &mut Vec<usize>,
    bad: &mut Vec<Vec<usize>>,
) {
    if groups.len() < 2 {
        bad.extend(groups.iter().cloned());
        return;
    }
    let (left, right) = groups.split_at(groups.len() / 2);
    for half in [left, right] {
        if *budget == 0 {
            bad.extend(half.iter().cloned());
            continue;
        }
        *budget -= 1;
        let members: Vec<usize> = half.iter().flatten().copied().collect();
        let candidate = apply(text, &edits_of(findings, &members));
        if Context::parse_checked(&candidate).signature == signature {
            good.extend(members);
        } else {
            search(text, signature, findings, half, budget, good, bad);
        }
    }
}

/// The findings that do not overlap an earlier finding's, by index. A
/// finding's edits are taken together or not at all.
fn select(findings: &[Finding]) -> Vec<usize> {
    let mut accepted: Vec<Edit> = Vec::new();
    let mut chosen = Vec::new();
    'findings: for (ix, finding) in findings.iter().enumerate() {
        for edit in &finding.fix {
            if conflicts(&accepted, edit) {
                continue 'findings;
            }
        }
        for edit in &finding.fix {
            let at = accepted.partition_point(|other| {
                (other.range.start, other.range.end) <= (edit.range.start, edit.range.end)
            });
            accepted.insert(at, edit.clone());
        }
        chosen.push(ix);
    }
    chosen
}

/// The edits of the `chosen` findings, sorted, each once: two rules may make
/// the same edit, as a blank line one wants after a heading and another
/// before a code block.
fn edits_of(findings: &[Finding], chosen: &[usize]) -> Vec<Edit> {
    let mut edits: Vec<Edit> = chosen
        .iter()
        .flat_map(|ix| findings[*ix].fix.iter().cloned())
        .collect();
    edits.sort_by_key(|edit| (edit.range.start, edit.range.end));
    edits.dedup();
    edits
}

/// Whether `edit` overlaps an edit in `accepted`, which is sorted and
/// free of overlaps. An insertion at the edge of a replacement does not
/// overlap it; two different insertions at one place do, as their order
/// would be a guess.
fn conflicts(accepted: &[Edit], edit: &Edit) -> bool {
    let range = &edit.range;
    // Only edits starting up to `range`'s end can overlap it. Of those
    // starting before it, only the last can reach into it: any other would
    // overlap that one.
    let start = accepted.partition_point(|other| other.range.start < range.start);
    let end = accepted.partition_point(|other| other.range.start <= range.end);
    let earlier = start.checked_sub(1).map(|ix| &accepted[ix]);
    earlier
        .into_iter()
        .chain(&accepted[start..end])
        .any(|other| {
            if other == edit {
                return false;
            }
            let other = &other.range;
            match (range.is_empty(), other.is_empty()) {
                (true, true) => range.start == other.start,
                (true, false) => other.start < range.start && range.start < other.end,
                (false, true) => range.start < other.start && other.start < range.end,
                (false, false) => range.start < other.end && other.start < range.end,
            }
        })
}

/// `text` with `edits`, which are sorted and do not overlap.
fn apply(text: &str, edits: &[Edit]) -> String {
    let grows: isize = edits
        .iter()
        .map(|edit| edit.text.len() as isize - edit.range.len() as isize)
        .sum();
    let mut out = String::with_capacity((text.len() as isize + grows.max(0)) as usize);
    let mut at = 0;
    for edit in edits {
        out.push_str(&text[at..edit.range.start]);
        out.push_str(&edit.text);
        at = edit.range.end;
    }
    out.push_str(&text[at..]);
    out
}

fn lengths(edits: &[Edit]) -> Vec<(Range<usize>, usize)> {
    edits
        .iter()
        .map(|edit| (edit.range.clone(), edit.text.len()))
        .collect()
}

/// The one edit that turns `old` into `new`: what lies between their common
/// start and their common end. An editor can apply it as a single change,
/// so the whole fix undoes in one step and text outside it is untouched.
pub fn minimal_edit(old: &str, new: &str) -> Option<Edit> {
    if old == new {
        return None;
    }
    let (old_bytes, new_bytes) = (old.as_bytes(), new.as_bytes());
    let mut prefix = old_bytes
        .iter()
        .zip(new_bytes)
        .take_while(|(a, b)| a == b)
        .count();
    while !old.is_char_boundary(prefix) || !new.is_char_boundary(prefix) {
        prefix -= 1;
    }
    let max_suffix = old.len().min(new.len()) - prefix;
    let mut suffix = old_bytes
        .iter()
        .rev()
        .zip(new_bytes.iter().rev())
        .take(max_suffix)
        .take_while(|(a, b)| a == b)
        .count();
    while !old.is_char_boundary(old.len() - suffix) || !new.is_char_boundary(new.len() - suffix) {
        suffix -= 1;
    }
    Some(Edit {
        range: prefix..old.len() - suffix,
        text: new[prefix..new.len() - suffix].to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn edit(range: Range<usize>, text: &str) -> Edit {
        Edit {
            range,
            text: text.into(),
        }
    }

    #[test]
    fn overlapping_edits_conflict() {
        let accepted = vec![edit(2..2, "x"), edit(4..8, "")];
        assert!(!conflicts(&accepted, &edit(0..2, "")));
        assert!(!conflicts(&accepted, &edit(2..4, "")));
        assert!(conflicts(&accepted, &edit(2..2, "y")));
        assert!(!conflicts(&accepted, &edit(2..2, "x")));
        assert!(conflicts(&accepted, &edit(5..6, "")));
        assert!(conflicts(&accepted, &edit(6..6, "")));
        assert!(!conflicts(&accepted, &edit(8..8, "")));
        assert!(conflicts(&accepted, &edit(1..3, "")));
        assert!(conflicts(&[edit(0..10, "")], &edit(5..6, "")));
    }

    #[test]
    fn whole_document_fixes_settle() {
        let config = Config::default();
        let source = "# Title ##\nText  \n\n\n\n```\ncode  \n```\nmore\n\n\n";
        let outcome = fix(source, &config, FixMode::OnSave).unwrap();
        assert_eq!(
            outcome.text,
            "# Title\n\nText\n\n```\ncode  \n```\n\nmore\n"
        );
        assert!(outcome.rejected.is_empty());
        // Fixing again finds nothing.
        assert!(fix(&outcome.text, &config, FixMode::OnSave).is_none());
    }

    #[test]
    fn offsets_follow_the_fixes() {
        let config = Config::default();
        let source = "a  \n\n\n\nb\n";
        let outcome = fix(source, &config, FixMode::All).unwrap();
        assert_eq!(outcome.text, "a\n\nb\n");
        assert_eq!(outcome.map_offset(0), 0);
        // Inside the removed spaces, the cursor stays at their start.
        assert_eq!(outcome.map_offset(2), 1);
        assert_eq!(&outcome.text[outcome.map_offset(7)..], "b\n");
    }

    #[test]
    fn minimal_edits_cover_only_the_change() {
        assert_eq!(minimal_edit("abc", "abc"), None);
        assert_eq!(minimal_edit("a  \nb", "a\nb"), Some(edit(1..3, "")));
        assert_eq!(minimal_edit("aXa", "aYa"), Some(edit(1..2, "Y")));
        assert_eq!(minimal_edit("中文", "中 文"), Some(edit(3..3, " ")));
        assert_eq!(minimal_edit("aa", "aaa"), Some(edit(2..2, "a")));
        // Multi-byte characters sharing leading bytes are not split.
        assert_eq!(minimal_edit("é", "ê"), Some(edit(0..2, "ê")));
    }
}
