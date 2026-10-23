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

- Right-click a file or folder in the sidebar to Copy, Paste, Duplicate, Copy Path or Reveal in Finder; with the sidebar focused, ⌘C, ⌘V, ⌘D, ⌥⌘C and ⌥⌘R do the same, and the folder's ⋯ menu can paste too. A copy never overwrites: a taken name becomes "name copy", "name copy 2" and so on, a folder copy brings its images and other files along, and a folder cannot be pasted into itself.

## [0.10.0] - 2026-10-23

### Added

- Zoom In (⌘=), Zoom Out (⌘-) and Actual Size (⌘0) in the View menu make the editor and preview text bigger or smaller together, saved like the size chosen in Settings.
- The preview shows Obsidian and GitHub callouts (`> [!tip] Title`) as tinted panels with an icon, in the color of their kind.
- `==Highlighted==` text shows with a soft accent wash in the preview; an escaped `\==` stays as typed, as in Obsidian.
- Inline `code` and highlights in the preview sit on rounded chips a little shorter than the line, instead of square blocks the full line height.
- `[[Wikilinks]]` and links to other Markdown files open the note in Neon: a note already open comes forward, and a tab with unsaved edits is kept by opening the note in a new tab. A note that is not beside the document is found by name anywhere in the open folder, as in Obsidian.
- Front matter shows as quiet properties above the document, keys beside values, instead of a YAML code block; lists join with commas and `|` and `>` blocks keep or fold their lines.

### Changed

- The preview reads in Charter at size 17 with more open leading and lighter, semibold headings; choose System default in Settings for SF Pro.
- The Editor layout centers the text in a column about 80 characters wide, and the margins beside it scroll the text; the column widens for line numbers, so the text keeps its width with them on.
- Markdown symbols in the editor (`#`, `>`, `1.`, `---`) are a quiet gray instead of the accent color, so the words lead.
- Line numbers are off by default for new settings; Cmd+Shift+L or Settings turns them on.
- Inline code in the preview is set in PT Mono when its letters stand closer to the height of the text's than the theme's monospace font, so code no longer looks larger than the Charter around it.
- The sidebar lists notes by name alone, without file icons or the `.md` extension, and the selected row is a neutral gray instead of an orange tint.
- A nested list sits as close to its parent item as the items sit to each other.
- An image that cannot load shows which path it looked for.
- Nested list items in the editor hang from one continuous line under their parent's marker, running past wrapped lines, instead of short ticks beside each line; the line around the item you are editing is a little darker.
- HTML, PDF and Word exports show callouts as tinted panels with their title, highlights on the same wash, `[[wikilinks]]` as links to the note and front matter as a table of properties, as the preview does, instead of plain text and a YAML block.

### Fixed

- Links and images with escaped characters, such as `My%20Note.md`, open the file they name.
- The editor stays on the line you were reading when the text size or font changes, instead of jumping hundreds of lines in a long document.
- Code and highlight chips in centered and right-aligned table cells sit behind their text instead of at the cell's left, and end before the space a line wraps at.

## [0.9.1] - 2026-10-22

### Changed

- Zen Mode's footer shows Editor · Preview and Exit Zen Mode as plain muted words instead of a boxed switch, with the current view in full ink and no background on hover.
- Zen Mode always opens on the source, ready to type in, even from the Preview layout; Cmd+3 still shows the preview.
- In Zen Mode, the line you type on reaches the middle of the screen from the first line of the document, not only once the text is long enough to scroll.
- Zen Mode's footer fades away once you start typing and comes back when you move the pointer, and the paragraphs away from the caret fade further back.
- The header's top right is quieter: the Editor, Split and Preview switch shows icons only, with each name and shortcut in its tooltip, and sits beside the Zen button, with Settings at the far edge.
- Save always appears at the start of the header's right-hand controls when there are edits to save, instead of moving between the title and the view controls as tabs open and close.

### Fixed

- The sidebar keeps its width when you hide and show it, instead of coming back wider.
- The split between editor and preview keeps its position when you switch layouts or hide the sidebar.

## [0.9.0] - 2026-10-21

### Added

- Zen Mode (Control-Cmd-Z, the Zen button in the header, or View > Toggle Zen Mode) shows the document alone, full screen, in a centered column: the paragraph you are writing stays at full strength while the rest fades back, and the line you type on or click glides to the middle of the screen. A switch under the column, or Cmd+1 and Cmd+3, moves between the source and the preview; Esc, the same shortcut or Exit Zen Mode returns to the layout you left.

### Changed

- Find (Cmd+F) and Find and Replace (Cmd+Shift+F) open a compact bar at the editor's top right instead of a strip across the whole editor; it shows the match count as "3 of 12" and Enter reveals the first match before moving on, and Esc closes it from the bar or the editor.
- Focused fields and controls show a thin accent border instead of a soft orange glow around it.
- The status bar shows how far you have read as a percentage with a small meter at its right edge, without the "Editor" or "Preview" prefix, and its numbers keep their place as they change.

## [0.8.0] - 2026-10-20

### Added

- Open several files in one window as tabs: Cmd+T opens a new tab, and a file picked in the sidebar opens in the tab you are on. Each tab keeps its own edits, undo history and scroll position.
- Switch tabs by clicking them, with Control-Tab and Control-Shift-Tab, or with Cmd+} and Cmd+{.
- Close a tab with Cmd+W, its close button or a middle click; a tab with unsaved edits asks first.

### Changed

- Cmd+W closes the current tab, and the window once its last tab closes; Close Window moves to Cmd+Shift+W.
- Opening a file that a tab already shows, from the sidebar, Open File or Finder, switches to that tab instead of opening it again.
- Closing a window or quitting asks before discarding unsaved edits in any of its tabs, not only the one shown.
- Open File and New File ask about unsaved edits after you choose the file instead of before, so choosing a file another tab shows just switches to it.

### Fixed

- Show the preview in the font and size saved in Settings as soon as a window opens; they used to apply only after being changed again.

## [0.7.0] - 2026-10-20

### Added

- Reopen the folder and file you last worked on when Neon starts, or when you click the Dock icon with no windows open. A file opened from Finder or the command line takes their place, and a folder or file that no longer exists is skipped.

### Changed

- Break the line in the preview and in exports wherever the source has a single newline, instead of joining the two lines into one paragraph line.
- Polish the title bar: a chevron separates the folder from the file, the file's extension is dimmed so its name stands out, and the settings button sits further from the layout switcher.
- Show an icon beside each mode in the Editor, Split and Preview switcher, with smaller labels that fit the control.

### Fixed

- Show the full focus ring around the name field in the New Folder dialog; its top edge was cut off.

## [0.6.1] - 2026-10-20

### Performance

- Keep preview images no wider than the preview can show: a 4,000 × 3,000 photo now takes 12.6 MB of memory instead of 48 MB, and the same reduction applies to its GPU copy.
- Shrink the app from 30.1 to 16.2 MB on Apple Silicon by building with full link-time optimization and without debug symbols.
- Bold and Italic shortcuts read only the selected lines instead of copying the whole document on each press.

## [0.6.0] - 2026-10-19

### Added

- New Folder creates a folder in the sidebar, inside the selected folder or beside the selected file. Use the folder button in the sidebar header, File > New Folder…, or Cmd+Shift+N. A name that is already taken is flagged in the dialog before anything is created.
- Line numbers beside the editor can be hidden: Cmd+Shift+L toggles them in the current window, and Settings > Editor sets whether they show by default.

### Changed

- The sidebar shows empty folders, with an "Empty" row inside that selects the folder when clicked. Folders that hold only non-Markdown files stay hidden.
- The sidebar tree draws indent guides, shows how many Markdown files a collapsed folder holds, and dims file extensions.
- Clicking empty space in the sidebar clears the selection, so New File and New Folder go to the top of the open folder.

## [0.5.2] - 2026-10-19

### Fixed

- The word count counts each Chinese and Japanese character as one word, instead of counting a whole unspaced sentence as one. Markdown markers such as `#` and `-` no longer count as words.

## [0.5.1] - 2026-10-19

### Added

- Each release lists SHA-256 checksums and a build provenance attestation, so a download can be checked against the source it was built from.

### Changed

- Neon is signed with an Apple Developer ID and notarized by Apple, so macOS opens it without asking for approval first.

## [0.5.0] - 2026-10-19

### Added

- Enter on a list item starts the next one: `- ` bullets keep their marker and indent, numbered items count up, and tasks continue unchecked. Enter on an empty item ends the list; Shift+Enter inserts a plain newline.
- Tab on a list item indents the whole item one level, making it a child of the item above. Shift+Tab moves it back out.
- Settings are remembered: the appearance and each pane's font and size are saved and restored the next time Neon opens.
- Cmd+B and Cmd+I make the selection bold or italic, and pressing them again removes the markers. With no selection they insert an empty pair with the cursor inside.

### Changed

- Toggle Sidebar moves to Cmd+Shift+B, freeing Cmd+B for bold.

### Fixed

- Saving with Cmd+S no longer reopens a hidden sidebar. Cmd+Shift+B still toggles it.
- The input method's candidate window stays at the caret when typing fast, instead of lagging a few characters behind and jumping forward.

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
