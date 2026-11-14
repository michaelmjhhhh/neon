//! Times linting and fixing a long generated note:
//! `cargo run --release -p lint --example bench`.

use std::time::{Duration, Instant};

use lint::{Config, FixMode, fix, lint};

/// About 10,000 lines of mixed prose, lists, code, tables and Chinese text,
/// with a problem every few lines.
fn document(sections: usize) -> String {
    let mut text = String::new();
    for ix in 0..sections {
        text.push_str(&format!("## Section {ix} ##\n"));
        text.push_str("Some prose with  two spaces and a trailing space \n");
        text.push_str("在Neon里写Markdown很快，速度是3倍... 中文English混排。\n\n\n\n");
        text.push_str("- item one\n* item two\n1. first\n1. second\n\n");
        text.push_str("```rust\nfn main() {  \n    println!(\"...\");\n}\n```\n");
        text.push_str("|a|b|\n|-|-|\n|`x`|y|\n\n");
        text.push_str("> [!note] A callout\n> with *emphasis* and __bold__ and [a link](https://example.com/a...b).\n\n");
    }
    text
}

fn time(label: &str, runs: u32, mut f: impl FnMut()) {
    f();
    let start = Instant::now();
    for _ in 0..runs {
        f();
    }
    let each: Duration = start.elapsed() / runs;
    println!("{label:<36} {:>8.2} ms", each.as_secs_f64() * 1000.);
}

fn main() {
    let source = document(500);
    println!(
        "{} lines, {} KB",
        source.lines().count(),
        source.len() / 1024
    );
    let defaults = Config::default();
    let everything = Config::from_toml(
        "[rules]\nmultiple-spaces = true\nunordered-list-marker = true\nordered-list-numbering = true\nemphasis-style = true\nstrong-style = true\ncjk-spacing = true\nproper-ellipsis = true\ntable-alignment = true\nfence-language = true\nheading-punctuation = true\n",
    );
    let clean = fix(&source, &defaults, FixMode::All).unwrap().text;
    println!(
        "findings: defaults {}, everything {}",
        lint(&source, &defaults).len(),
        lint(&source, &everything).len()
    );
    time("lint, defaults", 20, || drop(lint(&source, &defaults)));
    time("lint, every rule", 20, || drop(lint(&source, &everything)));
    time("lint, clean note, defaults", 20, || {
        drop(lint(&clean, &defaults))
    });
    time("fix on save, defaults", 10, || {
        drop(fix(&source, &defaults, FixMode::OnSave))
    });
    time("fix all, every rule", 10, || {
        drop(fix(&source, &everything, FixMode::All))
    });
    time("fix, clean note, defaults", 20, || {
        drop(fix(&clean, &defaults, FixMode::OnSave))
    });
    let small = document(5);
    time("lint, 100-line note, defaults", 200, || {
        drop(lint(&small, &defaults))
    });
}
