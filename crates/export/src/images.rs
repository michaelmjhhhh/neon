//! Local images referenced by a document, read so an export can carry them.

use std::path::{Path, PathBuf};

/// Larger files are left out rather than inflating the export.
const MAX_IMAGE_BYTES: u64 = 32 * 1024 * 1024;

/// An image file's bytes and what they hold.
pub(crate) struct LocalImage {
    pub bytes: Vec<u8>,
    pub mime: &'static str,
    /// Width and height in pixels.
    pub size: (u32, u32),
}

/// Whether `url` names a web resource the export can't embed.
pub(crate) fn is_remote(url: &str) -> bool {
    url.starts_with("http://") || url.starts_with("https://")
}

/// Read the image a Markdown `url` points at the way the preview resolves
/// it: absolute paths as they are, relative ones against the document's
/// folder. `None` for web images, other schemes, and files that are missing,
/// too large or not an image this export understands.
pub(crate) fn load(url: &str, base_dir: Option<&Path>) -> Option<LocalImage> {
    let path = resolve_path(url, base_dir)?;
    if std::fs::metadata(&path).ok()?.len() > MAX_IMAGE_BYTES {
        return None;
    }
    let bytes = std::fs::read(&path).ok()?;
    let format = image::guess_format(&bytes).ok()?;
    let mime = match format {
        image::ImageFormat::Png => "image/png",
        image::ImageFormat::Jpeg => "image/jpeg",
        image::ImageFormat::Gif => "image/gif",
        image::ImageFormat::WebP => "image/webp",
        _ => return None,
    };
    let size = image::ImageReader::with_format(std::io::Cursor::new(&bytes), format)
        .into_dimensions()
        .ok()
        .filter(|(width, height)| *width > 0 && *height > 0)?;
    Some(LocalImage { bytes, mime, size })
}

impl LocalImage {
    /// The image as PNG, which is all Word documents built here can hold.
    pub(crate) fn to_png(&self) -> Option<Vec<u8>> {
        if self.mime == "image/png" {
            return Some(self.bytes.clone());
        }
        let image = image::load_from_memory(&self.bytes).ok()?;
        let mut png = std::io::Cursor::new(Vec::new());
        image.write_to(&mut png, image::ImageFormat::Png).ok()?;
        Some(png.into_inner())
    }

    /// Width and height scaled down, never up, to fit `max_width`.
    pub(crate) fn fitted(&self, max_width: f32) -> (f32, f32) {
        let (width, height) = (self.size.0 as f32, self.size.1 as f32);
        if width <= max_width {
            (width, height)
        } else {
            (max_width, height * max_width / width)
        }
    }
}

fn resolve_path(url: &str, base_dir: Option<&Path>) -> Option<PathBuf> {
    let url = url.strip_prefix("file://").unwrap_or(url);
    if has_scheme(url) {
        return None;
    }
    let path = url
        .split(['#', '?'])
        .next()
        .filter(|path| !path.is_empty())?;
    let path = percent_decode(path).unwrap_or_else(|| path.to_string());
    let path = Path::new(&path);
    if path.is_absolute() {
        Some(path.to_path_buf())
    } else {
        base_dir.map(|dir| dir.join(path))
    }
}

fn has_scheme(url: &str) -> bool {
    url.split_once(':').is_some_and(|(scheme, _)| {
        scheme.len() > 1
            && scheme
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
    })
}

/// `my%20photo.png` as `my photo.png`; `None` when nothing is encoded or the
/// result isn't UTF-8.
fn percent_decode(path: &str) -> Option<String> {
    if !path.contains('%') {
        return None;
    }
    let bytes = path.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut ix = 0;
    while ix < bytes.len() {
        let hex = |b: Option<&u8>| b.and_then(|b| (*b as char).to_digit(16));
        match (bytes[ix], hex(bytes.get(ix + 1)), hex(bytes.get(ix + 2))) {
            (b'%', Some(hi), Some(lo)) => {
                decoded.push((hi * 16 + lo) as u8);
                ix += 3;
            }
            (byte, _, _) => {
                decoded.push(byte);
                ix += 1;
            }
        }
    }
    String::from_utf8(decoded).ok()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A 2×1 PNG.
    pub(crate) fn png() -> Vec<u8> {
        let image = image::RgbImage::from_pixel(2, 1, image::Rgb([200, 30, 30]));
        let mut png = std::io::Cursor::new(Vec::new());
        image.write_to(&mut png, image::ImageFormat::Png).unwrap();
        png.into_inner()
    }

    #[test]
    fn loads_relative_absolute_and_encoded_paths() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("my photo.png"), png()).unwrap();
        let relative = load("my%20photo.png", Some(dir.path())).unwrap();
        assert_eq!(relative.mime, "image/png");
        assert_eq!(relative.size, (2, 1));
        let absolute = dir.path().join("my photo.png");
        assert!(load(absolute.to_str().unwrap(), None).is_some());
        assert!(load("my photo.png", None).is_none());
    }

    #[test]
    fn skips_missing_remote_and_non_image_files() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("notes.txt"), "not an image").unwrap();
        assert!(load("notes.txt", Some(dir.path())).is_none());
        assert!(load("gone.png", Some(dir.path())).is_none());
        assert!(load("https://example.com/a.png", Some(dir.path())).is_none());
        assert!(load("data:image/png;base64,AAAA", Some(dir.path())).is_none());
    }

    #[test]
    fn fits_wide_images_and_keeps_small_ones() {
        let image = LocalImage {
            bytes: Vec::new(),
            mime: "image/png",
            size: (1000, 500),
        };
        assert_eq!(image.fitted(400.), (400., 200.));
        assert_eq!(image.fitted(2000.), (1000., 500.));
    }
}
