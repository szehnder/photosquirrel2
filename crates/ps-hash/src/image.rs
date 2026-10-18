//! The input of perceptual hashing: an already-decoded pixel buffer in stored orientation plus its EXIF
//! orientation. Decoding and downscaling belong to `ps-media`; this crate only borrows the pixels.

use std::fmt;

use ps_model::Orientation;

/// Byte layout of one pixel. 8 bits per channel, rows tightly packed, top row first.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PixelLayout {
    /// One luminance byte per pixel.
    Luma8,
    /// Red, green, blue.
    Rgb8,
    /// Red, green, blue, alpha (alpha is ignored).
    Rgba8,
}

impl PixelLayout {
    pub fn channels(self) -> usize {
        match self {
            PixelLayout::Luma8 => 1,
            PixelLayout::Rgb8 => 3,
            PixelLayout::Rgba8 => 4,
        }
    }
}

/// Why an [`ImageView`] could not be built.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ImageError {
    /// Width or height is zero or above [`ImageView::MAX_DIMENSION`].
    BadDimensions { width: u32, height: u32 },
    /// The buffer length is not `width * height * channels`.
    BadLength { expected: usize, actual: usize },
}

impl fmt::Display for ImageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ImageError::BadDimensions { width, height } => {
                write!(f, "unsupported image dimensions {width}x{height}")
            }
            ImageError::BadLength { expected, actual } => {
                write!(f, "pixel buffer has {actual} bytes, expected {expected}")
            }
        }
    }
}

impl std::error::Error for ImageError {}

/// A borrowed, decoded image in *stored* orientation; [`ImageView::orientation`] says how to display it.
/// Hashes are computed on the displayed (oriented) image.
#[derive(Clone, Copy, Debug)]
pub struct ImageView<'a> {
    width: u32,
    height: u32,
    layout: PixelLayout,
    data: &'a [u8],
    orientation: Orientation,
}

impl<'a> ImageView<'a> {
    /// Largest supported width or height. Keeps the integer resampling accumulators within `u32`.
    pub const MAX_DIMENSION: u32 = 65_535;

    /// Wrap a tightly packed pixel buffer (upright orientation).
    pub fn new(
        width: u32,
        height: u32,
        layout: PixelLayout,
        data: &'a [u8],
    ) -> Result<Self, ImageError> {
        if width == 0 || height == 0 || width > Self::MAX_DIMENSION || height > Self::MAX_DIMENSION
        {
            return Err(ImageError::BadDimensions { width, height });
        }
        let expected = width as usize * height as usize * layout.channels();
        if data.len() != expected {
            return Err(ImageError::BadLength {
                expected,
                actual: data.len(),
            });
        }
        Ok(Self {
            width,
            height,
            layout,
            data,
            orientation: Orientation::UPRIGHT,
        })
    }

    /// The EXIF orientation to apply before hashing. Values outside 1-8 are treated as upright.
    pub fn with_orientation(mut self, orientation: Orientation) -> Self {
        self.orientation = orientation;
        self
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    pub fn layout(&self) -> PixelLayout {
        self.layout
    }

    pub fn data(&self) -> &'a [u8] {
        self.data
    }

    pub fn orientation(&self) -> Orientation {
        self.orientation
    }
}
