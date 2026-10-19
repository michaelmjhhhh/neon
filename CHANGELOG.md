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

- Enter on a list item starts the next one: `- ` bullets keep their marker and indent, numbered items count up, and tasks continue unchecked. Enter on an empty item ends the list; Shift+Enter inserts a plain newline.

### Fixed

- Saving with Cmd+S no longer reopens a hidden sidebar. Cmd+B still toggles it.

## [0.4.0] - 2026-10-17

### Added

- Install instructions for allowing the first launch with one Terminal command instead of System Settings.
- Tab in the slash command menu completes the highlighted command, such as `/table `, so you can add an argument before inserting it.
- Export to HTML, PDF or Word from File > Export, set in the preview's font and size, with code in the monospace font. Local images are embedded, Word documents use Word's heading and list styles, and a failed export never leaves a partial file or damages an existing one.

### Changed

- Lighten the dark theme to match Zed's One Dark: blue-gray surfaces instead of near-black, with comments and link addresses brightened to stay readable.

## [0.3.0] - 2026-10-17

### Added

- Neon light and dark themes: graphite surfaces with the icon's orange reserved for the caret, selection, unsaved state and checked tasks.
- Markdown syntax colors in the editor that match the theme, with heading and list markers in the accent color.
- Guidance when the sidebar has no folder or no Markdown files, and when the preview has nothing to show.
- Settings page, opened with Cmd+, or Neon > Settings…, with Appearance and Typography sections. Escape or Done returns to the document.
- Choose System, Light or Dark appearance in Settings. System follows the macOS setting as it changes.
- List each font in the font picker in its own typeface, and reset the editor or preview typography to its defaults.
- Add a reproducible macOS memory benchmark covering startup, editing, images, large blocks, folders and document release.
- Slash commands: type `/` on an empty line to insert a bulleted, numbered or task list, a code block, a link, an image or a table, with a preview of the Markdown in the menu. Add a size after the command, such as `/bullet 5` or `/table 4x5` (columns x rows), or a language, such as `/code rust`. Escape closes the menu until you type `/` again.

### Changed

- Show the editor and preview on one rounded sheet inside a single window frame, with one divider in Split instead of two bordered panels and their labels.
- Mark unsaved changes next to the file name with a dot and "Edited", followed by a Save button that shows only while there is something to save.
- Show the folder name in the title bar only while the sidebar is hidden.
- Give sidebar rows rounded selection, disclosure chevrons and aligned icons, and highlight the open file.
- Tighten preview typography: a 40 rem reading column, headings spaced closer to the text they introduce, no heading underlines, framed code blocks and tables, accent-underlined links, and dimmed completed tasks.
- Run the sidebar the full height of the window, with the window controls and the sidebar button in its top row.
- Give the document sheet its own header and footer: the file name and view controls above the text, the cursor position, reading progress and word count below it.
- Move Open Folder and Refresh Folder into a folder menu in the sidebar header, which shows a spinner while the folder is read.
- Name the sidebar button by what it does: Hide sidebar or Show sidebar.
- Replace the Typography dialog and its View menu item with the Settings page, opened from the gear button in the document header; font samples render on the document surface.
- Show the FPS counter only in builds with the `diagnostics` feature.

### Performance

- Release hidden previews and their images in Editor mode and Settings, rebuilding from the latest text when shown again.
- Keep preview images in a document-owned cache with a 64 MiB retention target, releasing them when switching documents; visible images may exceed the target.
- Render long lists in visible chunks instead of laying out every item at once.
- Create font pickers only while Typography settings are open, sharing installed font names across windows.
- Remove profiler allocations from ordinary builds and stop retaining intermediate text snapshots during background syntax highlighting.

## [0.2.0] - 2026-10-17

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
