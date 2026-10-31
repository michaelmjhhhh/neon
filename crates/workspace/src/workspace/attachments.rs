//! Attachments in the sidebar: images, PDFs and other files beside the
//! notes. An image opens in the image viewer, anything else in its default
//! app, and either goes into a note as a Markdown link.

use std::path::{Path, PathBuf};

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::dialog::{DialogAction, DialogFooter};
use gpui_kit::component::input::RopeExt as _;
use gpui_kit::component::notification::Notification;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, IconName, Sizable as _, WindowExt as _,
};
use gpui_kit::{
    AppContext as _, Context, Focusable as _, InteractiveElement as _, IntoElement,
    ParentElement as _, SharedString, Styled as _, Window, div, prelude::FluentBuilder as _,
};

use super::panes::DraggedFile;
use super::{CopyEntryLink, ViewEntry, Workspace, file_name};
use crate::folder::{FileKind, embed_padding, file_kind, markdown_link};
use crate::image_viewer::{self, ImageViewer, NextImage, PreviousImage};

impl Workspace {
    /// Open a file picked in the sidebar the way its kind opens: a note in
    /// a tab, an image in the viewer, anything else in its default app.
    pub(super) fn open_entry(
        &mut self,
        path: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match file_kind(&path) {
            Some(FileKind::Markdown) => self.open_tree_file(path, window, cx),
            Some(FileKind::Image) => self.view_image(path, window, cx),
            Some(FileKind::Attachment) => cx.open_with_system(&path),
            None => {}
        }
    }

    /// Space in the sidebar, as Quick Look in Finder: show the selected
    /// image, or open the selected attachment. A note opens as soon as it
    /// is selected, so Space leaves it be.
    pub(super) fn view_selected_entry(
        &mut self,
        _: &ViewEntry,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some((path, false)) = self.selected_entry(cx)
            && file_kind(&path) != Some(FileKind::Markdown)
        {
            self.open_entry(path, window, cx);
        }
    }

    pub(super) fn copy_selected_entry_link(
        &mut self,
        _: &CopyEntryLink,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some((path, false)) = self.selected_entry(cx) {
            self.copy_link(&path, window, cx);
        }
    }

    /// Put a Markdown link to `target`, from the active note, on the
    /// clipboard.
    pub(super) fn copy_link(&self, target: &Path, window: &mut Window, cx: &mut Context<Self>) {
        let link = markdown_link(&self.link_base(self.tab().id), target);
        cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(link));
        window.push_notification(
            Notification::info(format!("Copied a link to “{}”.", file_name(target))),
            cx,
        );
    }

    /// The folder links in the note with tab id `tab_id` start from: its
    /// own, or for an untitled note the open folder, where new files go.
    fn link_base(&self, tab_id: u64) -> PathBuf {
        self.tabs
            .iter()
            .find(|tab| tab.id == tab_id)
            .and_then(|tab| tab.document.path())
            .and_then(Path::parent)
            .map(Path::to_path_buf)
            .or_else(|| self.folder.clone())
            .unwrap_or_default()
    }

    /// Insert a Markdown link to `target` at the cursor of the note with
    /// tab id `tab_id`, after any selection, and put focus there.
    pub(super) fn insert_link(
        &mut self,
        tab_id: u64,
        target: &Path,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.is_loading() {
            return;
        }
        let Some(editor) = self
            .tabs
            .iter()
            .find(|tab| tab.id == tab_id)
            .map(|tab| tab.editor.clone())
        else {
            return;
        };
        let link = markdown_link(&self.link_base(tab_id), target);
        let image = file_kind(target) == Some(FileKind::Image);
        editor.update(cx, |editor, cx| {
            let at = editor.selected_range().end;
            let (before, after) = if image {
                let text = editor.text();
                let row = text.offset_to_point(at).row;
                let line_has_text = |row: usize| {
                    let line = text.slice(text.line_start_offset(row)..text.line_end_offset(row));
                    !line.to_string().trim().is_empty()
                };
                embed_padding(
                    &text.slice(text.line_start_offset(row)..at).to_string(),
                    &text.slice(at..text.line_end_offset(row)).to_string(),
                    row.checked_sub(1).map(line_has_text),
                    (row + 1 < text.lines_len()).then(|| line_has_text(row + 1)),
                )
            } else {
                ("", "")
            };
            editor.set_selected_range(at..at, cx);
            editor.replace(format!("{before}{link}{after}"), window, cx);
            // The cursor goes after the link, not the space below it.
            let end = at + before.len() + link.len();
            editor.set_selected_range(end..end, cx);
        });
        if tab_id != self.tab().id {
            self.focus_other_pane(window, cx);
        }
        self.focus_content(window, cx);
    }

    /// Show the image at `path`, with the other images in its folder a
    /// step away, as the sidebar lists them.
    fn view_image(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let directory = path.parent().map(Path::to_path_buf);
        let images: Vec<PathBuf> = {
            let tree = self.tree.read(cx);
            (0..)
                .map_while(|ix| tree.entry(ix))
                .filter(|entry| !entry.is_folder() && !entry.is_disabled())
                .map(|entry| PathBuf::from(entry.item().id.as_ref()))
                .filter(|image| {
                    image.parent().map(Path::to_path_buf) == directory
                        && file_kind(image) == Some(FileKind::Image)
                })
                .collect()
        };
        let (images, index) = match images.iter().position(|image| *image == path) {
            Some(index) => (images, index),
            None => (vec![path], 0),
        };
        let viewer = cx.new(|cx| ImageViewer::new(images, index, cx));
        let handle = viewer.read(cx).focus_handle(cx);
        let tab_id = self.tab().id;
        let this = cx.weak_entity();
        window.open_dialog(cx, move |dialog, window, cx| {
            let state = viewer.read(cx);
            let path = state.path().to_path_buf();
            let (position, count) = state.position();
            let muted = cx.theme().muted_foreground;
            let step = |id: &'static str,
                        icon: IconName,
                        label: &'static str,
                        action: &dyn gpui_kit::Action,
                        disabled: bool,
                        step: fn(&mut ImageViewer, &mut Window, &mut Context<ImageViewer>)| {
                let viewer = viewer.clone();
                Button::new(id)
                    .ghost()
                    .small()
                    .icon(icon)
                    .accessibility_label(label)
                    .tooltip_with_action(label, action, Some(image_viewer::KEY_CONTEXT))
                    .disabled(disabled)
                    .on_click(move |_, window, cx| {
                        viewer.update(cx, |viewer, cx| {
                            step(viewer, window, cx);
                            // The press focused the button; Left and
                            // Right work only on the image.
                            viewer.focus_handle.focus(window, cx);
                        })
                    })
            };
            // A note still loading takes no link; the viewer stays open
            // rather than closing on nothing.
            let can_insert = this
                .upgrade()
                .is_some_and(|this| !this.read(cx).is_loading());
            let on_ok = {
                let this = this.clone();
                let path = path.clone();
                move |_: &gpui_kit::ClickEvent, window: &mut Window, cx: &mut gpui_kit::App| {
                    if !can_insert {
                        return false;
                    }
                    // Closed first, so focus goes back to where it was
                    // before the link moves it into the note.
                    window.close_dialog(cx);
                    this.update(cx, |this, cx| this.insert_link(tab_id, &path, window, cx))
                        .ok();
                    false
                }
            };
            let open = {
                let path = path.clone();
                move |_: &gpui_kit::ClickEvent, _: &mut Window, cx: &mut gpui_kit::App| {
                    cx.open_with_system(&path)
                }
            };
            dialog
                .title(
                    div()
                        .min_w_0()
                        .truncate()
                        .child(SharedString::from(file_name(&path))),
                )
                .w(window.rem_size() * 60.)
                .margin_top(window.viewport_size().height * 0.06)
                .child(viewer.clone())
                .footer(
                    DialogFooter::new()
                        .justify_between()
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_1()
                                // One image needs no way to the next.
                                .when(count > 1, |row| {
                                    row.child(step(
                                        "previous-image",
                                        IconName::ChevronLeft,
                                        "Previous Image",
                                        &PreviousImage,
                                        position == 1,
                                        ImageViewer::previous,
                                    ))
                                    .child(step(
                                        "next-image",
                                        IconName::ChevronRight,
                                        "Next Image",
                                        &NextImage,
                                        position == count,
                                        ImageViewer::next,
                                    ))
                                    .child(
                                        div()
                                            .pl_1()
                                            .text_sm()
                                            .text_color(muted)
                                            .child(format!("{position} of {count}")),
                                    )
                                }),
                        )
                        .child(
                            div()
                                .flex()
                                .gap_2()
                                .child(
                                    Button::new("open-default-app")
                                        .outline()
                                        .label("Open in Default App")
                                        .on_click(open),
                                )
                                .child(
                                    DialogAction::new().child(
                                        Button::new("insert-link")
                                            .primary()
                                            .label("Insert Link")
                                            .disabled(!can_insert),
                                    ),
                                ),
                        ),
                )
                .on_ok(on_ok)
        });
        // The dialog takes focus as it opens; hand it to the image, where
        // Left and Right step through the folder.
        handle.focus(window, cx);
    }

    /// Where an attachment dragged from the sidebar can land: anywhere on
    /// the pane showing the note with tab id `tab_id`, which takes a link
    /// to it at its cursor.
    pub(super) fn link_drop_zone(&self, tab_id: u64, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let tint = theme.drop_target;
        div()
            .flex_1()
            .h_full()
            .flex()
            .items_end()
            .justify_center()
            .pb_6()
            .drag_over::<DraggedFile>(move |style, _, _, _| style.bg(tint))
            .on_drop(cx.listener(move |this, file: &DraggedFile, window, cx| {
                this.dragging_file.set(None);
                this.insert_link(tab_id, &file.path.clone(), window, cx);
            }))
            // The link goes in at the cursor, not where the file drops.
            .child(
                div()
                    .px_2()
                    .py_1()
                    .rounded(theme.radius)
                    .border_1()
                    .border_color(theme.border)
                    .bg(theme.popover)
                    .text_color(theme.popover_foreground)
                    .text_xs()
                    .child("Drop to insert a link at the cursor"),
            )
    }
}
