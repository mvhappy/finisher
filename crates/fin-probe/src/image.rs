//! PNG, JPEG and WebP: the pixel size from the header only (`imagesize`).

use std::fs::File;
use std::io::BufReader;
use std::path::Path;

use fin_core::{AssetKind, CoreError, ImageInfo};
use imagesize::ImageType;

use crate::{MAX_DIMENSION, Probe, unsupported};

pub(crate) fn probe(file: &mut File, path: &Path) -> Result<Probe, CoreError> {
    let mut reader = BufReader::new(file);
    let bad = |e: imagesize::ImageError| unsupported(path, &format!("image header: {e}"));
    let kind = imagesize::reader_type(&mut reader).map_err(bad)?;
    if !matches!(kind, ImageType::Png | ImageType::Jpeg | ImageType::Webp) {
        return Err(unsupported(path, "unknown image type"));
    }
    std::io::Seek::rewind(&mut reader).map_err(|e| crate::io_error(path, &e))?;
    let size = imagesize::reader_size(&mut reader).map_err(bad)?;
    let dim = |v: usize| u32::try_from(v).ok().filter(|v| *v > 0);
    let (Some(width), Some(height)) = (dim(size.width), dim(size.height)) else {
        return Err(unsupported(path, "image has no pixels"));
    };
    if width > MAX_DIMENSION || height > MAX_DIMENSION {
        return Err(unsupported(path, &crate::too_large(width, height)));
    }
    Ok(Probe {
        image: Some(ImageInfo { width, height }),
        ..Probe::of(AssetKind::Image)
    })
}
