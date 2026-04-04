#![allow(dead_code)]

use std::path::Path;

use base64::Engine;
use image::ImageFormat;

pub async fn write_png(path: &Path, width: u32, height: u32) {
    write_image(path, width, height, ImageFormat::Png).await;
}

pub async fn write_image(path: &Path, width: u32, height: u32, format: ImageFormat) {
    let image = image::DynamicImage::new_rgba8(width, height);
    image.save_with_format(path, format).unwrap();
}

pub async fn write_invalid_bytes(path: &Path) {
    tokio::fs::write(path, b"not an image").await.unwrap();
}

pub fn decode_data_url(image_url: &str) -> (String, Vec<u8>) {
    let (metadata, data) = image_url.split_once(',').unwrap();
    let mime = metadata
        .strip_prefix("data:")
        .unwrap()
        .strip_suffix(";base64")
        .unwrap();
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(data)
        .unwrap();
    (mime.to_string(), bytes)
}
