use std::fs::File;
use std::io::{BufReader, Read};
use std::path::Path;

use application::ports::{ArchiveInspector, ArchiveItem, ArchiveListing, LocalFile, PreviewFailure};
use domain::ByteSize;
use flate2::read::GzDecoder;
use futures::FutureExt;
use futures::future::BoxFuture;
use tokio::runtime::Handle;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Format {
    Zip,
    Tar,
    TarGz,
}

/// Lists zip and tar(.gz) archives without extracting anything. Other
/// formats (7z, rar, xz, zstd) arrive with the archive utilities in M7.
#[derive(Debug, Clone)]
pub struct LocalArchiveInspector {
    runtime: Handle,
}

impl LocalArchiveInspector {
    #[must_use]
    pub fn new(runtime: Handle) -> Self {
        Self { runtime }
    }
}

impl ArchiveInspector for LocalArchiveInspector {
    fn list(&self, file: &LocalFile, limit: usize) -> BoxFuture<'static, Result<ArchiveListing, PreviewFailure>> {
        let path = file.path().to_path_buf();
        let task = self.runtime.spawn_blocking(move || list_blocking(&path, limit));
        async move { task.await.map_err(|e| failure(&e))? }.boxed()
    }
}

fn list_blocking(path: &Path, limit: usize) -> Result<ArchiveListing, PreviewFailure> {
    let file = File::open(path).map_err(|e| failure(&e))?;
    match format_of(path) {
        Some(Format::Zip) => list_zip(file, limit),
        Some(Format::Tar) => list_tar(BufReader::new(file), limit),
        Some(Format::TarGz) => list_tar(GzDecoder::new(BufReader::new(file)), limit),
        None => Err(PreviewFailure { message: "Previews for this archive format arrive in a later version.".into() }),
    }
}

fn format_of(path: &Path) -> Option<Format> {
    let name = path.file_name()?.to_str()?.to_lowercase();
    if name.ends_with(".zip") || name.ends_with(".jar") {
        Some(Format::Zip)
    } else if name.ends_with(".tar.gz") || name.ends_with(".tgz") {
        Some(Format::TarGz)
    } else if name.ends_with(".tar") {
        Some(Format::Tar)
    } else {
        None
    }
}

fn list_zip(file: File, limit: usize) -> Result<ArchiveListing, PreviewFailure> {
    let mut archive = zip::ZipArchive::new(BufReader::new(file)).map_err(|e| failure(&e))?;
    let mut items = Vec::with_capacity(archive.len().min(limit));
    for index in 0..archive.len().min(limit) {
        // Raw access reads the central directory only; nothing is decompressed.
        let entry = archive.by_index_raw(index).map_err(|e| failure(&e))?;
        items.push(ArchiveItem {
            path: entry.name().map_err(|e| failure(&e))?.into_owned(),
            size: (!entry.is_dir()).then(|| ByteSize(entry.size())),
            is_folder: entry.is_dir(),
        });
    }
    Ok(ArchiveListing { truncated: archive.len() > limit, items })
}

fn list_tar(reader: impl Read, limit: usize) -> Result<ArchiveListing, PreviewFailure> {
    let mut archive = tar::Archive::new(reader);
    let mut items = Vec::new();
    for entry in archive.entries().map_err(|e| failure(&e))? {
        if items.len() == limit {
            return Ok(ArchiveListing { items, truncated: true });
        }
        let entry = entry.map_err(|e| failure(&e))?;
        let is_folder = entry.header().entry_type().is_dir();
        let path = entry.path().map_err(|e| failure(&e))?.to_string_lossy().into_owned();
        items.push(ArchiveItem { path, size: (!is_folder).then(|| ByteSize(entry.size())), is_folder });
    }
    Ok(ArchiveListing { items, truncated: false })
}

fn failure(error: &dyn std::fmt::Display) -> PreviewFailure {
    PreviewFailure { message: error.to_string() }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tar_bytes() -> Vec<u8> {
        let mut builder = tar::Builder::new(Vec::new());
        let mut header = tar::Header::new_gnu();
        header.set_size(5);
        header.set_cksum();
        builder.append_data(&mut header, "docs/hello.txt", &b"hello"[..]).unwrap();
        builder.into_inner().unwrap()
    }

    #[test]
    fn lists_tar_and_tar_gz() {
        let dir = tempfile::tempdir().unwrap();
        let tar_path = dir.path().join("a.tar");
        std::fs::write(&tar_path, tar_bytes()).unwrap();
        let gz_path = dir.path().join("a.tgz");
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        std::io::Write::write_all(&mut encoder, &tar_bytes()).unwrap();
        std::fs::write(&gz_path, encoder.finish().unwrap()).unwrap();

        for path in [tar_path, gz_path] {
            let listing = list_blocking(&path, 10).unwrap();
            assert_eq!(
                listing.items,
                vec![ArchiveItem { path: "docs/hello.txt".into(), size: Some(ByteSize(5)), is_folder: false }]
            );
        }
    }

    #[test]
    fn zip_listing_respects_the_limit() {
        // A minimal empty zip (end-of-central-directory record only).
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("empty.zip");
        let mut eocd = vec![0x50, 0x4b, 0x05, 0x06];
        eocd.extend([0_u8; 18]);
        std::fs::write(&path, eocd).unwrap();
        assert_eq!(list_blocking(&path, 10).unwrap(), ArchiveListing { items: vec![], truncated: false });
    }

    #[test]
    fn unsupported_formats_say_so() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("x.7z");
        std::fs::write(&path, b"7z").unwrap();
        assert!(list_blocking(&path, 10).is_err());
    }
}
