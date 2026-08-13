//! Clipboard / file image attachments for multimodal requests.

use crate::providers::ImageAttachment;
use base64::Engine;
use image::imageops::FilterType;
use image::{DynamicImage, ImageFormat};

pub const MAX_IMAGE_SIDE: u32 = 1568;

pub fn from_rgba(width: u32, height: u32, rgba: &[u8]) -> Option<ImageAttachment> {
    if width == 0 || height == 0 || rgba.is_empty() {
        return None;
    }
    let img = DynamicImage::ImageRgba8(image::RgbaImage::from_raw(width, height, rgba.to_vec())?);
    encode(img)
}

pub fn from_bytes(bytes: &[u8]) -> Option<ImageAttachment> {
    let img = image::load_from_memory(bytes).ok()?;
    encode(img)
}

pub fn from_path(path: &std::path::Path) -> Option<ImageAttachment> {
    from_bytes(&std::fs::read(path).ok()?)
}

pub fn from_clipboard() -> Option<ImageAttachment> {
    let mut clipboard = arboard::Clipboard::new().ok()?;
    let img = clipboard.get_image().ok()?;
    from_rgba(img.width as u32, img.height as u32, &img.bytes)
}

fn encode(mut img: DynamicImage) -> Option<ImageAttachment> {
    let (w, h) = (img.width(), img.height());
    if w.max(h) > MAX_IMAGE_SIDE {
        img = img.resize(MAX_IMAGE_SIDE, MAX_IMAGE_SIDE, FilterType::Triangle);
    }
    let mut png = Vec::new();
    img.write_to(&mut std::io::Cursor::new(&mut png), ImageFormat::Png)
        .ok()?;
    Some(ImageAttachment {
        mime: "image/png".into(),
        data: base64::engine::general_purpose::STANDARD.encode(png),
        width: img.width(),
        height: img.height(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resizes_oversized_images() {
        let img = DynamicImage::new_rgb8(2000, 1000);
        let att = encode(img).unwrap();
        assert!(att.width.max(att.height) <= MAX_IMAGE_SIDE);
        assert_eq!(att.mime, "image/png");
        assert!(!att.data.is_empty());
    }
}
