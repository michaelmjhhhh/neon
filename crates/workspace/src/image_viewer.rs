use std::path::{Path, PathBuf};

use gpui_kit::assets::IconName as AssetIcon;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _};
use gpui_kit::{
    Context, Entity, FocusHandle, Focusable, InteractiveElement as _, IntoElement, ObjectFit,
    ParentElement as _, Render, SharedString, Styled as _, StyledImage as _, Window, actions, div,
    img,
};
use markdown_preview::PreviewImageCache;

pub(crate) const KEY_CONTEXT: &str = "ImageViewer";

actions!(image_viewer, [PreviousImage, NextImage]);

/// The image area of the image viewer: one image from the sidebar, scaled
/// down to fit and never up, with Left and Right stepping through the
/// images beside it. The dialog around it shows its name and actions.
pub(crate) struct ImageViewer {
    /// The images in the shown one's folder, in the sidebar's order.
    images: Vec<PathBuf>,
    index: usize,
    pub(crate) focus_handle: FocusHandle,
    /// Holds the shown image alone, so stepping through a folder of
    /// photos frees each one as the next shows.
    cache: Entity<PreviewImageCache>,
}

impl ImageViewer {
    pub(crate) fn new(images: Vec<PathBuf>, index: usize, cx: &mut Context<Self>) -> Self {
        Self {
            index: index.min(images.len().saturating_sub(1)),
            images,
            focus_handle: cx.focus_handle(),
            cache: PreviewImageCache::new(cx),
        }
    }

    pub(crate) fn path(&self) -> &Path {
        &self.images[self.index]
    }

    /// The shown image's place among its folder's images, from 1, and
    /// how many there are.
    pub(crate) fn position(&self) -> (usize, usize) {
        (self.index + 1, self.images.len())
    }

    pub(crate) fn previous(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.index > 0 {
            self.show(self.index - 1, window, cx);
        }
    }

    pub(crate) fn next(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.index + 1 < self.images.len() {
            self.show(self.index + 1, window, cx);
        }
    }

    fn show(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.index = index;
        self.cache = PreviewImageCache::new(cx);
        cx.notify();
        // The dialog's title and buttons are built with the window, not
        // with this view.
        window.refresh();
    }
}

impl Focusable for ImageViewer {
    fn focus_handle(&self, _: &gpui_kit::App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for ImageViewer {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        // Each image has state of its own, such as how long it has loaded.
        let id: SharedString = self.path().to_string_lossy().into_owned().into();
        let fallback = move || {
            div()
                .size_full()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap_2()
                .text_sm()
                .text_color(muted)
                .child(Icon::new(AssetIcon::ImageOff).large())
                .child("Neon can’t show this image. Open it in its default app instead.")
                .into_any_element()
        };
        div()
            .id("image-viewer")
            .key_context(KEY_CONTEXT)
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(|this, _: &PreviousImage, window, cx| this.previous(window, cx)))
            .on_action(cx.listener(|this, _: &NextImage, window, cx| this.next(window, cx)))
            .flex()
            .items_center()
            .justify_center()
            .w_full()
            // Room for the title and buttons in the window's height.
            .h(window.viewport_size().height * 0.6)
            .p_2()
            .rounded(theme.radius)
            // A ground for transparent images and white edges.
            .bg(theme.muted)
            .child(
                img(self.path().to_path_buf())
                    .image_cache(&self.cache)
                    .id(id)
                    .size_full()
                    .object_fit(ObjectFit::ScaleDown)
                    .with_loading(|| Spinner::new().into_any_element())
                    .with_fallback(fallback),
            )
    }
}
