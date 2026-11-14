//! Random Markdown, built from the pieces rules care about, must never
//! panic, and fixing it must settle and keep its structure.

use lint::{Config, FixMode, fix, lint};

const PIECES: &[&str] = &[
    "# ",
    "## ",
    "#### ",
    "#",
    " ##",
    "\n",
    "\r\n",
    "\n\n",
    "\n\n\n",
    "  ",
    "   ",
    "\t",
    " ",
    "- ",
    "* ",
    "+ ",
    "1. ",
    "2) ",
    "10. ",
    "   - ",
    "> ",
    "> [!tip] ",
    ">",
    "```",
    "~~~",
    "```rs",
    "    ",
    "|",
    "| a |",
    "|---|",
    ":-:",
    "---",
    "***",
    "___",
    "*",
    "_",
    "**",
    "__",
    "`",
    "$",
    "$$",
    "[",
    "]",
    "(",
    ")",
    "[[",
    "]]",
    "<",
    ">",
    "<!--",
    "-->",
    "<div>",
    "</div>",
    "neonlint-disable",
    "neonlint-disable-next-line",
    "...",
    "....",
    "中文",
    "abc",
    "123",
    "é",
    "😀",
    "\\",
    "[^1]",
    "[^1]: ",
    "[ref]: http://x",
    "<http://a.b>",
    "word",
    "a_b",
    "x*y*z",
    "=====",
    "| a | b |\n|---|:-:|\n",
    "|  |",
    "| 中文 |",
    "|---|---|",
    " | ",
    "- [ ] ",
    "- [x] ",
    "\u{3000}",
    "，",
    "。",
];

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
}

fn document(rng: &mut Rng) -> String {
    let len = rng.next() % 60;
    (0..len)
        .map(|_| PIECES[(rng.next() % PIECES.len() as u64) as usize])
        .collect()
}

#[test]
fn random_documents_never_panic_and_fixes_settle() {
    let everything = Config::from_toml(
        "[rules]\nblank-lines-around-blockquotes = true\nblank-lines-around-rules = true\nsingle-h1 = true\nheading-punctuation = true\nmultiple-spaces = true\nunordered-list-marker = true\nordered-list-numbering = true\nemphasis-style = true\nstrong-style = true\ncode-fence-style = true\nfence-language = true\ncjk-spacing = true\nproper-ellipsis = true\ntable-alignment = true\n[[custom]]\nname = \"x\"\npattern = 'a(b)'\nreplacement = '${1}a'\n",
    );
    let iterations: u64 = std::env::var("LINT_FUZZ_ITERATIONS")
        .ok()
        .and_then(|n| n.parse().ok())
        .unwrap_or(3000);
    let seed = std::env::var("LINT_FUZZ_SEED")
        .ok()
        .and_then(|n| n.parse().ok())
        .unwrap_or(0x9E37_79B9_7F4A_7C15);
    let mut rng = Rng(seed);
    for _ in 0..iterations {
        let source = document(&mut rng);
        for config in [&Config::default(), &everything] {
            for finding in lint(&source, config) {
                assert!(finding.range.end <= source.len());
                assert!(source.is_char_boundary(finding.range.start), "{source:?}");
                assert!(source.is_char_boundary(finding.range.end), "{source:?}");
                // A fix that changes nothing would never settle.
                if !finding.fix.is_empty() {
                    let mut text = source.clone();
                    for pair in finding.fix.windows(2) {
                        assert!(
                            pair[0].range.end <= pair[1].range.start,
                            "{} has overlapping edits {:?} in {source:?}",
                            finding.rule,
                            finding.fix
                        );
                    }
                    for edit in finding.fix.iter().rev() {
                        assert!(
                            source.is_char_boundary(edit.range.start)
                                && source.is_char_boundary(edit.range.end),
                            "{} edits {:?} inside a character of {source:?}",
                            finding.rule,
                            edit.range
                        );
                        text.replace_range(edit.range.clone(), &edit.text);
                    }
                    assert_ne!(text, source, "{} fixes nothing in {source:?}", finding.rule);
                }
            }
            for mode in [FixMode::All, FixMode::OnSave] {
                let Some(first) = fix(&source, config, mode) else {
                    continue;
                };
                if let Some(second) = fix(&first.text, config, mode) {
                    assert_eq!(
                        second.text, first.text,
                        "fixing again changed {source:?}\nfirst:  {:?}",
                        first.text
                    );
                }
            }
        }
    }
}
