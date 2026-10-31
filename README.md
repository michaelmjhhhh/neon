<div align="center">

<img src="assets/icon/neon.png" alt="Neon icon" width="160" height="160">

# Neon

**A fast, native Markdown editor for macOS.**

Write on the left, read on the right, or go full screen with nothing but the text.

[![Release](https://img.shields.io/github/v/release/michaelmjhhhh/neon?style=for-the-badge&color=FF5A1F&label=release)](https://github.com/michaelmjhhhh/neon/releases/latest)
[![macOS 11+](https://img.shields.io/badge/macOS_11+-Apple_Silicon_%7C_Intel-1C1C1E?style=for-the-badge&logo=apple&logoColor=white)](#install)
[![Keystroke to frame](https://img.shields.io/badge/keystroke_→_frame-1.6_ms-16A34A?style=for-the-badge)](#performance)
<br>
[![Rust](https://img.shields.io/badge/Rust-1.99-CE422B?style=for-the-badge&logo=rust&logoColor=white)](rust-toolchain.toml)
[![GPUI](https://img.shields.io/badge/built_with-GPUI-084CCF?style=for-the-badge)](https://github.com/zed-industries/zed/tree/main/crates/gpui)
[![License](https://img.shields.io/badge/license-Apache--2.0-0EA5E9?style=for-the-badge)](Cargo.toml)

[**Download**](https://github.com/michaelmjhhhh/neon/releases/latest) · [Shortcuts](#keyboard-shortcuts) · [Build](#build-from-source) · [Changelog](CHANGELOG.md)

</div>

---

Neon is built on [GPUI](https://github.com/zed-industries/zed/tree/main/crates/gpui), the GPU-accelerated UI framework behind the Zed editor. It draws with Metal, parses off the main thread, and stays responsive in files tens of thousands of lines long. The Apple Silicon build is about 16 MB.

## Features

**Writing**

- **Markdown highlighting** in the editor, with line numbers you can hide.
- **Lists that continue themselves.** Enter starts the next bullet, number or task; Tab and Shift+Tab indent and outdent the item.
- **Slash commands.** Type `/` on an empty line to insert a list, task list, code block, link, image or table. Add a size or language: `/table 4x5`, `/bullet 5`, `/code rust`.
- **Find and replace** in a compact bar at the editor's corner.

**Reading**

- **Live preview** in Charter, a book face, of headings, links, nested and numbered lists, task lists, block quotes, code, tables with alignment, images and footnotes.
- **Obsidian-style notes:** callouts (`> [!tip]`), `==highlights==`, front matter as properties, and `[[wikilinks]]` that open the linked note in Neon.
- **Three layouts:** Editor, Split and Preview. In Split, the preview follows the cursor; in Editor, the text sits in a centered column of readable width.
- **Zen Mode.** The document alone, full screen, in a centered column. The paragraph you are writing stays bright while the rest fades, and the line you type on stays at the middle of the screen.

**Files**

- **Tabs**, each with its own edits, undo history and scroll position.
- **Folder sidebar** for nested Markdown files, with new file and new folder.
- **Search the folder** from the sidebar, with match case, whole word and regular expressions. Unsaved edits are searched as they read, in English and Chinese.
- **Git** for a folder in a Git repository: see what changed since the last commit, word by word, then stage, commit and push from the sidebar, and browse the history as a branch graph.
- **Picks up where you left off:** Neon reopens the last folder and file at launch.
- **Export** to HTML, PDF or Word in the preview's font, with local images embedded.

**Look**

- Light and dark themes that follow the system, or a fixed choice in Settings.
- Separate fonts and sizes for the editor and the preview, remembered between launches.

## Install

1. Download `Neon-<version>-macos.dmg` from the [latest release](https://github.com/michaelmjhhhh/neon/releases/latest).
2. Open it and drag **Neon** to Applications.

Neon runs natively on Apple Silicon and Intel, macOS 11 or later. It is signed with an Apple Developer ID and notarized, so macOS opens it without a warning.

Neon updates itself. On the second launch it asks whether to check for updates automatically; when one is out, it shows what changed and installs it on request. **Neon › Check for Updates…** checks right away. Updates are signed, and Neon refuses one that is not.

<details>
<summary>Verify a download</summary>

Each release lists `SHA256SUMS` and a build provenance attestation. To check that a file was built by this repository's release workflow:

```sh
gh attestation verify Neon-<version>-macos.dmg --repo michaelmjhhhh/neon
```

</details>

## Keyboard shortcuts

| Files | | View | |
| --- | --- | --- | --- |
| New file | <kbd>⌘</kbd> <kbd>N</kbd> | Editor / Split / Preview | <kbd>⌘</kbd> <kbd>1</kbd> <kbd>2</kbd> <kbd>3</kbd> |
| New tab | <kbd>⌘</kbd> <kbd>T</kbd> | Zen Mode | <kbd>⌃</kbd> <kbd>⌘</kbd> <kbd>Z</kbd> |
| New folder | <kbd>⌘</kbd> <kbd>⇧</kbd> <kbd>N</kbd> | Toggle sidebar | <kbd>⌘</kbd> <kbd>⇧</kbd> <kbd>B</kbd> |
| Open file | <kbd>⌘</kbd> <kbd>O</kbd> | Toggle line numbers | <kbd>⌘</kbd> <kbd>⇧</kbd> <kbd>L</kbd> |
| Open folder | <kbd>⌘</kbd> <kbd>⇧</kbd> <kbd>O</kbd> | Settings | <kbd>⌘</kbd> <kbd>,</kbd> |
| Save | <kbd>⌘</kbd> <kbd>S</kbd> | Bigger / smaller / default text | <kbd>⌘</kbd> <kbd>=</kbd> / <kbd>⌘</kbd> <kbd>-</kbd> / <kbd>⌘</kbd> <kbd>0</kbd> |
| Save As | <kbd>⌘</kbd> <kbd>⇧</kbd> <kbd>S</kbd> | **Editing** | |
| Next / previous tab | <kbd>⌃</kbd> <kbd>Tab</kbd> / <kbd>⌃</kbd> <kbd>⇧</kbd> <kbd>Tab</kbd> | Bold / Italic | <kbd>⌘</kbd> <kbd>B</kbd> / <kbd>⌘</kbd> <kbd>I</kbd> |
| Close tab | <kbd>⌘</kbd> <kbd>W</kbd> | Find | <kbd>⌘</kbd> <kbd>F</kbd> |
| Close window | <kbd>⌘</kbd> <kbd>⇧</kbd> <kbd>W</kbd> | Find and replace | <kbd>⌥</kbd> <kbd>⌘</kbd> <kbd>F</kbd> |
| Files / Changes in sidebar | <kbd>⌘</kbd> <kbd>⇧</kbd> <kbd>E</kbd> / <kbd>⌃</kbd> <kbd>⇧</kbd> <kbd>G</kbd> | Indent / outdent list item | <kbd>Tab</kbd> / <kbd>⇧</kbd> <kbd>Tab</kbd> |
| Search in folder | <kbd>⌘</kbd> <kbd>⇧</kbd> <kbd>F</kbd> | | |
| Commit, in the message | <kbd>⌘</kbd> <kbd>↩</kbd> | | |

<kbd>⌘</kbd> <kbd>}</kbd> and <kbd>⌘</kbd> <kbd>{</kbd> also switch tabs. In the search field, <kbd>↩</kbd> or <kbd>↓</kbd> opens the next match and <kbd>⇧</kbd> <kbd>↩</kbd> or <kbd>↑</kbd> the previous one. In the preview, <kbd>Space</kbd> and <kbd>⇧</kbd> <kbd>Space</kbd> page down and up, and <kbd>⌘</kbd> <kbd>↑</kbd> <kbd>↓</kbd> jump to the top and bottom.

## Performance

Each keystroke is drawn and submitted as soon as it is handled, instead of waiting for the next display refresh. Syntax highlighting reparses in the background, and only the editor redraws while you type.

Measured in an 8,600-line document on a 60 Hz display:

| | Mean | p99 |
| --- | ---: | ---: |
| Keystroke → frame submitted | 1.6 ms | 5.2 ms |
| Keystroke → first refresh it can appear on | 10.5 ms | 17.9 ms |

To reproduce, keep the Neon window in front and run:

```sh
script/latency-bench my-run 3    # JSON reports go to output/latency/
```

Memory use is covered by a separate [memory benchmark](docs/memory-benchmark.md).

## Build from source

Requires macOS, Xcode with the Metal toolchain, and [rustup](https://rustup.rs). The Rust version is pinned in `rust-toolchain.toml`.

```sh
cargo run --release              # build and launch
cargo test --workspace           # run the tests
script/bundle-mac --native       # package dist/Neon.app, .dmg and .zip for this Mac
script/bundle-mac                # universal binary for Apple Silicon and Intel
```

Builds from source have no updater: `cargo run` leaves Sparkle out, and so does `script/bundle-mac` unless `sparkle_public_key` in it is set. `script/appcast` writes the update feed for a release.

If the Metal compiler is missing, run `xcodebuild -downloadComponent MetalToolchain`.

| Crate | Purpose |
| --- | --- |
| `crates/neon` | App entry point, menus and windows |
| `crates/workspace` | Document window: editor, tabs, sidebar, status bar, file handling |
| `crates/markdown_preview` | Virtualized, incrementally updated preview |
| `crates/markdown` | Markdown parsing into blocks, with source ranges |
| `crates/export` | HTML, PDF and Word export |

## License

[Apache-2.0](Cargo.toml).
