# Changelog

All notable changes to Neon are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/).

How to use it:

- Add each user-visible change under **Unreleased** as you make it, in the
  group it belongs to. Leave out groups with nothing in them.
- Write for people using Neon: say what changed for them, not how the code
  changed. One line per change, starting with a verb or the feature's name.
  Keep each entry on a single line: GitHub shows line breaks in release
  notes as they are.
- To release, rename **Unreleased** to `[x.y.z] - YYYY-MM-DD`, start a new
  empty **Unreleased** above it, set the same version in `Cargo.toml`, then
  push the tag `vx.y.z`. The release workflow publishes that section as the
  GitHub release notes and refuses to release a version that has no section.

Groups, in this order:

- **Added** for new features.
- **Changed** for changes to existing behavior.
- **Deprecated** for features that will be removed.
- **Removed** for features that are gone.
- **Fixed** for bug fixes.
- **Performance** for speed and memory improvements.
- **Security** for vulnerabilities.

## [Unreleased]

### Added

- Open folders and navigate nested Markdown files in a resizable sidebar, with folder refresh and keyboard navigation.
- Create Markdown files in the selected folder and save from the workspace toolbar.
- Choose installed fonts and text sizes independently for the editor and preview through Typography settings.
- Show the percentage of the file viewed in the current panel and window FPS in the status bar.

### Changed

- Simplify the workspace with a compact title bar, sidebar file actions, denser navigation, and labeled document panes.
- Refine Typography settings with paired font and size controls and live samples.

- Give the editor and preview panels matching rounded corners and inset borders in every layout.
- Remove the title bar divider above the rounded panels.
- Show the split divider only while hovering or resizing the panels.
- Update the preview at most every 200 ms while typing; a single edit still shows at once.

### Performance

- Show typed characters a full display refresh sooner by presenting the frame right after each keystroke instead of waiting for the next refresh.
- Keep syntax highlighting off the keystroke path: long documents reparse in the background while typing stays under 2 ms per key.
- Redraw only the editor while typing; the preview, its scrollbar and the sidebar reuse their last frame.

## [0.1.0] - 2026-10-16

First release.

### Added

- Markdown editor with syntax highlighting, built on GPUI; handles files of tens of thousands of lines.
- Live preview: headings, emphasis, links, task lists, nested and numbered lists, block quotes, code blocks, tables with column alignment, images, footnotes and YAML front matter.
- Editor, Split and Preview layouts, switched with ⌘1, ⌘2 and ⌘3; in Split, the preview follows the editor's cursor.
- Open (⌘O), Save (⌘S) and Save As (⌘⇧S), with a confirmation before discarding unsaved changes.
- Open Markdown files from Finder; a file that is already open comes to the front instead of opening twice.
- Light and dark appearance that follows the system, with a manual toggle in the View menu.
- Status bar with the cursor position and word count.

### Performance

- The preview parses on a background thread and re-lays out only the blocks an edit changed, so typing stays responsive in large documents.
