//! Removing metadata without re-encoding: the container's metadata
//! segments or chunks are dropped, everything else is copied as it is.

use application::ports::ImageError;
use bytes::Bytes;
use img_parts::jpeg::{Jpeg, markers};
use img_parts::png::Png;
use img_parts::webp::WebP;

/// APP1 carries EXIF and XMP; APP13 Photoshop/IPTC data; COM comments.
/// APP2 (the ICC colour profile) and APP14 (Adobe colour transform) stay.
const JPEG_METADATA: [u8; 3] = [markers::APP1, markers::APP13, markers::COM];
/// EXIF, text chunks and the creation time; `iCCP` stays.
const PNG_METADATA: [[u8; 4]; 5] = [*b"eXIf", *b"tEXt", *b"zTXt", *b"iTXt", *b"tIME"];
const WEBP_METADATA: [[u8; 4]; 2] = [*b"EXIF", *b"XMP "];
/// The `VP8X` flags that announce EXIF and XMP chunks.
const VP8X_EXIF_AND_XMP_FLAGS: u8 = 0b0000_1100;

const JPEG_MAGIC: &[u8] = &[0xFF, 0xD8];
const PNG_MAGIC: &[u8] = b"\x89PNG";
const RIFF_MAGIC: &[u8] = b"RIFF";

pub(crate) fn strip(bytes: Vec<u8>) -> Result<Vec<u8>, ImageError> {
    let invalid = |e: img_parts::Error| ImageError::Unreadable(e.to_string());
    let bytes = Bytes::from(bytes);
    if bytes.starts_with(JPEG_MAGIC) {
        let mut jpeg = Jpeg::from_bytes(bytes).map_err(invalid)?;
        for marker in JPEG_METADATA {
            jpeg.remove_segments_by_marker(marker);
        }
        Ok(jpeg.encoder().bytes().to_vec())
    } else if bytes.starts_with(PNG_MAGIC) {
        let mut png = Png::from_bytes(bytes).map_err(invalid)?;
        for kind in PNG_METADATA {
            png.remove_chunks_by_type(kind);
        }
        Ok(png.encoder().bytes().to_vec())
    } else if bytes.starts_with(RIFF_MAGIC) {
        let mut webp = WebP::from_bytes(bytes).map_err(invalid)?;
        for id in WEBP_METADATA {
            webp.remove_chunks_by_id(id);
        }
        clear_vp8x_metadata_flags(&mut webp);
        Ok(webp.encoder().bytes().to_vec())
    } else {
        Err(ImageError::Unsupported)
    }
}

/// An extended WebP header that still announces EXIF/XMP after the chunks
/// are gone confuses some readers.
fn clear_vp8x_metadata_flags(webp: &mut WebP) {
    for chunk in webp.chunks_mut() {
        if chunk.id() != *b"VP8X" {
            continue;
        }
        if let Some(data) = chunk.content().data()
            && let Some((flags, rest)) = data.split_first()
        {
            let mut cleared = vec![flags & !VP8X_EXIF_AND_XMP_FLAGS];
            cleared.extend_from_slice(rest);
            *chunk.content_mut() = img_parts::riff::RiffContent::Data(Bytes::from(cleared));
        }
    }
}
