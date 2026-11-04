//! Contract cases for `Reader` + `Writer`: round trips (including sizes that
//! need multipart uploads), never overwriting, and nothing visible until commit.

use application::CancelToken;
use application::ports::{Reader, Writer};
use domain::{ByteSize, VfsError};
use futures::TryStreamExt;

use crate::{VfsFixture, child};

/// Large enough to need several S3 multipart parts.
const LARGE_PAYLOAD: usize = 17 * 1024 * 1024;
const WRITE_CHUNK: usize = 1024 * 1024;

/// Expands to the streaming contract cases.
#[macro_export]
macro_rules! stream_contract_tests {
    ($make_fixture:expr $(, #[$attr:meta])*) => {
        $crate::vfs_contract_tests!(@cases $make_fixture; [$(#[$attr])*];
            written_bytes_read_back_identically,
            large_writes_read_back_identically,
            writer_never_overwrites,
            nothing_is_visible_before_commit,
            aborted_writes_leave_nothing,
            reading_missing_is_not_found,
            reading_a_folder_is_a_directory_error,
        );
    };
}

async fn write<F: VfsFixture>(fixture: &F, name: &str, payload: &[u8]) -> Result<(), VfsError>
where
    F::Vfs: Writer,
{
    let mut sink = fixture.vfs().create(&child(fixture, name), Some(ByteSize(payload.len() as u64))).await?;
    for chunk in payload.chunks(WRITE_CHUNK) {
        sink.write(chunk.to_vec()).await?;
    }
    sink.commit().await
}

async fn read_all<F: VfsFixture>(fixture: &F, name: &str) -> Result<Vec<u8>, VfsError>
where
    F::Vfs: Reader,
{
    let chunks: Vec<Vec<u8>> =
        fixture.vfs().read(&child(fixture, name), CancelToken::never()).await?.try_collect().await?;
    Ok(chunks.concat())
}

pub async fn written_bytes_read_back_identically<F: VfsFixture>(fixture: &F)
where
    F::Vfs: Reader + Writer,
{
    write(fixture, "small.bin", b"hello, provider").await.unwrap();
    assert_eq!(read_all(fixture, "small.bin").await.unwrap(), b"hello, provider");
    assert_eq!(fixture.read_file("small.bin").await.as_deref(), Some(b"hello, provider".as_slice()));
}

pub async fn large_writes_read_back_identically<F: VfsFixture>(fixture: &F)
where
    F::Vfs: Reader + Writer,
{
    let payload: Vec<u8> = (0..LARGE_PAYLOAD).map(|i| (i % 251) as u8).collect();
    write(fixture, "large.bin", &payload).await.unwrap();
    assert!(read_all(fixture, "large.bin").await.unwrap() == payload, "large payload must round-trip byte for byte");
}

pub async fn writer_never_overwrites<F: VfsFixture>(fixture: &F)
where
    F::Vfs: Writer,
{
    fixture.create_file("taken", b"original").await;
    let result = fixture.vfs().create(&child(fixture, "taken"), None).await.map(drop);
    assert_eq!(result, Err(VfsError::AlreadyExists));
    assert_eq!(fixture.read_file("taken").await.as_deref(), Some(b"original".as_slice()));
}

pub async fn nothing_is_visible_before_commit<F: VfsFixture>(fixture: &F)
where
    F::Vfs: Writer,
{
    let mut sink = fixture.vfs().create(&child(fixture, "pending"), None).await.unwrap();
    sink.write(b"not yet".to_vec()).await.unwrap();
    assert_eq!(fixture.read_file("pending").await, None);
    sink.commit().await.unwrap();
    assert_eq!(fixture.read_file("pending").await.as_deref(), Some(b"not yet".as_slice()));
}

pub async fn aborted_writes_leave_nothing<F: VfsFixture>(fixture: &F)
where
    F::Vfs: Writer,
{
    let mut sink = fixture.vfs().create(&child(fixture, "aborted"), None).await.unwrap();
    sink.write(vec![1; WRITE_CHUNK]).await.unwrap();
    sink.abort().await;
    assert_eq!(fixture.read_file("aborted").await, None);
}

pub async fn reading_missing_is_not_found<F: VfsFixture>(fixture: &F)
where
    F::Vfs: Reader,
{
    assert_eq!(read_all(fixture, "nope").await, Err(VfsError::NotFound));
}

pub async fn reading_a_folder_is_a_directory_error<F: VfsFixture>(fixture: &F)
where
    F::Vfs: Reader,
{
    fixture.create_dir("folder").await;
    fixture.create_file("folder/inside", b"x").await;
    assert_eq!(read_all(fixture, "folder").await, Err(VfsError::IsADirectory));
}
