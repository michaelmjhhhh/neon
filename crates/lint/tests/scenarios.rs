//! Fake notes and folders on disk, in temporary directories, covering the
//! situations a vault runs into: no configuration, nested and broken
//! configuration, odd file names, odd line endings, odd sizes.

use std::path::{Path, PathBuf};

use lint::{CONFIG_FILE_NAME, Config, FixMode, Resolver, fix, lint};

fn write(dir: &Path, path: &str, text: &str) -> PathBuf {
    let path = dir.join(path);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, text).unwrap();
    path
}

fn rule_names(config: &Config, source: &str) -> Vec<String> {
    let mut names: Vec<String> = lint(source, config)
        .into_iter()
        .map(|finding| finding.rule.to_string())
        .collect();
    names.sort();
    names.dedup();
    names
}

/// Lint and fix the note at `path` the way the editor does: read it,
/// resolve its configuration in `folder`, fix it, and check the fix
/// settles.
fn check_note(resolver: &mut Resolver, path: &Path, folder: Option<&Path>) -> Option<String> {
    let source = std::fs::read_to_string(path).unwrap();
    let config = resolver.resolve(Some(path), folder);
    lint(&source, &config);
    let fixed = fix(&source, &config, FixMode::All)?;
    let again = fix(&fixed.text, &config, FixMode::All);
    assert!(
        again.is_none_or(|again| again.text == fixed.text),
        "fixing {} twice differs",
        path.display()
    );
    Some(fixed.text)
}

#[test]
fn a_vault_with_nested_configuration() {
    let dir = tempfile::tempdir().unwrap();
    let vault = dir.path().join("vault");
    write(
        &vault,
        CONFIG_FILE_NAME,
        "ignore = [\"Templates\", \"*.excalidraw.md\"]\n[rules]\ncjk-spacing = true\n",
    );
    write(
        &vault,
        &format!("Work/{CONFIG_FILE_NAME}"),
        "[rules]\ntrailing-spaces = false\n[[overrides]]\nfiles = [\"Archive/**\"]\nignore = true\n",
    );
    write(
        &vault,
        &format!("Work/Team/{CONFIG_FILE_NAME}"),
        "root = true\n[rules]\nproper-ellipsis = true\n",
    );
    // One trailing space: two before another line would be a line break.
    let messy = "# 标题A\ntext \nWait...\n";
    let notes = [
        "top.md",
        "Work/plan.md",
        "Work/Archive/old.md",
        "Work/Team/standup.md",
        "Templates/daily.md",
        "Drawings/a.excalidraw.md",
        "Deep/er/and/deeper/note.md",
        "笔记 [1] (copy).md",
        "with space/My Note.md",
    ];
    for note in notes {
        write(&vault, note, messy);
    }
    let mut resolver = Resolver::new(None);
    let mut names = |note: &str| {
        let config = resolver.resolve(Some(&vault.join(note)), Some(&vault));
        rule_names(&config, messy)
    };
    let all = [
        "blank-lines-around-headings",
        "cjk-spacing",
        "trailing-spaces",
    ];
    assert_eq!(names("top.md"), all);
    assert_eq!(names("Deep/er/and/deeper/note.md"), all);
    assert_eq!(names("笔记 [1] (copy).md"), all);
    assert_eq!(names("with space/My Note.md"), all);
    assert_eq!(
        names("Work/plan.md"),
        ["blank-lines-around-headings", "cjk-spacing"]
    );
    assert!(names("Work/Archive/old.md").is_empty());
    assert!(names("Templates/daily.md").is_empty());
    assert!(names("Drawings/a.excalidraw.md").is_empty());
    // `root = true`: the vault's cjk-spacing and Work's trailing-spaces
    // stop above the team folder.
    assert_eq!(
        names("Work/Team/standup.md"),
        [
            "blank-lines-around-headings",
            "proper-ellipsis",
            "trailing-spaces"
        ]
    );
    // An untitled note takes the open folder's rules.
    assert_eq!(
        rule_names(&resolver.resolve(None, Some(&vault)), messy),
        all
    );
}

#[test]
fn configuration_files_that_are_broken_or_odd() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let cases: &[(&str, &str, bool)] = &[
        // (folder, configuration text, whether an error is reported)
        ("empty", "", false),
        ("comments", "# nothing yet\n\n", false),
        ("bom", "\u{feff}[rules]\ncjk-spacing = true\n", false),
        ("syntax", "[rules\n", true),
        ("unknown-key", "colour = \"red\"\n", true),
        ("unknown-rule", "[rules]\nmd999 = true\n", true),
        ("wrong-type", "[rules]\ntrailing-spaces = 3\n", true),
        ("bad-glob", "ignore = [\"[abc\"]\n", true),
        (
            "bad-regex",
            "[[custom]]\nname = \"x\"\npattern = \"(?<=a)b\"\n",
            true,
        ),
        (
            "huge-regex",
            "[[custom]]\nname = \"x\"\npattern = '\\w{2000}'\n",
            true,
        ),
        (
            "nameless",
            "[[custom]]\nname = \" \"\npattern = \"a\"\n",
            true,
        ),
        ("crlf", "[rules]\r\ncjk-spacing = true\r\n", false),
    ];
    let mut resolver = Resolver::new(None);
    for (folder, text, has_error) in cases {
        let path = write(root, &format!("{folder}/{CONFIG_FILE_NAME}"), text);
        let note = write(root, &format!("{folder}/note.md"), "a  \nb\n\n\n\nc\n");
        let config = resolver.resolve(Some(&note), Some(&root.join(folder)));
        assert_eq!(
            !config.errors().is_empty(),
            *has_error,
            "{folder}: {:?}",
            config.errors()
        );
        // Whatever the file says, the note is still linted with the rest.
        assert!(
            !lint("a  \n", &config).is_empty() || *folder == "wrong-type",
            "{folder}"
        );
        if *has_error {
            assert!(
                config.errors()[0].contains(&*path.to_string_lossy())
                    || config.errors()[0].starts_with("trailing-spaces"),
                "{folder}: errors name the file: {:?}",
                config.errors()
            );
        }
    }
    // The BOM file's rules applied.
    let bom = resolver.resolve(Some(&root.join("bom/note.md")), Some(&root.join("bom")));
    assert!(rule_names(&bom, "中文abc\n").contains(&"cjk-spacing".to_string()));
}

#[test]
fn configuration_that_is_not_a_readable_file() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    // A folder named like the configuration file is not one.
    std::fs::create_dir_all(root.join("dir").join(CONFIG_FILE_NAME)).unwrap();
    let note = write(root, "dir/note.md", "a  \n");
    let mut resolver = Resolver::new(None);
    let config = resolver.resolve(Some(&note), Some(&root.join("dir")));
    assert!(config.errors().is_empty());
    assert_eq!(rule_names(&config, "a  \n"), ["trailing-spaces"]);

    // Unreadable: reported, then skipped.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let path = write(root, &format!("locked/{CONFIG_FILE_NAME}"), "[rules]\n");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000)).unwrap();
        let note = write(root, "locked/note.md", "a  \n");
        let config = resolver.resolve(Some(&note), Some(&root.join("locked")));
        // Root can read anything; the check only holds for other users.
        if std::fs::read(&path).is_err() {
            assert_eq!(config.errors().len(), 1, "{:?}", config.errors());
        }
        assert_eq!(rule_names(&config, "a  \n"), ["trailing-spaces"]);
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
    }
}

#[test]
fn configuration_that_changes_or_goes_away() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let note = write(root, "note.md", "x\n");
    let mut resolver = Resolver::new(None);
    let resolve = |resolver: &mut Resolver| {
        rule_names(&resolver.resolve(Some(&note), Some(root)), "中文abc...\n")
    };
    assert!(resolve(&mut resolver).is_empty());
    write(root, CONFIG_FILE_NAME, "[rules]\ncjk-spacing = true\n");
    assert_eq!(resolve(&mut resolver), ["cjk-spacing"]);
    // Same length, other rule: still seen, by modification time.
    std::thread::sleep(std::time::Duration::from_millis(20));
    write(root, CONFIG_FILE_NAME, "[rules]\nproper-ellipsis=true\n");
    assert_eq!(resolve(&mut resolver), ["proper-ellipsis"]);
    std::fs::remove_file(root.join(CONFIG_FILE_NAME)).unwrap();
    assert!(resolve(&mut resolver).is_empty());
}

#[test]
fn notes_outside_the_open_folder() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root, CONFIG_FILE_NAME, "[rules]\ncjk-spacing = true\n");
    // A repository below: the search for a note in it stops at its root.
    std::fs::create_dir_all(root.join("repo/.git")).unwrap();
    let inside = write(root, "repo/docs/a.md", "中文abc\n");
    let other_folder = root.join("elsewhere");
    std::fs::create_dir_all(&other_folder).unwrap();
    let mut resolver = Resolver::new(None);
    let config = resolver.resolve(Some(&inside), Some(&other_folder));
    assert!(rule_names(&config, "中文abc\n").is_empty());
    // Without a repository the search goes up and finds the configuration.
    let loose = write(root, "loose/b.md", "中文abc\n");
    let config = resolver.resolve(Some(&loose), None);
    assert_eq!(rule_names(&config, "中文abc\n"), ["cjk-spacing"]);
    // Neither a path nor a folder: the defaults.
    let config = resolver.resolve(None, None);
    assert!(rule_names(&config, "中文abc\n").is_empty());
}

#[test]
fn notes_of_every_shape() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(
        root,
        CONFIG_FILE_NAME,
        "[rules]\ncjk-spacing = true\nproper-ellipsis = true\ntable-alignment = true\nmultiple-spaces = true\nordered-list-numbering = true\nemphasis-style = true\n",
    );
    let long_line = format!("{}\n", "word  中文abc... ".repeat(20_000));
    let many_lines = "line with spaces  \n\n\n\n## heading\ntext\n".repeat(5_000);
    let deep_quote = format!("{}deep\n", "> ".repeat(500));
    let deep_list: String = (0..200)
        .map(|depth| format!("{}- item\n", "  ".repeat(depth)))
        .collect();
    let notes: &[(&str, &str)] = &[
        ("empty.md", ""),
        ("blank.md", "\n\n   \n\t\n"),
        ("one-char.md", "a"),
        ("bom.md", "\u{feff}# Title\ntext  \n"),
        ("crlf.md", "# A\r\ntext  \r\n\r\n\r\n\r\nend"),
        ("mixed-endings.md", "# A\ntext  \r\nmore\n\n\n\r\nend\r\n"),
        ("cr-only.md", "# A\rtext  \rmore\r\r\r\rend"),
        ("long-line.md", &long_line),
        ("many-lines.md", &many_lines),
        ("deep-quote.md", &deep_quote),
        ("deep-list.md", &deep_list),
        (
            "obsidian.md",
            "---\ntags: [a]\n---\n# 笔记A\n%% comment... %%\n![[image.png]] [[Note#Head|别名A]] #tag ^block-id\n> [!warning] 注意A\n> - item  \n> | a | b |\n> |---|---|\n\n$$\nx...y\n$$\n- [ ] task...\n- [x] done\n[^1]\n\n[^1]: Note...\n",
        ),
        (
            "html-heavy.md",
            "<details>\n<summary>A...</summary>\n\ntext  \n\n</details>\n<!-- comment...  -->\n<br>  \n",
        ),
        ("only-code.md", "```\ncode...   \n\n\n\n"),
        ("only-front-matter.md", "---\na: 1  \n---"),
        ("nul-and-controls.md", "a\u{0}b  \n\u{7}c\u{1b}[0m...\n"),
        (
            "emoji.md",
            "# 😀 Title ##\n👨‍👩‍👧 family中文abc...\n| 😀 | 中文 |\n|---|---|\n| a | b |\n",
        ),
        ("rtl.md", "مرحبا  \nשלום...\n"),
        ("numbers.md", "1. a\n1. b\n99999999999999999999. huge\n"),
    ];
    let mut resolver = Resolver::new(None);
    for (name, text) in notes {
        let path = write(root, name, text);
        let fixed = check_note(&mut resolver, &path, Some(root));
        if let Some(fixed) = &fixed {
            // Nothing outside the note's own characters appears, and code
            // and front matter keep their bytes.
            for kept in ["code...   \n\n\n\n", "a: 1  ", "x...y", "comment...  "] {
                if text.contains(kept) {
                    assert!(fixed.contains(kept), "{name} lost {kept:?}");
                }
            }
        }
        match *name {
            "empty.md" | "only-code.md" => {
                assert_eq!(fixed.as_deref().unwrap_or(text), *text, "{name}")
            }
            "only-front-matter.md" => assert_eq!(fixed.unwrap(), "---\na: 1  \n---\n"),
            "bom.md" => assert!(
                fixed.unwrap().starts_with('\u{feff}'),
                "{name} lost its BOM"
            ),
            "crlf.md" => assert_eq!(fixed.unwrap(), "# A\r\n\r\ntext\r\n\r\nend\r\n"),
            "mixed-endings.md" => {
                assert_eq!(fixed.unwrap(), "# A\n\ntext  \r\nmore\n\nend\r\n")
            }
            "cr-only.md" => {
                // Lines ended by a lone CR are rare; the note is left
                // whole rather than split wrongly.
                let fixed = fixed.unwrap_or_else(|| text.to_string());
                assert!(fixed.starts_with("# A\rtext"), "{fixed:?}");
            }
            _ => {}
        }
    }
}
