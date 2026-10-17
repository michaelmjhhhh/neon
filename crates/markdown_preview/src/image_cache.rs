use std::collections::HashMap;
use std::sync::Arc;

use gpui_kit::{
    App, AppContext as _, Entity, ImageCache, ImageCacheError, ImageCacheItem, RenderImage,
    Resource, Window,
};

/// Keep recently viewed images warm without retaining every image in a document.
/// Images used by the current layout are pinned, even if they exceed the budget:
/// evicting those would repeatedly decode the same visible image.
const DECODED_BYTE_BUDGET: usize = 64 * 1024 * 1024;
const ENTRY_BUDGET: usize = 128;

struct Entry {
    image: ImageCacheItem,
    last_used: u64,
}

pub(crate) struct PreviewImageCache {
    entries: HashMap<Resource, Entry>,
    frame: u64,
}

impl PreviewImageCache {
    pub(crate) fn new(cx: &mut App) -> Entity<Self> {
        let cache = cx.new(|_| Self {
            entries: HashMap::new(),
            frame: 0,
        });
        cx.observe_release(&cache, |cache, cx| cache.clear(cx))
            .detach();
        cache
    }

    pub(crate) fn clear(&mut self, cx: &mut App) {
        let images = std::mem::take(&mut self.entries)
            .into_values()
            .filter_map(|entry| entry.image.get().and_then(Result::ok))
            .collect::<Vec<_>>();
        // A clear or release can happen while the current window is borrowed.
        // Defer until it is back in App so every window's GPU tiles are removed.
        if !images.is_empty() {
            cx.defer(move |cx| {
                for image in images {
                    cx.drop_image(image, None);
                }
            });
        }
    }

    pub(crate) fn begin_frame(&mut self) {
        self.frame = self.frame.wrapping_add(1);
    }

    /// Run after painting, once every image in the current layout has been used.
    /// The budget includes all decoded frames, rather than compressed file size.
    pub(crate) fn finish_frame(&mut self, window: &mut Window, cx: &mut App) {
        let mut bytes = 0usize;
        let mut evictable = Vec::new();
        for (source, entry) in &self.entries {
            let image_bytes = entry
                .image
                .get()
                .and_then(Result::ok)
                .map_or(0, |image| decoded_bytes(&image));
            bytes = bytes.saturating_add(image_bytes);
            if entry.last_used != self.frame {
                evictable.push((entry.last_used, source.clone(), image_bytes));
            }
        }
        if bytes <= DECODED_BYTE_BUDGET && self.entries.len() <= ENTRY_BUDGET {
            return;
        }
        evictable.sort_unstable_by_key(|(last_used, _, _)| *last_used);
        for (_, source, image_bytes) in evictable {
            if bytes <= DECODED_BYTE_BUDGET && self.entries.len() <= ENTRY_BUDGET {
                break;
            }
            if let Some(entry) = self.entries.remove(&source) {
                bytes = bytes.saturating_sub(image_bytes);
                if let Some(Ok(image)) = entry.image.get() {
                    cx.drop_image(image, Some(window));
                }
            }
        }
    }
}

impl ImageCache for PreviewImageCache {
    fn load(
        &mut self,
        resource: &Resource,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<Result<Arc<RenderImage>, ImageCacheError>> {
        let entry = self
            .entries
            .entry(resource.clone())
            .or_insert_with(|| Entry {
                image: ImageCacheItem::new(resource, cx),
                last_used: self.frame,
            });
        entry.last_used = self.frame;
        entry.image.use_image(window)
    }
}

fn decoded_bytes(image: &RenderImage) -> usize {
    (0..image.frame_count()).fold(0usize, |bytes, frame| {
        bytes.saturating_add(image.as_bytes(frame).map_or(0, <[u8]>::len))
    })
}
