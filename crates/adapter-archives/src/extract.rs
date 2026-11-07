//! Reading archives into events, one format per function.

use std::fs::File;
use std::io::{BufReader, Read};
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

use application::ports::{ArchiveError, ArchiveEvent, SkipReason};
use domain::{ArchiveFormat, ByteSize, UnixMode, entry_path};

use crate::{CHUNK, Failure, Outbox, corrupt, io_failure};

/// File-type bits of a Unix mode, and the value for symbolic links.
const FILE_TYPE_MASK: u32 = 0o170_000;
const SYMLINK_TYPE: u32 = 0o120_000;
/// 7z and RAR keep a Unix mode in the high half of the attributes when
/// this bit is set (p7zip's convention).
const UNIX_EXTENSION_BIT: u32 = 0x8000;
const UNIX_MODE_SHIFT: u32 = 16;

pub(crate) fn extract(archive: &Path, format: ArchiveFormat, outbox: &Outbox<ArchiveEvent>) -> Result<(), Failure> {
    match format {
        ArchiveFormat::Zip => zip(archive, outbox),
        ArchiveFormat::SevenZ => seven_z(archive, outbox),
        ArchiveFormat::Rar => rar(archive, outbox),
        tar_format => tar(archive, tar_format, outbox),
    }
}

/// Emits an entry's events: `Skipped` for unsafe paths, else `File`, its
/// chunks and `EndFile`.
fn emit_file(
    raw_name: &[u8],
    size: Option<u64>,
    mode: Option<u32>,
    data: &mut dyn Read,
    format: ArchiveFormat,
    outbox: &Outbox<ArchiveEvent>,
) -> Result<(), Failure> {
    let path = match entry_path(raw_name) {
        Ok(path) => path,
        Err(reason) => return skip(raw_name, SkipReason::Unsafe(reason), outbox),
    };
    let mode = mode.map(UnixMode::from_mode);
    // Modification times aren't restored: writers can't set them (ADR 0027).
    outbox.send(ArchiveEvent::File { path, size: size.map(ByteSize), mode, modified: None })?;
    let mut buffer = vec![0_u8; CHUNK];
    loop {
        let read = data.read(&mut buffer).map_err(corrupt(format))?;
        if read == 0 {
            break;
        }
        outbox.send(ArchiveEvent::Chunk(buffer[..read].to_vec()))?;
    }
    Ok(outbox.send(ArchiveEvent::EndFile)?)
}

fn emit_folder(raw_name: &[u8], outbox: &Outbox<ArchiveEvent>) -> Result<(), Failure> {
    match entry_path(raw_name) {
        Ok(path) => Ok(outbox.send(ArchiveEvent::Folder(path))?),
        Err(reason) => skip(raw_name, SkipReason::Unsafe(reason), outbox),
    }
}

fn skip(raw_name: &[u8], reason: SkipReason, outbox: &Outbox<ArchiveEvent>) -> Result<(), Failure> {
    let name = String::from_utf8_lossy(raw_name).into_owned();
    Ok(outbox.send(ArchiveEvent::Skipped { name, reason })?)
}

fn open(archive: &Path) -> Result<File, Failure> {
    File::open(archive).map_err(|e| io_failure(&e))
}

fn zip(archive: &Path, outbox: &Outbox<ArchiveEvent>) -> Result<(), Failure> {
    let format = ArchiveFormat::Zip;
    let mut zip = zip::ZipArchive::new(BufReader::new(open(archive)?))
        .map_err(|e| ArchiveError::Corrupt(format, e.to_string()))?;
    for index in 0..zip.len() {
        let mut entry = match zip.by_index(index) {
            Ok(entry) => entry,
            Err(zip::result::ZipError::UnsupportedArchive(message)) if message.contains("assword") => {
                return Err(ArchiveError::Encrypted.into());
            }
            Err(error) => return Err(ArchiveError::Corrupt(format, error.to_string()).into()),
        };
        if entry.encrypted() {
            return Err(ArchiveError::Encrypted.into());
        }
        let name = entry.name_raw().to_vec();
        if entry.is_dir() {
            emit_folder(&name, outbox)?;
        } else if entry.is_symlink() {
            skip(&name, SkipReason::NotARegularFile, outbox)?;
        } else {
            let (size, mode) = (entry.size(), entry.unix_mode());
            emit_file(&name, Some(size), mode, &mut entry, format, outbox)?;
        }
    }
    Ok(())
}

fn tar(archive: &Path, format: ArchiveFormat, outbox: &Outbox<ArchiveEvent>) -> Result<(), Failure> {
    let file = BufReader::new(open(archive)?);
    let decoder: Box<dyn Read> = match format {
        ArchiveFormat::TarGz => Box::new(flate2::read::MultiGzDecoder::new(file)),
        ArchiveFormat::TarBz2 => Box::new(bzip2::read::MultiBzDecoder::new(file)),
        ArchiveFormat::TarXz => Box::new(liblzma::read::XzDecoder::new_multi_decoder(file)),
        ArchiveFormat::TarZst => Box::new(zstd::stream::read::Decoder::new(file).map_err(corrupt(format))?),
        _ => Box::new(file),
    };
    let mut tar = tar::Archive::new(decoder);
    for entry in tar.entries().map_err(corrupt(format))? {
        let mut entry = entry.map_err(corrupt(format))?;
        let name = entry.path_bytes().into_owned();
        let kind = entry.header().entry_type();
        if kind.is_dir() {
            emit_folder(&name, outbox)?;
        } else if kind.is_file() {
            let (size, mode) = (entry.size(), entry.header().mode().ok());
            emit_file(&name, Some(size), mode, &mut entry, format, outbox)?;
        } else if !(kind.is_pax_global_extensions() || kind.is_pax_local_extensions() || kind.is_gnu_longname()) {
            skip(&name, SkipReason::NotARegularFile, outbox)?;
        }
    }
    Ok(())
}

fn unix_mode(attributes: u32) -> Option<u32> {
    (attributes & UNIX_EXTENSION_BIT != 0).then_some(attributes >> UNIX_MODE_SHIFT)
}

fn seven_z(archive: &Path, outbox: &Outbox<ArchiveEvent>) -> Result<(), Failure> {
    use sevenz_rust2::{ArchiveReader, Error, Password};
    let format = ArchiveFormat::SevenZ;
    let to_failure = |error: Error| match error {
        Error::PasswordRequired | Error::MaybeBadPassword(_) => Failure::Error(ArchiveError::Encrypted),
        other => Failure::Error(ArchiveError::Corrupt(format, other.to_string())),
    };
    let mut reader = ArchiveReader::new(BufReader::new(open(archive)?), Password::empty()).map_err(to_failure)?;
    // The callback can only return the library's error; remember why we stopped.
    let mut stopped: Option<Failure> = None;
    let result = reader.for_each_entries(|entry, data| {
        let name = entry.name.as_bytes();
        let mode = if entry.has_windows_attributes { unix_mode(entry.windows_attributes) } else { None };
        let outcome = if entry.is_directory {
            emit_folder(name, outbox)
        } else if mode.is_some_and(|m| m & FILE_TYPE_MASK == SYMLINK_TYPE) {
            skip(name, SkipReason::NotARegularFile, outbox)
        } else {
            emit_file(name, Some(entry.size), mode, data, format, outbox)
        };
        match outcome {
            Ok(()) => Ok(true),
            Err(failure) => {
                stopped = Some(failure);
                Err(Error::Other("stopped".into()))
            }
        }
    });
    match (stopped, result) {
        (Some(failure), _) => Err(failure),
        (None, Err(error)) => Err(to_failure(error)),
        (None, Ok(())) => Ok(()),
    }
}

fn rar(archive: &Path, outbox: &Outbox<ArchiveEvent>) -> Result<(), Failure> {
    let format = ArchiveFormat::Rar;
    let to_failure = |error: unrar::error::UnrarError| Failure::Error(ArchiveError::Corrupt(format, error.to_string()));
    let spool_dir = tempfile::tempdir().map_err(|e| io_failure(&e))?;
    let spool = spool_dir.path().join("entry");
    let mut cursor = unrar::Archive::new(archive).open_for_processing().map_err(to_failure)?;
    while let Some(header) = cursor.read_header().map_err(to_failure)? {
        let entry = header.entry();
        if entry.is_encrypted() {
            return Err(ArchiveError::Encrypted.into());
        }
        let name = entry.filename.as_os_str().as_bytes().to_vec();
        let mode = unix_mode(entry.file_attr).or(Some(entry.file_attr)).filter(|m| *m & FILE_TYPE_MASK != 0);
        if entry.is_directory() {
            emit_folder(&name, outbox)?;
            cursor = header.skip().map_err(to_failure)?;
        } else if mode.is_some_and(|m| m & FILE_TYPE_MASK == SYMLINK_TYPE) {
            skip(&name, SkipReason::NotARegularFile, outbox)?;
            cursor = header.skip().map_err(to_failure)?;
        } else {
            // unRAR writes files, not streams: spool each entry, then stream it.
            let size = entry.unpacked_size;
            cursor = header.extract_to(&spool).map_err(to_failure)?;
            let mut data = File::open(&spool).map_err(|e| io_failure(&e))?;
            emit_file(&name, Some(size), mode, &mut data, format, outbox)?;
            std::fs::remove_file(&spool).map_err(|e| io_failure(&e))?;
        }
    }
    Ok(())
}
