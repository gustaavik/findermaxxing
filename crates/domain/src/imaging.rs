//! Converting and resizing images, and which image metadata gives away
//! something about the person who took it.

use crate::error::PathError;
use crate::path::PathSegment;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ImageFormat {
    Jpeg,
    Png,
    Webp,
    Tiff,
    Bmp,
    Gif,
}

impl ImageFormat {
    pub const ALL: [Self; 6] = [Self::Jpeg, Self::Png, Self::Webp, Self::Tiff, Self::Bmp, Self::Gif];

    #[must_use]
    pub const fn extension(self) -> &'static str {
        match self {
            Self::Jpeg => "jpg",
            Self::Png => "png",
            Self::Webp => "webp",
            Self::Tiff => "tiff",
            Self::Bmp => "bmp",
            Self::Gif => "gif",
        }
    }

    #[must_use]
    pub fn of(name: &PathSegment) -> Option<Self> {
        let extension = name.stem_and_extension().1.map(|e| String::from_utf8_lossy(e).to_lowercase())?;
        match extension.as_str() {
            "jpg" | "jpeg" => Some(Self::Jpeg),
            "png" => Some(Self::Png),
            "webp" => Some(Self::Webp),
            "tif" | "tiff" => Some(Self::Tiff),
            "bmp" => Some(Self::Bmp),
            "gif" => Some(Self::Gif),
            _ => None,
        }
    }

    /// Formats whose encoder takes a quality setting.
    #[must_use]
    pub const fn is_lossy(self) -> bool {
        matches!(self, Self::Jpeg)
    }

    /// Formats that can carry EXIF the cleaner knows how to remove.
    #[must_use]
    pub const fn can_clean_metadata(self) -> bool {
        matches!(self, Self::Jpeg | Self::Png | Self::Webp)
    }
}

/// How big the result should be. Never enlarges.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resize {
    Keep,
    /// Percent of the original (1–100).
    Percent(u32),
    /// Fit inside this box, keeping the aspect ratio.
    Fit {
        width: u32,
        height: u32,
    },
}

impl Resize {
    /// The output size for an image of `width` × `height`.
    #[must_use]
    pub fn applied_to(self, width: u32, height: u32) -> (u32, u32) {
        let scale = match self {
            Self::Keep => 1.0,
            Self::Percent(percent) => f64::from(percent.clamp(1, 100)) / 100.0,
            Self::Fit { width: max_w, height: max_h } => {
                let by_width = f64::from(max_w.max(1)) / f64::from(width.max(1));
                let by_height = f64::from(max_h.max(1)) / f64::from(height.max(1));
                by_width.min(by_height).min(1.0)
            }
        };
        (scaled(width, scale), scaled(height, scale))
    }
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss, reason = "scale is in (0, 1], the result fits")]
fn scaled(length: u32, scale: f64) -> u32 {
    ((f64::from(length) * scale).round() as u32).max(1)
}

/// JPEG quality used unless the user picks another (1–100).
pub const DEFAULT_JPEG_QUALITY: u8 = 85;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConversionRequest {
    pub format: ImageFormat,
    pub resize: Resize,
    /// Used by lossy formats only.
    pub quality: u8,
}

/// `IMG_1234.HEIC` converted to PNG is `IMG_1234.png`.
///
/// # Errors
/// Never in practice; the stem comes from a valid name.
pub fn converted_name(original: &PathSegment, format: ImageFormat) -> Result<PathSegment, PathError> {
    let (stem, extension) = original.stem_and_extension();
    let stem = if extension.is_some() && !stem.is_empty() { stem } else { original.as_bytes() };
    let mut name = stem.to_vec();
    name.push(b'.');
    name.extend_from_slice(format.extension().as_bytes());
    PathSegment::from_bytes(&name)
}

/// What a metadata field gives away.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Sensitivity {
    /// Where the photo was taken.
    Location,
    /// Who took it or owns the camera (names, serial numbers).
    Identity,
    /// Camera, settings, dates.
    Other,
}

const LOCATION_TAGS: &[&str] = &["gps"];
const IDENTITY_TAGS: &[&str] = &[
    "artist",
    "copyright",
    "ownername",
    "cameraownername",
    "serialnumber",
    "bodyserialnumber",
    "lensserialnumber",
    "imageuniqueid",
    "usercomment",
    "xpauthor",
    "hostcomputer",
];

/// Classifies a tag by name (as the EXIF reader names them).
#[must_use]
pub fn sensitivity_of(tag: &str) -> Sensitivity {
    let tag = tag.to_ascii_lowercase();
    if LOCATION_TAGS.iter().any(|prefix| tag.starts_with(prefix)) {
        Sensitivity::Location
    } else if IDENTITY_TAGS.contains(&tag.as_str()) {
        Sensitivity::Identity
    } else {
        Sensitivity::Other
    }
}

/// One field of an image's metadata, ready to show.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetadataField {
    /// EXIF directory: "Camera", "Exif", "GPS", "Thumbnail", …
    pub group: String,
    pub tag: String,
    pub value: String,
    pub sensitivity: Sensitivity,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seg(name: &str) -> PathSegment {
        PathSegment::new(name).unwrap()
    }

    #[test]
    fn resizing_never_enlarges_and_keeps_the_ratio() {
        assert_eq!(Resize::Keep.applied_to(4000, 3000), (4000, 3000));
        assert_eq!(Resize::Percent(50).applied_to(4000, 3000), (2000, 1500));
        assert_eq!(Resize::Fit { width: 1920, height: 1920 }.applied_to(4000, 3000), (1920, 1440));
        assert_eq!(Resize::Fit { width: 1920, height: 1080 }.applied_to(800, 600), (800, 600));
        assert_eq!(Resize::Percent(1).applied_to(10, 10), (1, 1), "never zero");
    }

    #[test]
    fn converted_names_swap_the_extension() {
        assert_eq!(converted_name(&seg("IMG_1.HEIC"), ImageFormat::Png).unwrap(), seg("IMG_1.png"));
        assert_eq!(converted_name(&seg("scan"), ImageFormat::Jpeg).unwrap(), seg("scan.jpg"));
        assert_eq!(ImageFormat::of(&seg("a.JPEG")), Some(ImageFormat::Jpeg));
        assert_eq!(ImageFormat::of(&seg("a.heic")), None);
    }

    #[test]
    fn location_and_identity_are_flagged() {
        assert_eq!(sensitivity_of("GPSLatitude"), Sensitivity::Location);
        assert_eq!(sensitivity_of("BodySerialNumber"), Sensitivity::Identity);
        assert_eq!(sensitivity_of("Artist"), Sensitivity::Identity);
        assert_eq!(sensitivity_of("ExposureTime"), Sensitivity::Other);
    }
}
