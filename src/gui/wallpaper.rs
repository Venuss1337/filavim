//! Bounded wallpaper decoding into one reusable GPUI image. The stage
//! ([`super::stage`]) owns loading, presentation and the frosted material.
use gpui::RenderImage;
use std::{
    io::{Cursor, Read},
    path::Path,
    sync::Arc,
};

pub fn decode(path: &Path) -> anyhow::Result<Arc<RenderImage>> {
    use anyhow::{Context, ensure};
    const MAX_FILE: u64 = 64 * 1024 * 1024;
    const MAX_PIXELS: u64 = 32 * 1024 * 1024;
    let file =
        std::fs::File::open(path).with_context(|| format!("Cannot open {}", path.display()))?;
    let metadata = file.metadata()?;
    ensure!(
        metadata.is_file() && metadata.len() <= MAX_FILE,
        "Wallpaper must be a regular file no larger than 64 MiB"
    );
    let mut bytes = Vec::new();
    file.take(MAX_FILE + 1).read_to_end(&mut bytes)?;
    ensure!(bytes.len() as u64 <= MAX_FILE, "Wallpaper exceeds 64 MiB");
    let header = image::ImageReader::new(Cursor::new(&bytes)).with_guessed_format()?;
    let format = header.format().context("Unrecognized wallpaper image")?;
    ensure!(
        matches!(
            format,
            image::ImageFormat::Png | image::ImageFormat::Jpeg | image::ImageFormat::WebP
        ),
        "Use a PNG, JPEG or WebP wallpaper"
    );
    let (width, height) = header.into_dimensions()?;
    ensure!(
        width > 0 && height > 0 && u64::from(width) * u64::from(height) <= MAX_PIXELS,
        "Wallpaper exceeds 32 megapixels"
    );
    let mut reader = image::ImageReader::with_format(Cursor::new(bytes), format);
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(16384);
    limits.max_image_height = Some(16384);
    limits.max_alloc = Some(128 * 1024 * 1024);
    reader.limits(limits);
    let mut rgba = reader
        .decode()
        .context("Cannot decode wallpaper")?
        .into_rgba8();
    for pixel in rgba.as_chunks_mut::<4>().0 {
        pixel.swap(0, 2);
    }
    Ok(Arc::new(RenderImage::new(vec![image::Frame::new(rgba)])))
}
#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{Bounds, DevicePixels, ObjectFit, point, px, size};
    use std::prelude::v1::test;
    #[test]
    fn image_formats_are_decoded_once_to_bgra_and_invalid_files_fail() {
        let temp = tempfile::tempdir().unwrap();
        for (extension, format) in [
            ("png", image::ImageFormat::Png),
            ("jpg", image::ImageFormat::Jpeg),
            ("webp", image::ImageFormat::WebP),
        ] {
            let path = temp.path().join(format!("wall.{extension}"));
            image::RgbImage::from_pixel(4, 2, image::Rgb([10, 20, 220]))
                .save_with_format(&path, format)
                .unwrap();
            let image = decode(&path).unwrap();
            assert_eq!(image.frame_count(), 1);
            assert_eq!(image.size(0).width.0, 4);
            if extension != "jpg" {
                assert_eq!(&image.as_bytes(0).unwrap()[..4], &[220, 20, 10, 255]);
            }
            let retained = image.clone();
            assert_eq!(image.id, retained.id);
        }
        assert!(decode(&temp.path().join("missing.png")).is_err());
        let bad = temp.path().join("bad.png");
        std::fs::write(&bad, b"not an image").unwrap();
        assert!(decode(&bad).is_err());
        let large = temp.path().join("large.png");
        let file = std::fs::File::create(&large).unwrap();
        file.set_len(64 * 1024 * 1024 + 1).unwrap();
        assert!(decode(&large).is_err());
    }
    #[test]
    fn fitting_is_centered_cropped_or_letterboxed_without_dpi_assumptions() {
        let bounds = Bounds::new(point(px(0.), px(0.)), size(px(100.), px(100.)));
        let dimensions = size(DevicePixels(200), DevicePixels(100));
        let cover = ObjectFit::Cover.get_bounds(bounds, dimensions);
        assert_eq!(cover.size, size(px(200.), px(100.)));
        assert_eq!(cover.origin.x, px(-50.));
        let contain = ObjectFit::Contain.get_bounds(bounds, dimensions);
        assert_eq!(contain.size, size(px(100.), px(50.)));
        assert_eq!(contain.origin.y, px(25.));
        assert_eq!(ObjectFit::Fill.get_bounds(bounds, dimensions), bounds);
    }
}
