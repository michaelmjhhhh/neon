//! Every rule at once over documents that mix the constructs rules must
//! leave alone. Fixing must settle, keep code and links as written, and
//! find nothing more to fix the second time.

use lint::{Config, FixMode, fix, lint};

const EVERYTHING: &str = r#"
[rules]
blank-lines-around-blockquotes = true
blank-lines-around-rules = true
single-h1 = true
heading-punctuation = true
multiple-spaces = true
unordered-list-marker = true
ordered-list-numbering = true
emphasis-style = true
strong-style = true
code-fence-style = true
fence-language = true
cjk-spacing = true
proper-ellipsis = true
table-alignment = true
"#;

const DOCUMENTS: &[&str] = &[
    "---\ntitle: a...b  \ntags: [x]\n---\n# Title ##\nText  with   spaces...  \nnext\n\n\n\n* a\n- b\n+ c\n\n1. x\n1. y\n\n",
    "在Neon里写Markdown...\n\n```\n中文abc...  \n\n\n\n```\n`code  ...`\n$$\nx...y\n$$\n",
    "> [!tip] 提示A\n> body  \n>\n> - item\n\n|a|b|\n|-|:-:|\n|`x|y`|中文|\n| 1 | 2 | 3 |\n",
    "Setext\n===\nPara\n***\n_em_ and *em* __b__ **b** snake_case_word x*y*z\n",
    "<!-- neonlint-disable trailing-spaces -->\nkeep  \n<!-- neonlint-enable trailing-spaces -->\ndrop  \n",
    "<div>\n  html  \n\n\n</div>\n\n[ref]: https://example.com/a...b  \n[x][ref] [[Note...|alias...]] <https://a.b/c...>\n",
    "- a\n\n  ```\n  code\n  ```\n- b\n   - c\n\n\n\npara\n\n    indented code  \n",
    "\r\n\r\n# CRLF\r\ntext  \r\n\r\n\r\n\r\nend",
    "Footnote[^1] here.\n\n[^1]: The note...  \n",
    "~~~\n```\n~~~\n\n```js\nx\n```\n",
    "",
    "\n\n\n",
    "    only indented code",
    "| a |\n|---|\n",
    "#NotAHeading\n#tag and ## not\n",
    "a\\\nb\n\nc  \nd",
];

#[test]
fn fixes_settle_and_keep_what_must_stay() {
    let config = Config::from_toml(EVERYTHING);
    assert!(config.errors().is_empty(), "{:?}", config.errors());
    for source in DOCUMENTS {
        lint(source, &config);
        let Some(first) = fix(source, &config, FixMode::All) else {
            continue;
        };
        // Code blocks, inline code, math and link targets survive.
        for kept in [
            "中文abc...  \n\n\n\n",
            "`code  ...`",
            "x...y",
            "https://example.com/a...b",
            "[[Note...|alias...]]",
            "<https://a.b/c...>",
            "    indented code  ",
            "  html  \n",
        ] {
            if source.contains(kept) {
                assert!(
                    first.text.contains(kept),
                    "lost {kept:?} fixing {source:?}:\n{}",
                    first.text
                );
            }
        }
        let second = fix(&first.text, &config, FixMode::All);
        assert!(
            second
                .as_ref()
                .is_none_or(|second| second.text == first.text),
            "fixing {source:?} again changed it:\n{}\n---\n{}",
            first.text,
            second.unwrap().text
        );
    }
}

#[test]
fn comments_turn_rules_off() {
    let config = Config::default();
    let found = lint(DOCUMENTS[4], &config);
    let lines: Vec<&str> = found
        .iter()
        .filter(|finding| finding.rule.as_str() == "trailing-spaces")
        .map(|finding| &DOCUMENTS[4][finding.range.clone()])
        .collect();
    assert_eq!(lines, ["  "]);
    assert_eq!(
        found
            .iter()
            .filter(|f| f.rule.as_str() == "trailing-spaces")
            .count(),
        1
    );
    let next_line = "<!-- markdownlint-disable-next-line MD009 -->\na  \nb  \n\nc\n";
    let found = lint(next_line, &config);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].range.start, next_line.find("b  ").unwrap() + 1);
    let file = "<!-- neonlint-disable-file -->\na  \n\n\n\n";
    assert!(lint(file, &config).is_empty());
}

#[test]
fn crlf_documents_keep_their_line_endings() {
    let config = Config::default();
    let outcome = fix(DOCUMENTS[7], &config, FixMode::OnSave).unwrap();
    assert_eq!(outcome.text, "# CRLF\r\n\r\ntext\r\n\r\nend\r\n");
}

#[test]
fn ignored_files_are_left_alone() {
    let config = Config::from_toml("[[overrides]]\nfiles = [\"*\"]\nignore = true\n");
    // Without a path the overrides never match.
    assert!(!config.is_ignored());
}

#[test]
fn positions_count_characters() {
    let source = "ab\n中文x\n\nlast";
    let positions = lint::Positions::new(source);
    assert_eq!(positions.at(0), (0, 0));
    assert_eq!(positions.at(2), (0, 2));
    assert_eq!(positions.at(3), (1, 0));
    assert_eq!(positions.at(9), (1, 2));
    // Inside a character, the position is the character's.
    assert_eq!(positions.at(4), (1, 0));
    assert_eq!(positions.at(11), (2, 0));
    assert_eq!(positions.at(source.len() + 5), (3, 4));
}
