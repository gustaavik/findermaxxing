//! Real files of each format, built on the fly.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, reason = "test code")]

use std::io::Write;

use adapter_content_extract::extract;
use domain::FileCategory;

fn zipped(dir: &tempfile::TempDir, name: &str, members: &[(&str, &str)]) -> std::path::PathBuf {
    let path = dir.path().join(name);
    let mut writer = zip::ZipWriter::new(std::fs::File::create(&path).unwrap());
    for (member, content) in members {
        writer.start_file(*member, zip::write::SimpleFileOptions::default()).unwrap();
        writer.write_all(content.as_bytes()).unwrap();
    }
    writer.finish().unwrap();
    path
}

#[test]
fn office_and_opendocument_files_yield_their_text() {
    let dir = tempfile::tempdir().unwrap();
    let docx = zipped(
        &dir,
        "letter.docx",
        &[(
            "word/document.xml",
            r#"<w:document xmlns:w="w"><w:body><w:p><w:r><w:t>Faktura for</w:t></w:r><w:r><w:t xml:space="preserve"> elektricitet</w:t></w:r></w:p><w:p><w:r><w:t>Beløb: 1.200 kr</w:t></w:r></w:p></w:body></w:document>"#,
        )],
    );
    assert_eq!(extract(&docx, FileCategory::Document).unwrap().unwrap(), "Faktura for elektricitet\nBeløb: 1.200 kr");

    let pptx = zipped(
        &dir,
        "deck.pptx",
        &[
            ("ppt/slides/slide10.xml", r#"<p:sld xmlns:a="a" xmlns:p="p"><a:p><a:t>Tenth</a:t></a:p></p:sld>"#),
            ("ppt/slides/slide2.xml", r#"<p:sld xmlns:a="a" xmlns:p="p"><a:p><a:t>Second</a:t></a:p></p:sld>"#),
        ],
    );
    assert_eq!(extract(&pptx, FileCategory::Document).unwrap().unwrap(), "Second\nTenth", "slides in order");

    let odt = zipped(
        &dir,
        "notes.odt",
        &[(
            "content.xml",
            r#"<office:document xmlns:office="o" xmlns:text="t"><text:p>Mødereferat</text:p></office:document>"#,
        )],
    );
    assert_eq!(extract(&odt, FileCategory::Document).unwrap().unwrap(), "Mødereferat");
}

#[test]
fn text_files_and_things_without_text() {
    let dir = tempfile::tempdir().unwrap();
    let notes = dir.path().join("notes.md");
    std::fs::write(&notes, "# Title\n\n  Some   notes  ").unwrap();
    assert_eq!(extract(&notes, FileCategory::Text).unwrap().unwrap(), "# Title\nSome notes");
    let binary = dir.path().join("Cargo.lock");
    std::fs::write(&binary, [0_u8, 1, 2, 3]).unwrap();
    assert_eq!(extract(&binary, FileCategory::Text).unwrap(), None);
    let broken = dir.path().join("broken.pdf");
    std::fs::write(&broken, b"%PDF-1.4 not really").unwrap();
    assert!(extract(&broken, FileCategory::Pdf).is_err() || extract(&broken, FileCategory::Pdf).unwrap().is_none());
    assert_eq!(extract(&dir.path().join("song.mp3"), FileCategory::Audio).unwrap(), None);
}

#[test]
fn pdf_text_is_extracted() {
    use pdf_extract::{Document, Object, Stream, dictionary};
    let mut document = Document::with_version("1.5");
    let pages_id = document.new_object_id();
    let font_id = document.add_object(dictionary! {
        "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica",
    });
    let resources_id = document.add_object(dictionary! { "Font" => dictionary! { "F1" => font_id } });
    let content = b"BT /F1 24 Tf 72 700 Td (Invoice for electricity) Tj ET".to_vec();
    let content_id = document.add_object(Stream::new(dictionary! {}, content));
    let page_id = document.add_object(dictionary! {
        "Type" => "Page", "Parent" => pages_id, "Contents" => content_id, "Resources" => resources_id,
        "MediaBox" => vec![0.into(), 0.into(), 595.into(), 842.into()],
    });
    document.objects.insert(
        pages_id,
        Object::Dictionary(dictionary! {
            "Type" => "Pages", "Kids" => vec![page_id.into()], "Count" => 1,
        }),
    );
    let catalog_id = document.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
    document.trailer.set("Root", catalog_id);
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("invoice.pdf");
    document.save(&path).unwrap();
    let text = extract(&path, FileCategory::Pdf).unwrap().unwrap();
    assert!(text.contains("Invoice for electricity"), "{text:?}");
}
