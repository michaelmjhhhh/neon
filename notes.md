First release.

### Added

- Markdown editor with syntax highlighting, built on GPUI; handles files of
  tens of thousands of lines.
- Live preview: headings, emphasis, links, task lists, nested and numbered
  lists, block quotes, code blocks, tables with column alignment, images,
  footnotes and YAML front matter.
- Editor, Split and Preview layouts, switched with ⌘1, ⌘2 and ⌘3; in Split,
  the preview follows the editor's cursor.
- Open (⌘O), Save (⌘S) and Save As (⌘⇧S), with a confirmation before
  discarding unsaved changes.
- Open Markdown files from Finder; a file that is already open comes to the
  front instead of opening twice.
- Light and dark appearance that follows the system, with a manual toggle in
  the View menu.
- Status bar with the cursor position and word count.

### Performance

- The preview parses on a background thread and re-lays out only the blocks
  an edit changed, so typing stays responsive in large documents.

## Install

Download `Neon-0.1.0-macos.dmg`, open it and drag Neon to Applications. It runs natively on Apple Silicon and Intel Macs, macOS 11 or later.

Neon is not signed with an Apple Developer ID yet, so macOS blocks the first launch. Try to open Neon once, then go to System Settings → Privacy & Security and click **Open Anyway**.
