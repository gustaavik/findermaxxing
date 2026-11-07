//! Image tools (ADR 0027):
//!
//! - `ImageConverter` on the `image` crate: decode (EXIF orientation
//!   applied), resize with Lanczos3, encode as JPEG, PNG, WebP (lossless),
//!   TIFF, BMP or GIF;
//! - `ImageFingerprinter`: an 8×8 gradient hash (`image_hasher`) of a small
//!   thumbnail, for "looks the same" duplicates;
//! - `ImageMetadata`: EXIF read with kamadak-exif; stripped losslessly with
//!   img-parts, removing EXIF, XMP, IPTC and comments but keeping the colour
//!   profile, so pixels and colours are untouched.
//!
//! Decoding is CPU work, so it runs on tokio's blocking pool.

use std::fs::File;
use std::io::{BufReader, Cursor};

use application::ports::{ImageConverter, ImageError, ImageFingerprinter, ImageMetadata, LocalFile};
use domain::{ConversionRequest, ImageFingerprint, ImageFormat, MetadataField, sensitivity_of};
use futures::FutureExt;
use futures::future::BoxFuture;
use image::{DynamicImage, ImageDecoder, ImageReader};
use tokio::runtime::Handle;

mod strip;

/// Fingerprints are taken from a thumbnail: fast, and resizing is exactly
/// what the hash must ignore anyway.
const FINGERPRINT_THUMBNAIL: u32 = 256;
const FINGERPRINT_SIZE: u32 = 8;

#[derive(Debug, Clone)]
pub struct LocalImageTools {
    runtime: Handle,
}

impl LocalImageTools {
    #[must_use]
    pub fn new(runtime: Handle) -> Self {
        Self { runtime }
    }

    fn blocking<T: Send + 'static>(
        &self,
        work: impl FnOnce() -> Result<T, ImageError> + Send + 'static,
    ) -> BoxFuture<'static, Result<T, ImageError>> {
        let task = self.runtime.spawn_blocking(work);
        async move { task.await.map_err(|e| ImageError::Unreadable(e.to_string()))? }.boxed()
    }
}

fn unreadable(error: &dyn std::fmt::Display) -> ImageError {
    ImageError::Unreadable(error.to_string())
}

/// Decodes with the EXIF orientation applied, so portrait photos stay upright.
pub(crate) fn decode(file: &LocalFile) -> Result<DynamicImage, ImageError> {
    let reader = ImageReader::open(file.path()).map_err(|e| unreadable(&e))?;
    let mut decoder =
        reader.with_guessed_format().map_err(|e| unreadable(&e))?.into_decoder().map_err(|e| match e {
            image::ImageError::Unsupported(_) => ImageError::Unsupported,
            other => unreadable(&other),
        })?;
    let orientation = decoder.orientation().map_err(|e| unreadable(&e))?;
    let mut image = DynamicImage::from_decoder(decoder).map_err(|e| unreadable(&e))?;
    image.apply_orientation(orientation);
    Ok(image)
}

fn encode(image: &DynamicImage, request: ConversionRequest) -> Result<Vec<u8>, ImageError> {
    let encoding = |e: image::ImageError| ImageError::Encoding(e.to_string());
    let mut out = Cursor::new(Vec::new());
    match request.format {
        ImageFormat::Jpeg => {
            // JPEG has no alpha channel.
            let encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, request.quality.clamp(1, 100));
            image.to_rgb8().write_with_encoder(encoder).map_err(encoding)?;
        }
        ImageFormat::Webp => {
            let encoder = image::codecs::webp::WebPEncoder::new_lossless(&mut out);
            image.to_rgba8().write_with_encoder(encoder).map_err(encoding)?;
        }
        ImageFormat::Gif => image.to_rgba8().write_to(&mut out, image::ImageFormat::Gif).map_err(encoding)?,
        ImageFormat::Png => image.write_to(&mut out, image::ImageFormat::Png).map_err(encoding)?,
        ImageFormat::Tiff => image.write_to(&mut out, image::ImageFormat::Tiff).map_err(encoding)?,
        ImageFormat::Bmp => image.to_rgba8().write_to(&mut out, image::ImageFormat::Bmp).map_err(encoding)?,
    }
    Ok(out.into_inner())
}

impl ImageConverter for LocalImageTools {
    fn convert(&self, image: LocalFile, request: ConversionRequest) -> BoxFuture<'static, Result<Vec<u8>, ImageError>> {
        self.blocking(move || {
            let decoded = decode(&image)?;
            let (width, height) = request.resize.applied_to(decoded.width(), decoded.height());
            let resized = if (width, height) == (decoded.width(), decoded.height()) {
                decoded
            } else {
                decoded.resize_exact(width, height, image::imageops::FilterType::Lanczos3)
            };
            encode(&resized, request)
        })
    }
}

impl ImageFingerprinter for LocalImageTools {
    fn fingerprint(&self, image: LocalFile) -> BoxFuture<'static, Option<ImageFingerprint>> {
        let hashed = self.blocking(move || {
            let thumbnail = decode(&image)?.thumbnail(FINGERPRINT_THUMBNAIL, FINGERPRINT_THUMBNAIL);
            let hasher = image_hasher::HasherConfig::new()
                .hash_alg(image_hasher::HashAlg::Gradient)
                .hash_size(FINGERPRINT_SIZE, FINGERPRINT_SIZE)
                .to_hasher();
            Ok(ImageFingerprint(hasher.hash_image(&thumbnail).as_bytes().to_vec()))
        });
        async move { hashed.await.ok() }.boxed()
    }
}

impl ImageMetadata for LocalImageTools {
    fn read(&self, image: LocalFile) -> BoxFuture<'static, Result<Vec<MetadataField>, ImageError>> {
        self.blocking(move || read_exif(&image))
    }

    fn strip(&self, image: LocalFile) -> BoxFuture<'static, Result<Vec<u8>, ImageError>> {
        self.blocking(move || {
            let bytes = std::fs::read(image.path()).map_err(|e| unreadable(&e))?;
            strip::strip(bytes)
        })
    }
}

fn read_exif(image: &LocalFile) -> Result<Vec<MetadataField>, ImageError> {
    let file = File::open(image.path()).map_err(|e| unreadable(&e))?;
    let exif = match exif::Reader::new().read_from_container(&mut BufReader::new(file)) {
        Ok(exif) => exif,
        Err(exif::Error::NotFound(_) | exif::Error::BlankValue(_)) => return Ok(Vec::new()),
        Err(error) => return Err(unreadable(&error)),
    };
    Ok(exif
        .fields()
        .map(|field| {
            let tag = field.tag.to_string();
            let group = if field.ifd_num == exif::In::THUMBNAIL {
                "Thumbnail"
            } else {
                match field.tag.context() {
                    exif::Context::Gps => "GPS",
                    exif::Context::Exif | exif::Context::Interop => "Exif",
                    _ => "Camera",
                }
            };
            MetadataField {
                group: group.to_owned(),
                value: field.display_value().with_unit(&exif).to_string(),
                sensitivity: sensitivity_of(&tag),
                tag,
            }
        })
        .collect())
}
