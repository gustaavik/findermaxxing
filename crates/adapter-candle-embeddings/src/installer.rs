//! Downloads model packs (resumable, verified), then prepares them: weights
//! are converted to f16 on disk (half the space; computed in f32), and of
//! CLIP only the vision half is kept. The originals are deleted once the
//! prepared files are written. A pack counts as installed when its marker
//! file exists, which is written last.

use std::path::{Path, PathBuf};

use application::CancelToken;
use application::ports::{ModelInstaller, ModelPack, ProgressCallback};
use domain::{ByteSize, VfsError};
use futures::future::BoxFuture;
use futures::{FutureExt, StreamExt};
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;
use tokio::runtime::Handle;

use crate::manifest::{HUGGING_FACE, ModelFile, files, pack_folder};
use crate::prepare;

const MARKER: &str = "installed";
const PARTIAL_SUFFIX: &str = ".partial";

/// Installs packs below `root` (the app's models folder).
#[derive(Debug, Clone)]
pub struct HuggingFaceInstaller {
    root: PathBuf,
    base: String,
    http: reqwest::Client,
    runtime: Handle,
}

impl HuggingFaceInstaller {
    #[must_use]
    pub fn new(root: PathBuf, runtime: Handle) -> Self {
        Self::with_base(root, HUGGING_FACE.to_owned(), runtime)
    }

    /// Downloads from `base` instead of Hugging Face (tests).
    #[must_use]
    pub fn with_base(root: PathBuf, base: String, runtime: Handle) -> Self {
        Self { root, base, http: reqwest::Client::new(), runtime }
    }

    #[must_use]
    pub fn pack_dir(&self, pack: ModelPack) -> PathBuf {
        self.root.join(pack_folder(pack))
    }

    async fn install_pack(
        &self,
        pack: ModelPack,
        progress: ProgressCallback,
        cancel: CancelToken,
    ) -> Result<(), VfsError> {
        let dir = self.pack_dir(pack);
        if prepare::prepared(pack, &dir) {
            // Interrupted after preparing: the originals are gone, the result is there.
            progress(crate::manifest::download_size(pack));
        }
        for file in files(pack).iter().filter(|_| !prepare::prepared(pack, &dir)) {
            let target = dir.join(file.local);
            if verified(&target, file).await {
                progress(ByteSize(file.size));
                continue;
            }
            self.download(file, &target, &progress, &cancel).await?;
        }
        let task = self.runtime.spawn_blocking(move || prepare::prepare(pack, &dir));
        task.await.map_err(|e| VfsError::Io { message: e.to_string() })??;
        tokio::fs::write(self.pack_dir(pack).join(MARKER), crate::manifest::model_id(pack)).await.map_err(|e| io(&e))
    }

    /// Streams into `<target>.partial`, continuing a previous attempt, then
    /// checks the hash before giving the file its name.
    async fn download(
        &self,
        file: &ModelFile,
        target: &Path,
        progress: &ProgressCallback,
        cancel: &CancelToken,
    ) -> Result<(), VfsError> {
        let partial = PathBuf::from(format!("{}{PARTIAL_SUFFIX}", target.display()));
        if let Some(parent) = partial.parent() {
            tokio::fs::create_dir_all(parent).await.map_err(|e| io(&e))?;
        }
        let have = tokio::fs::metadata(&partial).await.map(|m| m.len()).unwrap_or(0).min(file.size);
        let url = format!("{}/{}/resolve/{}/{}", self.base, file.repo, file.revision, file.path);
        let mut request = self.http.get(&url);
        if have > 0 {
            request = request.header(reqwest::header::RANGE, format!("bytes={have}-"));
        }
        let response = request.send().await.map_err(|e| network(&e))?;
        let resumed = response.status() == reqwest::StatusCode::PARTIAL_CONTENT;
        if !response.status().is_success() {
            return Err(VfsError::Io { message: format!("{url} answered {}", response.status()) });
        }
        let mut output = tokio::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .append(resumed)
            .truncate(!resumed)
            .open(&partial)
            .await
            .map_err(|e| io(&e))?;
        if resumed {
            progress(ByteSize(have));
        }
        let mut body = response.bytes_stream();
        while let Some(chunk) = body.next().await {
            if cancel.is_cancelled() {
                return Err(VfsError::Cancelled);
            }
            let chunk = chunk.map_err(|e| network(&e))?;
            output.write_all(&chunk).await.map_err(|e| io(&e))?;
            progress(ByteSize(chunk.len() as u64));
        }
        output.flush().await.map_err(|e| io(&e))?;
        drop(output);
        if !verified(&partial, file).await {
            let _ = tokio::fs::remove_file(&partial).await;
            return Err(VfsError::Io { message: format!("{} didn't match its checksum; it was deleted", file.path) });
        }
        tokio::fs::rename(&partial, target).await.map_err(|e| io(&e))
    }
}

/// The file is complete and has the pinned hash.
async fn verified(path: &Path, file: &ModelFile) -> bool {
    let (path, expected) = (path.to_path_buf(), file.sha256);
    if tokio::fs::metadata(&path).await.map(|m| m.len()).ok() != Some(file.size) {
        return false;
    }
    tokio::task::spawn_blocking(move || sha256_of(&path).is_ok_and(|hash| hash == expected)).await.unwrap_or(false)
}

fn sha256_of(path: &Path) -> std::io::Result<String> {
    use std::io::Read;
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0; 1 << 20];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hasher.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

fn io(error: &std::io::Error) -> VfsError {
    VfsError::Io { message: error.to_string() }
}

fn network(error: &reqwest::Error) -> VfsError {
    if error.is_connect() || error.is_timeout() {
        VfsError::Transient { retry_after: None }
    } else {
        VfsError::Io { message: error.to_string() }
    }
}

impl ModelInstaller for HuggingFaceInstaller {
    fn installed(&self, pack: ModelPack) -> bool {
        std::fs::read_to_string(self.pack_dir(pack).join(MARKER)).is_ok_and(|id| id == crate::manifest::model_id(pack))
    }

    fn download_size(&self, pack: ModelPack) -> ByteSize {
        crate::manifest::download_size(pack)
    }

    fn source(&self, pack: ModelPack) -> String {
        crate::manifest::source(pack)
    }

    fn install(
        &self,
        pack: ModelPack,
        progress: ProgressCallback,
        cancel: CancelToken,
    ) -> BoxFuture<'static, Result<(), VfsError>> {
        let installer = self.clone();
        async move { installer.install_pack(pack, progress, cancel).await }.boxed()
    }

    fn uninstall(&self, pack: ModelPack) -> BoxFuture<'static, Result<(), VfsError>> {
        let dir = self.pack_dir(pack);
        async move {
            match tokio::fs::remove_dir_all(&dir).await {
                Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(io(&error)),
                _ => Ok(()),
            }
        }
        .boxed()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU64, Ordering};

    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    use super::*;

    const PAYLOAD: &[u8] = b"pretend these are model weights";

    /// Serves `body` for any path, honouring `Range: bytes=N-`.
    async fn serve(body: &'static [u8]) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                let mut request = vec![0; 4096];
                let read = stream.read(&mut request).await.unwrap_or(0);
                let request = String::from_utf8_lossy(&request[..read]).to_lowercase();
                let start: usize = request
                    .lines()
                    .find_map(|l| l.strip_prefix("range: bytes=")?.strip_suffix('-')?.parse().ok())
                    .unwrap_or(0);
                let status = if start > 0 { "206 Partial Content" } else { "200 OK" };
                let rest = &body[start.min(body.len())..];
                let head = format!("HTTP/1.1 {status}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n", rest.len());
                let _ = stream.write_all(head.as_bytes()).await;
                let _ = stream.write_all(rest).await;
            }
        });
        base
    }

    fn file(sha256: &'static str) -> ModelFile {
        ModelFile {
            repo: "test/repo",
            revision: "abc",
            path: "weights.bin",
            size: PAYLOAD.len() as u64,
            sha256,
            local: "weights.bin",
        }
    }

    fn sha(data: &[u8]) -> &'static str {
        let hex: String = Sha256::digest(data).iter().map(|b| format!("{b:02x}")).collect();
        Box::leak(hex.into_boxed_str())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn downloads_resume_and_are_verified() {
        let dir = tempfile::tempdir().unwrap();
        let base = serve(PAYLOAD).await;
        let installer = HuggingFaceInstaller::with_base(dir.path().to_path_buf(), base, Handle::current());
        let target = dir.path().join("weights.bin");
        // A previous attempt got the first 10 bytes.
        std::fs::write(dir.path().join("weights.bin.partial"), &PAYLOAD[..10]).unwrap();
        let reported = Arc::new(AtomicU64::new(0));
        let counter = Arc::clone(&reported);
        let progress: ProgressCallback = Arc::new(move |b| {
            counter.fetch_add(b.0, Ordering::SeqCst);
        });
        installer.download(&file(sha(PAYLOAD)), &target, &progress, &CancelToken::never()).await.unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), PAYLOAD);
        assert_eq!(reported.load(Ordering::SeqCst), PAYLOAD.len() as u64, "resumed bytes count once");

        let wrong = dir.path().join("wrong.bin");
        let result = installer.download(&file(sha(b"something else")), &wrong, &progress, &CancelToken::never()).await;
        assert!(result.is_err());
        assert!(!wrong.exists() && !dir.path().join("wrong.bin.partial").exists(), "a bad download is deleted");
    }

    #[test]
    fn packs_are_pinned_and_sized() {
        assert_eq!(crate::manifest::download_size(ModelPack::Text).0, 655 + 17_082_730 + 470_641_600);
        for file in crate::manifest::files(ModelPack::Text).iter().chain(crate::manifest::files(ModelPack::Images)) {
            assert_eq!(file.sha256.len(), 64, "{}", file.path);
            assert_eq!(file.revision.len(), 40, "full commit ids, never branches");
        }
    }
}
