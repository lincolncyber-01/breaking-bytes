//! A dimensão é inspecionada antes da alocação do bitmap.

use crate::ImagingError;
use image::{ImageDecoder, ImageFormat, ImageReader, RgbaImage};
use std::io::Cursor;

#[derive(Debug, Clone, Copy)]
pub struct Limits {
    pub max_bytes: usize,
    pub max_pixels: u64,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            max_bytes: 8 * 1024 * 1024,
            max_pixels: 16_000_000,
        }
    }
}
impl Limits {
    pub fn validate(self) -> Result<(), ImagingError> {
        if !(1024..=8 * 1024 * 1024).contains(&self.max_bytes)
            || !(1_000_000..=16_000_000).contains(&self.max_pixels)
        {
            return Err(ImagingError::Configuration);
        }
        Ok(())
    }
}

pub fn decode(bytes: &[u8], mime: &str, limits: Limits) -> Result<RgbaImage, ImagingError> {
    limits.validate()?;
    if bytes.len() > limits.max_bytes {
        return Err(ImagingError::Bytes);
    }
    let declared = match mime.split(';').next().map(str::trim) {
        Some("image/jpeg") => ImageFormat::Jpeg,
        Some("image/png") => ImageFormat::Png,
        Some("image/webp") => ImageFormat::WebP,
        _ => return Err(ImagingError::Mime),
    };
    let format = image::guess_format(bytes).map_err(|_| ImagingError::Mime)?;
    if format != declared {
        return Err(ImagingError::Mime);
    }
    let mut reader = ImageReader::with_format(Cursor::new(bytes), format);
    let mut allocation = image::Limits::default();
    allocation.max_image_width = Some(12000);
    allocation.max_image_height = Some(12000);
    allocation.max_alloc = Some(96 * 1024 * 1024);
    // PNG pode alocar buffers durante a leitura do cabeçalho: limitar desde o início.
    reader.limits(allocation);
    let mut decoder = reader.into_decoder().map_err(|_| ImagingError::Decode)?;
    let (width, height) = decoder.dimensions();
    if width < 1000
        || height == 0
        || width > 12000
        || height > 12000
        || u64::from(width) * u64::from(height) > limits.max_pixels
    {
        return Err(ImagingError::Dimensions);
    }
    if decoder.total_bytes() > 96 * 1024 * 1024 {
        return Err(ImagingError::Dimensions);
    }
    let orientation = decoder.orientation().map_err(|_| ImagingError::Decode)?;
    let mut decoded =
        image::DynamicImage::from_decoder(decoder).map_err(|_| ImagingError::Decode)?;
    decoded.apply_orientation(orientation);
    if decoded.width() < 1000 {
        return Err(ImagingError::Dimensions);
    }
    Ok(decoded.into_rgba8())
}
