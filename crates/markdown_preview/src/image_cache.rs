use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use gpui_kit::{
    App, AppContext as _, Entity, EntityId, ImageCache, ImageCacheError, ImageCacheItem,
    RenderImage, Resource, Task, WeakEntity, Window,
};

/// Keep recently viewed images warm without retaining every image in a document.
/// Images used by the current layout are pinned, even if they exceed the budget:
/// evicting those would repeatedly decode the same visible image.
const DECODED_BYTE_BUDGET: usize = 64 * 1024 * 1024;
const ENTRY_BUDGET: usize = 128;

/// Widest image kept after decoding, in device pixels. The reading column is
/// at most 40 rem (640 px) wide, 1,280 device pixels on a 2x display, so wider
/// pixels are never shown. A 4,000 x 3,000 photo decodes to 48 MB, and the GPU
/// keeps another copy; at this width both shrink to 12.6 MB.
const MAX_DECODED_WIDTH: u32 = 2048;

type LoadResult = Result<Arc<RenderImage>, ImageCacheError>;

enum State {
    Loading(ImageCacheItem),
    /// The decoded image was too wide; a background task is shrinking it.
    /// These views drew the image as loading and redraw when it is ready.
    Fitting(HashSet<EntityId>),
    Loaded(LoadResult),
}

struct Entry {
    state: State,
    /// Owned outside the state, so publishing a result cannot cancel its own task.
    fit_task: Option<Task<()>>,
    last_used: u64,
}

impl Entry {
    fn image(&self) -> Option<Arc<RenderImage>> {
        match &self.state {
            State::Loading(item) => item.get().and_then(Result::ok),
            State::Fitting(_) => None,
            State::Loaded(result) => result.clone().ok(),
        }
    }
}

/// Decodes images for `img(...).image_cache(...)`, shrinking any wider than
/// [`MAX_DECODED_WIDTH`], and frees them all when dropped.
pub struct PreviewImageCache {
    this: WeakEntity<Self>,
    entries: HashMap<Resource, Entry>,
    frame: u64,
}

impl PreviewImageCache {
    pub fn new(cx: &mut App) -> Entity<Self> {
        let cache = cx.new(|cx| Self {
            this: cx.weak_entity(),
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
            .filter_map(|entry| entry.image())
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
            let image_bytes = entry.image().map_or(0, |image| decoded_bytes(&image));
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
                if let Some(image) = entry.image() {
                    cx.drop_image(image, Some(window));
                }
            }
        }
    }

    /// Shrink an oversized image on a background thread, then show it in
    /// every view that asked for it meanwhile.
    fn fit(&self, resource: Resource, image: Arc<RenderImage>, cx: &mut App) -> Task<()> {
        let this = self.this.clone();
        cx.spawn(async move |cx| {
            let fitted = cx
                .background_spawn(async move { fit_width(&image).unwrap_or(image) })
                .await;
            this.update(cx, |cache, cx| {
                let Some(entry) = cache.entries.get_mut(&resource) else {
                    return;
                };
                let previous = std::mem::replace(&mut entry.state, State::Loaded(Ok(fitted)));
                if let State::Fitting(views) = previous {
                    let cx: &mut App = cx;
                    for view in views {
                        cx.notify(view);
                    }
                }
            })
            .ok();
        })
    }
}

impl ImageCache for PreviewImageCache {
    fn load(
        &mut self,
        resource: &Resource,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<LoadResult> {
        let frame = self.frame;
        let entry = self
            .entries
            .entry(resource.clone())
            .or_insert_with(|| Entry {
                state: State::Loading(ImageCacheItem::new(resource, cx)),
                fit_task: None,
                last_used: frame,
            });
        entry.last_used = frame;
        let image = match &mut entry.state {
            State::Loaded(result) => return Some(result.clone()),
            State::Fitting(views) => {
                views.insert(window.current_view());
                return None;
            }
            State::Loading(item) => match item.use_image(window)? {
                Ok(image) if needs_fit(&image) => image,
                result => {
                    entry.state = State::Loaded(result.clone());
                    return Some(result);
                }
            },
        };
        // Dropping the loader leaves the fit task with the only full-size
        // copy, which is freed once the smaller image replaces it.
        entry.state = State::Fitting(HashSet::from([window.current_view()]));
        let task = self.fit(resource.clone(), image, cx);
        if let Some(entry) = self.entries.get_mut(resource) {
            entry.fit_task = Some(task);
        }
        None
    }
}

/// Animated images keep every frame as decoded; only still images shrink.
fn needs_fit(image: &RenderImage) -> bool {
    image.frame_count() == 1 && u32::from(image.size(0).width) > MAX_DECODED_WIDTH
}

/// The image scaled down to [`MAX_DECODED_WIDTH`], keeping its aspect ratio.
fn fit_width(image: &RenderImage) -> Option<Arc<RenderImage>> {
    let size = image.size(0);
    let (width, height) = (u32::from(size.width), u32::from(size.height));
    let pixels =
        image::ImageBuffer::<image::Rgba<u8>, _>::from_raw(width, height, image.as_bytes(0)?)?;
    let fitted_height = (u64::from(height) * u64::from(MAX_DECODED_WIDTH) / u64::from(width))
        .clamp(1, u64::from(u32::MAX)) as u32;
    // Area averaging writes straight into the output; a filtered resize
    // would first build a floating-point copy several times the image size.
    // It treats the four channels alike, so BGRA stays BGRA.
    let fitted = image::imageops::thumbnail(&pixels, MAX_DECODED_WIDTH, fitted_height);
    let frame = image::Frame::from_parts(fitted, 0, 0, image.delay(0));
    Some(Arc::new(RenderImage::new([frame])))
}

fn decoded_bytes(image: &RenderImage) -> usize {
    (0..image.frame_count()).fold(0usize, |bytes, frame| {
        bytes.saturating_add(image.as_bytes(frame).map_or(0, <[u8]>::len))
    })
}
