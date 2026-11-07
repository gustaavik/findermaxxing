//! Writing archives from events. The archive is built in an anonymous
//! temporary file (zip and 7z need to seek), then streamed out. tar and 7z
//! need each file's length up front, so their files are spooled first;
//! that also makes a file that changes while being packed harmless.

use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom, Write};

use application::ports::ArchiveEvent;
use domain::{ArchiveFormat, PathSegment, Timestamp, UnixMode};
use tokio::sync::mpsc;

use crate::{CHUNK, Failure, Outbox, io_failure};

/// Modes for entries whose source had none (e.g. from a bucket).
const DEFAULT_FILE_MODE: u32 = 0o644;
const DEFAULT_FOLDER_MODE: u32 = 0o755;
/// zip needs the 64-bit extension announced for files from this size on.
const ZIP64_THRESHOLD: u64 = u32::MAX as u64;
/// p7zip's way of storing a Unix mode in 7z attributes (see `extract.rs`).
const SEVEN_Z_UNIX_EXTENSION: u32 = 0x8000;
const SEVEN_Z_ARCHIVE_ATTRIBUTE: u32 = 0x20;
const SEVEN_Z_DIRECTORY_ATTRIBUTE: u32 = 0x10;
const XZ_LEVEL: u32 = 6;
/// zstd's default level.
const ZSTD_LEVEL: i32 = 3;

/// One item to add, with its data spooled or still arriving.
enum Item {
    Folder { name: String },
    File { name: String, mode: u32, data: File, length: u64, modified: Option<Timestamp> },
}

/// Receives the next item (folders immediately, files once all their chunks
/// are in), or `None` when the contents end.
fn next_item(inbox: &mut mpsc::Receiver<ArchiveEvent>) -> Result<Option<Item>, Failure> {
    loop {
        match inbox.blocking_recv() {
            None => return Ok(None),
            Some(ArchiveEvent::Folder(path)) => return Ok(Some(Item::Folder { name: joined(&path) })),
            Some(ArchiveEvent::File { path, mode, modified, .. }) => {
                let mut data = tempfile::tempfile().map_err(|e| io_failure(&e))?;
                let mut length = 0_u64;
                loop {
                    match inbox.blocking_recv() {
                        Some(ArchiveEvent::Chunk(bytes)) => {
                            data.write_all(&bytes).map_err(|e| io_failure(&e))?;
                            length += bytes.len() as u64;
                        }
                        Some(ArchiveEvent::EndFile) => break,
                        // Contents ended mid-file: the caller aborts anyway.
                        _ => return Ok(None),
                    }
                }
                data.seek(SeekFrom::Start(0)).map_err(|e| io_failure(&e))?;
                let mode = mode.map_or(DEFAULT_FILE_MODE, UnixMode::bits);
                return Ok(Some(Item::File { name: joined(&path), mode, data, length, modified }));
            }
            Some(_) => {}
        }
    }
}

fn joined(path: &[PathSegment]) -> String {
    path.iter().map(|s| s.display_name().into_owned()).collect::<Vec<_>>().join("/")
}

pub(crate) fn build(
    format: ArchiveFormat,
    mut inbox: mpsc::Receiver<ArchiveEvent>,
    outbox: &Outbox<Vec<u8>>,
) -> Result<(), Failure> {
    let output = tempfile::tempfile().map_err(|e| io_failure(&e))?;
    let mut finished = match format {
        ArchiveFormat::Zip => zip(output, &mut inbox, outbox),
        ArchiveFormat::SevenZ => seven_z(output, &mut inbox, outbox),
        _ => tar(output, format, &mut inbox, outbox),
    }?;
    finished.seek(SeekFrom::Start(0)).map_err(|e| io_failure(&e))?;
    let mut buffer = vec![0_u8; CHUNK];
    loop {
        let read = finished.read(&mut buffer).map_err(|e| io_failure(&e))?;
        if read == 0 {
            return Ok(());
        }
        outbox.send(buffer[..read].to_vec())?;
    }
}

fn zip(output: File, inbox: &mut mpsc::Receiver<ArchiveEvent>, outbox: &Outbox<Vec<u8>>) -> Result<File, Failure> {
    use zip::write::SimpleFileOptions;
    let failed = |e: zip::result::ZipError| io_failure(&io::Error::other(e));
    let mut zip = zip::ZipWriter::new(output);
    let options = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    while let Some(item) = next_item(inbox)? {
        stop_if_cancelled(outbox)?;
        match item {
            Item::Folder { name } => {
                zip.add_directory(name, options.unix_permissions(DEFAULT_FOLDER_MODE)).map_err(failed)?;
            }
            Item::File { name, mode, mut data, length, modified } => {
                let mut options = options.unix_permissions(mode).large_file(length >= ZIP64_THRESHOLD);
                if let Some(time) = modified.and_then(zip_time) {
                    options = options.last_modified_time(time);
                }
                zip.start_file(name, options).map_err(failed)?;
                io::copy(&mut data, &mut zip).map_err(|e| io_failure(&e))?;
            }
        }
    }
    zip.finish().map_err(failed)
}

/// A tar stream over one of the compressors; `finish` flushes the trailer.
enum Compressed {
    Plain(File),
    Gzip(flate2::write::GzEncoder<File>),
    Bzip2(bzip2::write::BzEncoder<File>),
    Xz(liblzma::write::XzEncoder<File>),
    Zstd(zstd::stream::write::Encoder<'static, File>),
}

impl Write for Compressed {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        match self {
            Self::Plain(w) => w.write(bytes),
            Self::Gzip(w) => w.write(bytes),
            Self::Bzip2(w) => w.write(bytes),
            Self::Xz(w) => w.write(bytes),
            Self::Zstd(w) => w.write(bytes),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match self {
            Self::Plain(w) => w.flush(),
            Self::Gzip(w) => w.flush(),
            Self::Bzip2(w) => w.flush(),
            Self::Xz(w) => w.flush(),
            Self::Zstd(w) => w.flush(),
        }
    }
}

impl Compressed {
    fn new(output: File, format: ArchiveFormat) -> io::Result<Self> {
        Ok(match format {
            ArchiveFormat::TarGz => Self::Gzip(flate2::write::GzEncoder::new(output, flate2::Compression::default())),
            ArchiveFormat::TarBz2 => Self::Bzip2(bzip2::write::BzEncoder::new(output, bzip2::Compression::default())),
            ArchiveFormat::TarXz => Self::Xz(liblzma::write::XzEncoder::new(output, XZ_LEVEL)),
            ArchiveFormat::TarZst => Self::Zstd(zstd::stream::write::Encoder::new(output, ZSTD_LEVEL)?),
            _ => Self::Plain(output),
        })
    }

    fn finish(self) -> io::Result<File> {
        match self {
            Self::Plain(w) => Ok(w),
            Self::Gzip(w) => w.finish(),
            Self::Bzip2(w) => w.finish(),
            Self::Xz(w) => w.finish(),
            Self::Zstd(w) => w.finish(),
        }
    }
}

fn tar(
    output: File,
    format: ArchiveFormat,
    inbox: &mut mpsc::Receiver<ArchiveEvent>,
    outbox: &Outbox<Vec<u8>>,
) -> Result<File, Failure> {
    let mut tar = tar::Builder::new(Compressed::new(output, format).map_err(|e| io_failure(&e))?);
    while let Some(item) = next_item(inbox)? {
        stop_if_cancelled(outbox)?;
        let mut header = tar::Header::new_gnu();
        match item {
            Item::Folder { name } => {
                header.set_entry_type(tar::EntryType::Directory);
                header.set_mode(DEFAULT_FOLDER_MODE);
                header.set_size(0);
                tar.append_data(&mut header, format!("{name}/"), io::empty()).map_err(|e| io_failure(&e))?;
            }
            Item::File { name, mode, data, length, modified } => {
                header.set_entry_type(tar::EntryType::Regular);
                header.set_mtime(
                    modified.and_then(|t| u64::try_from(t.unix_millis() / MILLIS_PER_SECOND).ok()).unwrap_or(0),
                );
                header.set_mode(mode);
                header.set_size(length);
                tar.append_data(&mut header, name, data).map_err(|e| io_failure(&e))?;
            }
        }
    }
    let compressed = tar.into_inner().map_err(|e| io_failure(&e))?;
    compressed.finish().map_err(|e| io_failure(&e))
}

fn seven_z(output: File, inbox: &mut mpsc::Receiver<ArchiveEvent>, outbox: &Outbox<Vec<u8>>) -> Result<File, Failure> {
    use sevenz_rust2::{ArchiveEntry, ArchiveWriter};
    let failed = |e: sevenz_rust2::Error| io_failure(&io::Error::other(e.to_string()));
    let mut writer = ArchiveWriter::new(output).map_err(failed)?;
    while let Some(item) = next_item(inbox)? {
        stop_if_cancelled(outbox)?;
        match item {
            Item::Folder { name } => {
                let mut entry = ArchiveEntry::new_directory(&name);
                entry.has_windows_attributes = true;
                entry.windows_attributes =
                    SEVEN_Z_DIRECTORY_ATTRIBUTE | SEVEN_Z_UNIX_EXTENSION | ((0o040_000 | DEFAULT_FOLDER_MODE) << 16);
                writer.push_archive_entry::<File>(entry, None).map_err(failed)?;
            }
            Item::File { name, mode, data, modified, .. } => {
                let mut entry = ArchiveEntry::new_file(&name);
                if let Some(time) = modified.and_then(seven_z_time) {
                    entry.last_modified_date = time;
                    entry.has_last_modified_date = true;
                }
                entry.has_windows_attributes = true;
                entry.windows_attributes =
                    SEVEN_Z_ARCHIVE_ATTRIBUTE | SEVEN_Z_UNIX_EXTENSION | ((0o100_000 | mode) << 16);
                writer.push_archive_entry(entry, Some(data)).map_err(failed)?;
            }
        }
    }
    writer.finish().map_err(|e| io_failure(&e))
}

const MILLIS_PER_SECOND: i64 = 1000;
const SECONDS_PER_DAY: i64 = 86_400;
const SECONDS_PER_HOUR: i64 = 3600;
const SECONDS_PER_MINUTE: i64 = 60;

/// zip stores calendar fields (DOS time, 1980–2107); UTC here.
fn zip_time(time: Timestamp) -> Option<zip::DateTime> {
    let seconds = time.unix_millis().div_euclid(MILLIS_PER_SECOND);
    let (days, second_of_day) = (seconds.div_euclid(SECONDS_PER_DAY), seconds.rem_euclid(SECONDS_PER_DAY));
    let (year, month, day) = civil_from_days(days);
    zip::DateTime::from_date_and_time(
        u16::try_from(year).ok()?,
        u8::try_from(month).ok()?,
        u8::try_from(day).ok()?,
        u8::try_from(second_of_day / SECONDS_PER_HOUR).ok()?,
        u8::try_from(second_of_day / SECONDS_PER_MINUTE % SECONDS_PER_MINUTE).ok()?,
        u8::try_from(second_of_day % SECONDS_PER_MINUTE).ok()?,
    )
    .ok()
}

fn seven_z_time(time: Timestamp) -> Option<sevenz_rust2::NtTime> {
    let since_epoch = std::time::Duration::from_millis(u64::try_from(time.unix_millis()).ok()?);
    sevenz_rust2::NtTime::try_from(std::time::UNIX_EPOCH + since_epoch).ok()
}

/// Days since 1970-01-01 to (year, month, day): Howard Hinnant's
/// `civil_from_days`, with his names for the intermediate values.
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + i64::from(month <= 2), month, day)
}

fn stop_if_cancelled(outbox: &Outbox<Vec<u8>>) -> Result<(), Failure> {
    if outbox.is_cancelled() { Err(Failure::Stopped) } else { Ok(()) }
}
