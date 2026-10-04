use std::{io, sync::Arc};

use helix_view::graphics::Rect;

/// Pixel dimensions of a terminal cell.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct CellPixelSize { pub width: u16, pub height: u16 }

/// A rectangle in immutable source-image pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PixelRect { pub x: u32, pub y: u32, pub width: u32, pub height: u32 }

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ImageSourceId(pub u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ImagePlacementKey(pub u64, pub usize);

/// Immutable RGBA image data shared by all placements of a prepared raster.
#[derive(Debug)]
pub struct ImageSource { pub id: ImageSourceId, pub width: u32, pub height: u32, pub rgba: Box<[u8]> }

impl ImageSource {
    pub fn new(id: ImageSourceId, width: u32, height: u32, rgba: Box<[u8]>) -> io::Result<Self> {
        let bytes = usize::try_from(width).ok().and_then(|w| usize::try_from(height).ok()?.checked_mul(w))
            .and_then(|pixels| pixels.checked_mul(4));
        if width == 0 || height == 0 || bytes != Some(rgba.len()) {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "invalid RGBA image"));
        }
        Ok(Self { id, width, height, rgba })
    }
}

/// A visible crop of an immutable image source in terminal cells.
#[derive(Clone, Debug)]
pub struct ImagePlacement { pub key: ImagePlacementKey, pub source: Arc<ImageSource>, pub crop: PixelRect, pub cell_rect: Rect }
