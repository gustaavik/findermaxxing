//! Real images: conversion and resizing (with EXIF orientation), metadata
//! read and stripped without touching pixels or the colour profile, and
//! fingerprints that match resized copies but not other pictures.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, reason = "test code")]

use std::io::Cursor;
use std::path::{Path, PathBuf};

use adapter_imaging::LocalImageTools;
use application::ports::{ImageConverter, ImageFingerprinter, ImageMetadata, LocalFile};
use domain::{ConversionRequest, ImageFormat, Resize, Sensitivity};
use image::{DynamicImage, GenericImageView, Rgb, RgbImage};
use img_parts::jpeg::{Jpeg, JpegSegment, markers};

fn gradient(width: u32, height: u32, seed: u32) -> DynamicImage {
    DynamicImage::ImageRgb8(RgbImage::from_fn(width, height, |x, y| {
        Rgb([(x * 255 / width) as u8, (y * 255 / height) as u8, ((x * y + seed * 97) % 251) as u8])
    }))
}

fn checkerboard(width: u32, height: u32) -> DynamicImage {
    DynamicImage::ImageRgb8(RgbImage::from_fn(width, height, |x, y| {
        if (x / 16 + y / 16) % 2 == 0 { Rgb([250, 250, 250]) } else { Rgb([5, 5, 5]) }
    }))
}

fn jpeg_bytes(image: &DynamicImage) -> Vec<u8> {
    let mut out = Cursor::new(Vec::new());
    image.to_rgb8().write_to(&mut out, image::ImageFormat::Jpeg).unwrap();
    out.into_inner()
}

/// EXIF with a camera make, an orientation and a GPS latitude reference.
fn exif_segment(orientation: u16) -> Vec<u8> {
    use exif::{Field, In, Tag, Value};
    let make = Field { tag: Tag::Make, ifd_num: In::PRIMARY, value: Value::Ascii(vec![b"FakeCam".to_vec()]) };
    let turn = Field { tag: Tag::Orientation, ifd_num: In::PRIMARY, value: Value::Short(vec![orientation]) };
    let gps = Field { tag: Tag::GPSLatitudeRef, ifd_num: In::PRIMARY, value: Value::Ascii(vec![b"N".to_vec()]) };
    let mut writer = exif::experimental::Writer::new();
    for field in [&make, &turn, &gps] {
        writer.push_field(field);
    }
    let mut tiff = Cursor::new(Vec::new());
    writer.write(&mut tiff, false).unwrap();
    let mut segment = b"Exif\0\0".to_vec();
    segment.extend(tiff.into_inner());
    segment
}

/// A JPEG with EXIF (APP1) and a stand-in colour profile (APP2).
fn photo_with_exif(dir: &Path, image: &DynamicImage, orientation: u16) -> PathBuf {
    let mut jpeg = Jpeg::from_bytes(jpeg_bytes(image).into()).unwrap();
    let icc = JpegSegment::new_with_contents(markers::APP2, b"ICC_PROFILE\0\x01\x01fake".to_vec().into());
    let app1 = JpegSegment::new_with_contents(markers::APP1, exif_segment(orientation).into());
    jpeg.segments_mut().insert(1, app1);
    jpeg.segments_mut().insert(2, icc);
    let path = dir.join("photo.jpg");
    std::fs::write(&path, jpeg.encoder().bytes()).unwrap();
    path
}

fn tools() -> LocalImageTools {
    LocalImageTools::new(tokio::runtime::Handle::current())
}

#[tokio::test(flavor = "multi_thread")]
async fn conversions_resize_and_honour_the_orientation() {
    let dir = tempfile::tempdir().unwrap();
    // Orientation 6: stored landscape, shown rotated 90° clockwise.
    let path = photo_with_exif(dir.path(), &gradient(400, 200, 1), 6);
    let request = ConversionRequest { format: ImageFormat::Png, resize: Resize::Percent(50), quality: 85 };
    let png = tools().convert(LocalFile::new(path.clone()), request).await.unwrap();
    let decoded = image::load_from_memory(&png).unwrap();
    assert_eq!(decoded.dimensions(), (100, 200), "upright, then halved");

    for format in ImageFormat::ALL {
        let request = ConversionRequest { format, resize: Resize::Fit { width: 64, height: 64 }, quality: 70 };
        let bytes = tools().convert(LocalFile::new(path.clone()), request).await.unwrap();
        let decoded = image::load_from_memory(&bytes).unwrap_or_else(|e| panic!("{format:?}: {e}"));
        assert_eq!(decoded.dimensions(), (32, 64), "{format:?}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn metadata_is_read_with_sensitivity_and_stripped_losslessly() {
    let dir = tempfile::tempdir().unwrap();
    let path = photo_with_exif(dir.path(), &gradient(64, 48, 2), 1);
    let fields = tools().read(LocalFile::new(path.clone())).await.unwrap();
    let find = |tag: &str| fields.iter().find(|f| f.tag == tag).unwrap_or_else(|| panic!("{tag} in {fields:?}"));
    assert_eq!(find("Make").sensitivity, Sensitivity::Other);
    assert!(find("Make").value.contains("FakeCam"));
    assert_eq!(find("GPSLatitudeRef").sensitivity, Sensitivity::Location);
    assert_eq!(find("GPSLatitudeRef").group, "GPS");

    let original = std::fs::read(&path).unwrap();
    let stripped = tools().strip(LocalFile::new(path.clone())).await.unwrap();
    let cleaned = dir.path().join("clean.jpg");
    std::fs::write(&cleaned, &stripped).unwrap();
    assert!(tools().read(LocalFile::new(cleaned.clone())).await.unwrap().is_empty());
    let jpeg = Jpeg::from_bytes(stripped.clone().into()).unwrap();
    assert!(jpeg.segment_by_marker(markers::APP2).is_some(), "the colour profile stays");
    assert!(stripped.len() < original.len());
    let before = image::load_from_memory(&original).unwrap().to_rgb8();
    let after = image::load_from_memory(&stripped).unwrap().to_rgb8();
    assert_eq!(before, after, "pixels untouched");
}

#[tokio::test(flavor = "multi_thread")]
async fn png_text_chunks_are_removed() {
    let dir = tempfile::tempdir().unwrap();
    let mut out = Cursor::new(Vec::new());
    gradient(32, 32, 3).write_to(&mut out, image::ImageFormat::Png).unwrap();
    let mut png = img_parts::png::Png::from_bytes(out.into_inner().into()).unwrap();
    let text = img_parts::png::PngChunk::new(*b"tEXt", b"Author\0Someone".to_vec().into());
    let position = png.chunks().len() - 1;
    png.chunks_mut().insert(position, text);
    let path = dir.path().join("a.png");
    std::fs::write(&path, png.encoder().bytes()).unwrap();
    let stripped = tools().strip(LocalFile::new(path)).await.unwrap();
    let png = img_parts::png::Png::from_bytes(stripped.clone().into()).unwrap();
    assert!(png.chunk_by_type(*b"tEXt").is_none());
    image::load_from_memory(&stripped).unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn fingerprints_match_resized_copies_but_not_other_pictures() {
    let dir = tempfile::tempdir().unwrap();
    let save = |name: &str, image: &DynamicImage| {
        let path = dir.path().join(name);
        image.save(&path).unwrap();
        LocalFile::new(path)
    };
    let original = gradient(640, 480, 4);
    let a = tools().fingerprint(save("a.png", &original)).await.unwrap();
    let small = original.resize(160, 120, image::imageops::FilterType::Triangle);
    let b = tools().fingerprint(save("b.jpg", &small)).await.unwrap();
    let c = tools().fingerprint(save("c.png", &checkerboard(640, 480))).await.unwrap();
    assert!(a.distance(&b) <= domain::SIMILAR_IMAGE_DISTANCE, "resized copy: {}", a.distance(&b));
    assert!(a.distance(&c) > domain::SIMILAR_IMAGE_DISTANCE, "other picture: {}", a.distance(&c));
    let text = dir.path().join("notes.txt");
    std::fs::write(&text, "not an image").unwrap();
    assert_eq!(tools().fingerprint(LocalFile::new(text)).await, None);
}
