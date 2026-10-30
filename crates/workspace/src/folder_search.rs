//! Searching the Markdown files of a folder for text, for the sidebar's
//! Search view.
//!
//! There is no index: each search reads the files again, as ripgrep, VS
//! Code and Zed do. A folder of notes is small enough to read in
//! milliseconds, and an index would have to be kept in step with every
//! change made outside Neon. The files are searched in parallel, an open
//! document's unsaved text in place of its file, and the regex engine skips
//! ahead with SIMD to the bytes a match must contain.

use std::collections::HashMap;
use std::fs;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use gpui_kit::SharedString;
use rayon::prelude::*;
use regex::{Regex, RegexBuilder};

/// The most matches a search lists, as in VS Code. More than anyone reads
/// through, and few enough to keep in memory and lay out at once.
pub(crate) const MAX_MATCHES: usize = 10_000;
/// Larger files are skipped: no note is this long, and reading one would
/// hold up every search.
const MAX_FILE_BYTES: u64 = 16 * 1024 * 1024;
/// How far into a file to look for a NUL byte, ripgrep's sign of a binary
/// file.
const BINARY_PROBE_BYTES: usize = 8 * 1024;
/// How many columns of the line before a match its preview keeps, at most,
/// so a match deep in a long paragraph still shows in a narrow sidebar. A
/// Chinese character takes two, so the lead is 16 English letters or 8
/// Chinese characters: about the same width.
const PREVIEW_LEAD_COLUMNS: usize = 16;
/// The most of a line a preview keeps, in bytes. The row truncates well
/// before this; the cap only bounds the copy.
const PREVIEW_MAX_BYTES: usize = 240;

/// How a query's text is matched.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct SearchOptions {
    pub(crate) match_case: bool,
    pub(crate) whole_word: bool,
    pub(crate) regex: bool,
}

/// A query ready to run: its text compiled to one regex, escaped unless it
/// is a regex itself.
#[derive(Clone, Debug)]
pub(crate) struct Query {
    regex: Regex,
    whole_word: bool,
}

impl Query {
    /// `None` for an empty query. An invalid regex is an error that says
    /// what is wrong with it.
    pub(crate) fn new(text: &str, options: SearchOptions) -> Result<Option<Self>, String> {
        if text.is_empty() {
            return Ok(None);
        }
        let pattern = if options.regex {
            text.to_string()
        } else {
            regex::escape(text)
        };
        RegexBuilder::new(&pattern)
            .case_insensitive(!options.match_case)
            // `^` and `$` match at each line, as they do in a line-based
            // search like ripgrep's, before `\r\n` as well as `\n`.
            .multi_line(true)
            .crlf(true)
            .build()
            .map(|regex| {
                Some(Self {
                    regex,
                    whole_word: options.whole_word,
                })
            })
            .map_err(|error| regex_problem(&error))
    }

    /// The byte ranges of the matches in `text`, in order. Empty matches,
    /// which a regex like `a*` finds between every two characters, are left
    /// out: there is nothing to show of them.
    ///
    /// A match that is not a whole word is tried again from its next
    /// character, not from its end, so one inside it is still found: `a a`
    /// in `ba a a`, after `a a` at 1 is turned down, is found at 3.
    fn matches<'a>(&'a self, text: &'a str) -> impl Iterator<Item = Range<usize>> + 'a {
        let mut at = 0;
        std::iter::from_fn(move || {
            while at <= text.len() {
                let range = self.regex.find_at(text, at)?.range();
                if range.is_empty() {
                    at = next_char(text, range.end);
                } else if self.whole_word && !is_whole_word(text, &range) {
                    at = next_char(text, range.start);
                } else {
                    at = range.end;
                    return Some(range);
                }
            }
            None
        })
    }
}

/// Where the character after the one at `ix` starts; past the end at the
/// end.
fn next_char(text: &str, ix: usize) -> usize {
    text[ix..]
        .chars()
        .next()
        .map_or(text.len() + 1, |c| ix + c.len_utf8())
}

/// What is wrong with a regex, as one line: the parser's message without
/// the pattern and caret it draws above it.
fn regex_problem(error: &regex::Error) -> String {
    match error {
        regex::Error::Syntax(message) => {
            let reason = message
                .lines()
                .rev()
                .find(|line| !line.trim().is_empty())
                .unwrap_or(message)
                .trim()
                .trim_start_matches("error: ");
            crate::workspace::sentence(reason)
        }
        regex::Error::CompiledTooBig(_) => "The pattern is too large.".to_string(),
        _ => crate::workspace::sentence(&error.to_string()),
    }
}

/// Whether the match at `range` stands as a word of its own: no letter,
/// digit or underscore runs on from either end. An end that is itself
/// punctuation, as in `c++`, needs nothing around it.
///
/// Chinese writes words without spaces between them, so a Han character
/// never joins the one beside it: `计划` is a whole word in `这个计划很好`,
/// and `Rust` in `使用Rust编写`.
fn is_whole_word(text: &str, range: &Range<usize>) -> bool {
    fn joins(a: Option<char>, b: Option<char>) -> bool {
        a.is_some_and(is_latin_word) && b.is_some_and(is_latin_word)
    }
    let found = &text[range.clone()];
    !joins(
        text[..range.start].chars().next_back(),
        found.chars().next(),
    ) && !joins(found.chars().next_back(), text[range.end..].chars().next())
}

/// A letter, digit or underscore of a word that spaces separate, as in
/// English; not a Han character.
fn is_latin_word(c: char) -> bool {
    (c.is_alphanumeric() || c == '_') && !is_han(c)
}

/// Whether `c` is a Chinese character.
fn is_han(c: char) -> bool {
    matches!(c,
        '\u{3007}'                  // 〇
        | '\u{3400}'..='\u{4DBF}'   // CJK Extension A
        | '\u{4E00}'..='\u{9FFF}'   // CJK Unified Ideographs
        | '\u{F900}'..='\u{FAFF}'   // CJK Compatibility Ideographs
        | '\u{20000}'..='\u{323AF}' // CJK Extensions B to H
    )
}

/// How many columns `c` takes in a row: two for Chinese characters and the
/// full-width punctuation written with them, such as `，` and `：`; one for
/// the rest.
fn display_columns(c: char) -> usize {
    let wide = is_han(c)
        || matches!(c,
            '\u{3000}'..='\u{303F}'   // CJK punctuation
            | '\u{FF00}'..='\u{FF60}' // Full-width forms
            | '\u{FFE0}'..='\u{FFE6}'
        );
    if wide { 2 } else { 1 }
}

/// One match, with the line it is on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct LineMatch {
    /// The line's index, from 0.
    pub(crate) line: usize,
    /// The match's bytes within the line. A match across lines ends at the
    /// end of the first.
    pub(crate) columns: Range<usize>,
    /// The line around the match, as its row shows it.
    pub(crate) preview: SharedString,
    /// The match's bytes within `preview`.
    pub(crate) highlight: Range<usize>,
}

/// A file's matches, in order.
#[derive(Clone, Debug)]
pub(crate) struct FileMatches {
    pub(crate) path: PathBuf,
    pub(crate) matches: Vec<LineMatch>,
}

/// The first `limit` matches in `text`, each with its line.
pub(crate) fn search_text(query: &Query, text: &str, limit: usize) -> Vec<LineMatch> {
    let mut found = Vec::new();
    // Where the last match was. Each match then costs only the text since
    // the one before it, even in a file that is one long line.
    let mut scanned = 0;
    let mut line = 0;
    let mut line_start = 0;
    let mut line_end: Option<usize> = None;
    for range in query.matches(text) {
        if found.len() == limit {
            break;
        }
        let skipped = &text[scanned..range.start];
        let newlines = skipped.bytes().filter(|&byte| byte == b'\n').count();
        if newlines > 0 {
            line += newlines;
            line_start = scanned + skipped.rfind('\n').map_or(0, |ix| ix + 1);
            line_end = None;
        }
        scanned = range.start;
        let end = *line_end.get_or_insert_with(|| {
            text[range.start..]
                .find('\n')
                .map_or(text.len(), |ix| range.start + ix)
        });
        let line_text = text[line_start..end]
            .strip_suffix('\r')
            .unwrap_or(&text[line_start..end]);
        let columns = range.start - line_start..range.end.min(end) - line_start;
        let columns = columns.start.min(line_text.len())..columns.end.min(line_text.len());
        let (preview, highlight) = preview(line_text, columns.clone());
        found.push(LineMatch {
            line,
            columns,
            preview: preview.into(),
            highlight,
        });
    }
    found
}

/// The part of `line` a row shows for the match at `columns`, and where the
/// match is in it: from a little before the match, or from the line's first
/// character that is not a space if that is nearer.
fn preview(line: &str, columns: Range<usize>) -> (String, Range<usize>) {
    let indent = line.len() - line.trim_start().len();
    // Back from the match, as far as the lead's columns reach.
    let mut lead = columns.start;
    let mut width = 0;
    for (ix, c) in line[..columns.start].char_indices().rev() {
        width += display_columns(c);
        if width > PREVIEW_LEAD_COLUMNS {
            break;
        }
        lead = ix;
    }
    let mut start = lead.max(indent.min(columns.start));
    // A cut inside an English word starts at the next word instead, so the
    // preview never opens on a fragment like `tch here`. A Chinese line may
    // be cut between any two characters.
    let mid_word = line[..start].chars().next_back().is_some_and(is_latin_word)
        && line[start..].chars().next().is_some_and(is_latin_word);
    if start > indent
        && mid_word
        && let Some((ix, space)) = line[start..columns.start]
            .char_indices()
            .find(|(_, c)| c.is_whitespace())
    {
        start += ix + space.len_utf8();
    }
    // Nor on the space between two words.
    if start > indent {
        start += line[start..columns.start].len() - line[start..columns.start].trim_start().len();
    }
    let mut end = line.len().min(start + PREVIEW_MAX_BYTES);
    while !line.is_char_boundary(end) {
        end -= 1;
    }
    let mut text = String::with_capacity(end - start + 3);
    if start > indent {
        text.push('…');
    }
    let shift = text.len();
    text.push_str(&line[start..end]);
    let highlight = columns.start - start + shift..columns.end.min(end) - start + shift;
    (text, highlight)
}

/// Where in `line` the query matches nearest `column`: for a match listed
/// before its document was edited, the place it moved to on its line.
pub(crate) fn locate(query: &Query, line: &str, column: usize) -> Option<Range<usize>> {
    query
        .matches(line)
        .min_by_key(|range| range.start.abs_diff(column))
}

/// Search `files` in parallel and keep their order, leaving out files with
/// no match. `limit` is how many more matches are wanted. Each file gives at
/// most one more than that, so a caller that gets more knows there were
/// more to find. A file in `unsaved` is searched as that text, an open
/// document's unsaved edits, not as it is on disk.
///
/// Files not yet started are skipped once `cancelled` is set, or once the
/// files before them already hold more than `limit` matches: a common word
/// stops the search early, and the matches kept are still the first ones.
pub(crate) fn search_files(
    query: &Query,
    files: &[PathBuf],
    unsaved: &HashMap<PathBuf, String>,
    limit: usize,
    cancelled: &AtomicBool,
) -> Vec<FileMatches> {
    let found = FoundSoFar::new(limit);
    files
        .par_iter()
        .enumerate()
        .filter_map(|(ix, path)| {
            if cancelled.load(Ordering::Relaxed) || found.is_past_limit() {
                return None;
            }
            let per_file = found.room().saturating_add(1);
            let matches = match unsaved.get(path) {
                Some(text) => search_text(query, text, per_file),
                None => read_text(path)
                    .map_or_else(Vec::new, |text| search_text(query, &text, per_file)),
            };
            found.record(ix, matches.len());
            (!matches.is_empty()).then(|| FileMatches {
                path: path.clone(),
                matches,
            })
        })
        .collect()
}

/// How many matches the files searched so far hold, counted in file order.
/// Files finish out of order, so a count joins the total only once every
/// file before it has: a file can then be skipped only when all those
/// before it are done, and the matches kept are the first ones.
struct FoundSoFar {
    limit: usize,
    past_limit: AtomicBool,
    /// The files whose counts joined the total all come before any file
    /// still to start, so the total is a floor on what precedes it.
    total: AtomicUsize,
    counted: Mutex<Counted>,
}

#[derive(Default)]
struct Counted {
    /// The first file whose count has not joined the total.
    next: usize,
    total: usize,
    /// Counts of files that finished before one ahead of them.
    waiting: HashMap<usize, usize>,
}

impl FoundSoFar {
    fn new(limit: usize) -> Self {
        Self {
            limit,
            past_limit: AtomicBool::new(false),
            total: AtomicUsize::new(0),
            counted: Mutex::default(),
        }
    }

    fn is_past_limit(&self) -> bool {
        self.past_limit.load(Ordering::Relaxed)
    }

    /// How many matches a file starting now could still add: the limit,
    /// less what the files before it are known to hold.
    fn room(&self) -> usize {
        self.limit
            .saturating_sub(self.total.load(Ordering::Relaxed))
    }

    fn record(&self, ix: usize, count: usize) {
        let mut counted = self
            .counted
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        counted.waiting.insert(ix, count);
        loop {
            let next = counted.next;
            let Some(count) = counted.waiting.remove(&next) else {
                break;
            };
            counted.total += count;
            counted.next += 1;
        }
        self.total.store(counted.total, Ordering::Relaxed);
        if counted.total > self.limit {
            self.past_limit.store(true, Ordering::Relaxed);
        }
    }
}

/// The text of the file at `path`, unless it is too large, binary, or not
/// UTF-8. The editor opens none of those, so none has a match to show.
fn read_text(path: &Path) -> Option<String> {
    if fs::metadata(path).ok()?.len() > MAX_FILE_BYTES {
        return None;
    }
    let bytes = fs::read(path).ok()?;
    if bytes[..bytes.len().min(BINARY_PROBE_BYTES)].contains(&0) {
        return None;
    }
    String::from_utf8(bytes).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn query(text: &str, options: SearchOptions) -> Query {
        Query::new(text, options).unwrap().unwrap()
    }

    fn plain(text: &str) -> Query {
        query(text, SearchOptions::default())
    }

    fn found(query: &Query, text: &str) -> Vec<(usize, Range<usize>)> {
        search_text(query, text, usize::MAX)
            .into_iter()
            .map(|found| (found.line, found.columns))
            .collect()
    }

    #[test]
    fn ignores_case_unless_asked_to_match_it() {
        let text = "Plan\nplan\nPLAN";
        assert_eq!(found(&plain("plan"), text).len(), 3);
        let exact = query(
            "plan",
            SearchOptions {
                match_case: true,
                ..Default::default()
            },
        );
        assert_eq!(found(&exact, text), [(1, 0..4)]);
        // Case folding covers more than ASCII.
        assert_eq!(found(&plain("straße"), "STRASSE Straße").len(), 1);
        assert_eq!(found(&plain("été"), "ÉTÉ").len(), 1);
    }

    #[test]
    fn treats_text_literally_unless_it_is_a_regex() {
        assert_eq!(found(&plain("a.c"), "abc a.c"), [(0, 4..7)]);
        assert_eq!(found(&plain("[x]"), "- [x] done"), [(0, 2..5)]);
        let regex = query(
            r"^- \[ \]",
            SearchOptions {
                regex: true,
                ..Default::default()
            },
        );
        // `^` matches at the start of each line.
        assert_eq!(
            found(&regex, "- [ ] one\n- [x] two\n- [ ] three"),
            [(0, 0..5), (2, 0..5)]
        );
    }

    #[test]
    fn says_what_is_wrong_with_a_regex() {
        let options = SearchOptions {
            regex: true,
            ..Default::default()
        };
        let problem = Query::new("(open", options).unwrap_err();
        assert_eq!(problem, "Unclosed group.");
        assert!(Query::new("", options).unwrap().is_none());
    }

    #[test]
    fn matches_whole_words_at_letters_but_not_punctuation() {
        let whole = |text: &str| {
            query(
                text,
                SearchOptions {
                    whole_word: true,
                    ..Default::default()
                },
            )
        };
        assert_eq!(
            found(&whole("plan"), "plan planning plan_b replan (plan)"),
            [(0, 0..4), (0, 29..33)]
        );
        // A query that ends in punctuation needs no boundary there, but
        // one that starts with a letter does.
        assert_eq!(found(&whole("c++"), "c++x and xc++"), [(0, 0..3)]);
        // A rejected match does not hide the next one, even one that
        // starts inside it.
        assert_eq!(found(&whole("ab"), "abab ab"), [(0, 5..7)]);
        assert_eq!(found(&whole("a a"), "ba a a"), [(0, 3..6)]);
    }

    #[test]
    fn finds_lines_and_columns_in_bytes() {
        let text = "# Notes\r\n\r\n中文 plan\nlast plan";
        assert_eq!(found(&plain("plan"), text), [(2, 7..11), (3, 5..9)]);
        // A line ending in `\r\n` keeps its `\r` out of the preview.
        let matches = search_text(&plain("notes"), text, usize::MAX);
        assert_eq!(matches[0].preview.as_ref(), "# Notes");
    }

    #[test]
    fn matches_line_ends_in_crlf_files() {
        let regex = query(
            "todo$",
            SearchOptions {
                regex: true,
                ..Default::default()
            },
        );
        assert_eq!(
            found(&regex, "a todo\r\nb todo\r\n"),
            [(0, 2..6), (1, 2..6)]
        );
    }

    #[test]
    fn ends_a_match_across_lines_at_the_first_line() {
        let regex = query(
            r"one\ntwo",
            SearchOptions {
                regex: true,
                ..Default::default()
            },
        );
        let matches = search_text(&regex, "one\ntwo", usize::MAX);
        assert_eq!(matches[0].columns, 0..3);
        assert_eq!(&matches[0].preview[matches[0].highlight.clone()], "one");
    }

    #[test]
    fn leaves_out_empty_matches() {
        let regex = query(
            "x*",
            SearchOptions {
                regex: true,
                ..Default::default()
            },
        );
        assert_eq!(found(&regex, "ab xx c"), [(0, 3..5)]);
    }

    #[test]
    fn stops_at_the_limit() {
        assert_eq!(search_text(&plain("a"), "aaaaa", 2).len(), 2);
    }

    #[test]
    fn previews_the_text_just_before_a_match_in_a_long_line() {
        let line = format!("    {}needle and more", "word ".repeat(40));
        let matches = search_text(&plain("needle"), &line, usize::MAX);
        let found = &matches[0];
        assert_eq!(found.preview.as_ref(), "…word word word needle and more");
        assert_eq!(&found.preview[found.highlight.clone()], "needle");

        // A short line shows whole, without its indent.
        let matches = search_text(&plain("plan"), "    - the plan", usize::MAX);
        assert_eq!(matches[0].preview.as_ref(), "- the plan");
        assert_eq!(matches[0].highlight, 6..10);
    }

    #[test]
    fn previews_start_at_a_whole_english_word() {
        let line = "No match here except Plan in a heading.";
        let matches = search_text(&plain("plan"), line, usize::MAX);
        assert_eq!(
            matches[0].preview.as_ref(),
            "…here except Plan in a heading."
        );
        // One word longer than the lead is cut all the same.
        let line = format!("{}plan", "x".repeat(40));
        let matches = search_text(&plain("plan"), &line, usize::MAX);
        assert_eq!(
            matches[0].preview.chars().count(),
            1 + PREVIEW_LEAD_COLUMNS + 4
        );
    }

    #[test]
    fn previews_as_wide_a_lead_in_chinese_as_in_english() {
        // Each Han character takes two columns, so half as many lead.
        let line = format!("{}需要的计划", "这是一段很长的中文笔记".repeat(4));
        let matches = search_text(&plain("计划"), &line, usize::MAX);
        let found = &matches[0];
        assert_eq!(found.preview.as_ref(), "…的中文笔记需要的计划");
        assert_eq!(&found.preview[found.highlight.clone()], "计划");
        assert_eq!(
            found.preview.chars().count(),
            1 + PREVIEW_LEAD_COLUMNS / 2 + 2
        );
    }

    #[test]
    fn finds_chinese_text() {
        let text = "# 周计划\n\n本周的计划\n本周完成 Neon 的搜索。";
        assert_eq!(found(&plain("计划"), text), [(0, 5..11), (2, 9..15)]);
        // Mixed with English, case is ignored only in the English part.
        assert_eq!(found(&plain("neon 的"), text), [(3, 13..21)]);
        // Full-width letters, typed with a Chinese input method, fold their
        // case as ASCII ones do.
        assert_eq!(found(&plain("ｎｅｏｎ"), "ＮＥＯＮ").len(), 1);
        // 16 columns before the match fit exactly, so nothing is cut.
        let matches = search_text(&plain("搜索"), text, usize::MAX);
        assert_eq!(matches[0].preview.as_ref(), "本周完成 Neon 的搜索。");
        assert_eq!(&matches[0].preview[matches[0].highlight.clone()], "搜索");
    }

    #[test]
    fn matches_whole_words_in_chinese() {
        let whole = |text: &str| {
            query(
                text,
                SearchOptions {
                    whole_word: true,
                    ..Default::default()
                },
            )
        };
        // A Chinese character never joins its neighbors, so a word in a
        // run of them is whole, and so is English beside them.
        assert_eq!(found(&whole("计划"), "这个计划很好"), [(0, 6..12)]);
        assert_eq!(found(&whole("Rust"), "使用Rust编写"), [(0, 6..10)]);
        // English letters joined to the match still rule it out.
        assert!(found(&whole("Rust"), "使用Rusty编写").is_empty());
    }

    #[test]
    fn previews_cut_only_between_characters() {
        let line = format!("{}needle{}", "中".repeat(40), "文".repeat(200));
        let matches = search_text(&plain("needle"), &line, usize::MAX);
        let found = &matches[0];
        assert_eq!(&found.preview[found.highlight.clone()], "needle");
        assert!(found.preview.len() <= PREVIEW_MAX_BYTES + '…'.len_utf8());
    }

    #[test]
    fn locates_a_match_that_moved_on_its_line() {
        let query = plain("plan");
        assert_eq!(locate(&query, "a plan, then a plan", 15), Some(15..19));
        assert_eq!(locate(&query, "new text: plan", 0), Some(10..14));
        assert_eq!(locate(&query, "nothing here", 0), None);
    }

    #[test]
    fn searches_files_in_order_and_unsaved_text_in_place_of_the_file() {
        let root = tempfile::tempdir().unwrap();
        let root = root.path();
        fs::create_dir(root.join("notes")).unwrap();
        fs::write(root.join("notes/a.md"), "plan on disk").unwrap();
        fs::write(root.join("b.md"), "no match").unwrap();
        fs::write(root.join("c.md"), "first plan\nsecond plan").unwrap();
        fs::write(root.join("binary.md"), b"plan\0").unwrap();
        fs::write(root.join("latin1.md"), b"plan \xe9t\xe9").unwrap();
        fs::write(root.join("d.md"), "").unwrap();
        fs::create_dir(root.join("笔记")).unwrap();
        fs::write(root.join("笔记/周计划.md"), "下周的 plan").unwrap();
        let files = crate::folder::markdown_files(root).unwrap();
        let unsaved = HashMap::from([(root.join("d.md"), "an unsaved plan".to_string())]);

        let results = search_files(
            &plain("plan"),
            &files,
            &unsaved,
            usize::MAX,
            &AtomicBool::new(false),
        );

        let summary: Vec<_> = results
            .iter()
            .map(|file| {
                let name = file.path.strip_prefix(root).unwrap();
                (name.to_string_lossy().into_owned(), file.matches.len())
            })
            .collect();
        assert_eq!(
            summary,
            [
                ("notes/a.md".to_string(), 1),
                ("笔记/周计划.md".to_string(), 1),
                ("c.md".to_string(), 2),
                ("d.md".to_string(), 1),
            ]
        );
    }

    #[test]
    fn stops_once_the_first_files_hold_more_than_the_limit() {
        let root = tempfile::tempdir().unwrap();
        for name in ["a", "b", "c", "d", "e", "f", "g", "h"] {
            fs::write(root.path().join(format!("{name}.md")), "x ".repeat(5)).unwrap();
        }
        let files = crate::folder::markdown_files(root.path()).unwrap();

        let results = search_files(
            &plain("x"),
            &files,
            &HashMap::new(),
            7,
            &AtomicBool::new(false),
        );

        // The first files are all there, whole, so the first 7 matches are
        // the first 7 in the folder, and there are more than 7, so the
        // caller can tell the list was cut.
        let counts: Vec<_> = results.iter().map(|file| file.matches.len()).collect();
        assert_eq!(counts[..2], [5, 5]);
        assert_eq!(results[0].path, files[0]);
        assert_eq!(results[1].path, files[1]);
        assert!(counts.iter().sum::<usize>() > 7);
    }

    #[test]
    fn a_file_gives_one_match_past_the_limit_to_show_it_was_cut() {
        let root = tempfile::tempdir().unwrap();
        fs::write(root.path().join("a.md"), "x".repeat(12)).unwrap();
        let files = crate::folder::markdown_files(root.path()).unwrap();
        let results = search_files(
            &plain("x"),
            &files,
            &HashMap::new(),
            10,
            &AtomicBool::new(false),
        );
        assert_eq!(results[0].matches.len(), 11);
    }

    #[test]
    fn a_cancelled_search_reads_nothing_more() {
        let root = tempfile::tempdir().unwrap();
        fs::write(root.path().join("a.md"), "plan").unwrap();
        let files = crate::folder::markdown_files(root.path()).unwrap();
        let results = search_files(
            &plain("plan"),
            &files,
            &HashMap::new(),
            usize::MAX,
            &AtomicBool::new(true),
        );
        assert!(results.is_empty());
    }
}
