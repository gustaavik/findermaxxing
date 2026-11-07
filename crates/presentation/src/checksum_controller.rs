//! `ChecksumController`: the Checksums dialog. Computes MD5, SHA-1, SHA-256
//! and BLAKE3 in one pass, looks for a published digest next to the file,
//! and checks a pasted one.

use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use application::CancellationSource;
use cxx_qt::{CxxQtType, Threading};
use cxx_qt_lib::QString;
use domain::{Digest, HashAlgorithm, StoragePath, Verification, verify};

use crate::clipboard_bridge::set_clipboard_text;
use crate::services::AppServices;

#[cxx_qt::bridge]
pub mod qobject {
    unsafe extern "C++" {
        include!("cxx-qt-lib/qstring.h");
        type QString = cxx_qt_lib::QString;

        include!("presentation/cpp/services_holder.h");
        type ServicesHolder = crate::services_bridge::ServicesHolder;
    }

    /// How a pasted or published digest compares.
    #[qenum(ChecksumController)]
    enum Outcome {
        NotChecked,
        Matches,
        Differs,
        NotADigest,
    }

    #[auto_cxx_name]
    unsafe extern "RustQt" {
        #[qobject]
        #[qml_element]
        #[qproperty(*mut ServicesHolder, services)]
        #[qproperty(QString, file_name)]
        #[qproperty(bool, running)]
        #[qproperty(f64, progress)]
        #[qproperty(QString, md5)]
        #[qproperty(QString, sha1)]
        #[qproperty(QString, sha256)]
        #[qproperty(QString, blake3)]
        /// The digest found in a checksum list next to the file, if any.
        #[qproperty(QString, published)]
        #[qproperty(Outcome, outcome)]
        #[qproperty(QString, verdict)]
        #[qproperty(QString, error_message)]
        type ChecksumController = super::ChecksumControllerRust;
    }

    #[auto_cxx_name]
    extern "RustQt" {
        #[qinvokable]
        fn start(self: Pin<&mut ChecksumController>, location: &QString);
        #[qinvokable]
        fn check(self: Pin<&mut ChecksumController>, expected: &QString);
        #[qinvokable]
        fn cancel(self: Pin<&mut ChecksumController>);
        #[qinvokable]
        fn copy_text(self: &ChecksumController, text: &QString);
    }

    impl cxx_qt::Threading for ChecksumController {}
}

use qobject::{Outcome, ServicesHolder};

pub struct ChecksumControllerRust {
    services: *mut ServicesHolder,
    file_name: QString,
    running: bool,
    progress: f64,
    md5: QString,
    sha1: QString,
    sha256: QString,
    blake3: QString,
    published: QString,
    outcome: Outcome,
    verdict: QString,
    error_message: QString,

    digests: Vec<Digest>,
    job: Option<CancellationSource>,
}

impl Default for ChecksumControllerRust {
    fn default() -> Self {
        Self {
            services: std::ptr::null_mut(),
            file_name: QString::default(),
            running: false,
            progress: 0.0,
            md5: QString::default(),
            sha1: QString::default(),
            sha256: QString::default(),
            blake3: QString::default(),
            published: QString::default(),
            outcome: Outcome::NotChecked,
            verdict: QString::default(),
            error_message: QString::default(),
            digests: Vec::new(),
            job: None,
        }
    }
}

impl qobject::ChecksumController {
    pub fn start(mut self: Pin<&mut Self>, location: &QString) {
        let (Some(services), Ok(file)) = (self.app_services(), StoragePath::decode(&location.to_string())) else {
            return;
        };
        self.as_mut().cancel();
        self.as_mut().reset(&file);
        let source = CancellationSource::new();
        let cancel = source.token();
        self.as_mut().rust_mut().job = Some(source);
        self.as_mut().set_running(true);
        let thread = self.qt_thread();
        let runtime = services.runtime.clone();
        runtime.spawn(async move {
            let total = file_size(&services, &file).await;
            let done = Arc::new(AtomicU64::new(0));
            let progress_thread = thread.clone();
            let progress: application::ports::ProgressCallback = Arc::new(move |bytes| {
                let so_far = done.fetch_add(bytes.0, Ordering::Relaxed) + bytes.0;
                #[allow(clippy::cast_precision_loss, reason = "a progress bar")]
                let fraction = so_far as f64 / total.max(1) as f64;
                let _ = progress_thread.queue(move |controller| controller.set_progress(fraction.min(1.0)));
            });
            let utilities = &services.utilities;
            let digests = utilities.checksums.execute(&file, &HashAlgorithm::ALL, progress, cancel.clone()).await;
            let published = utilities.published_checksum.execute(&file).await.ok().flatten();
            let _ = thread.queue(move |mut controller| {
                controller.as_mut().set_running(false);
                controller.as_mut().rust_mut().job = None;
                match digests {
                    Ok(digests) => controller.show(digests, published),
                    Err(domain::VfsError::Cancelled) => {}
                    Err(error) => controller.set_error_message(QString::from(&error.to_string())),
                }
            });
        });
    }

    pub fn check(mut self: Pin<&mut Self>, expected: &QString) {
        let expected = expected.to_string();
        if expected.trim().is_empty() {
            self.as_mut().set_outcome(Outcome::NotChecked);
            self.set_verdict(QString::default());
            return;
        }
        let verification = verify(&expected, &self.digests);
        let (outcome, verdict) = describe(verification);
        self.as_mut().set_outcome(outcome);
        self.set_verdict(QString::from(&verdict));
    }

    pub fn cancel(mut self: Pin<&mut Self>) {
        if let Some(job) = self.as_mut().rust_mut().job.take() {
            job.cancel();
        }
        self.set_running(false);
    }

    pub fn copy_text(&self, text: &QString) {
        set_clipboard_text(&text.to_string());
    }

    fn app_services(&self) -> Option<AppServices> {
        // SAFETY: the holder outlives every QML object (see bootstrap.cpp).
        let holder = unsafe { self.services().as_ref() }?;
        Some(holder.services().clone())
    }

    fn reset(mut self: Pin<&mut Self>, file: &StoragePath) {
        let name = file.name().map(|n| n.display_name().into_owned()).unwrap_or_default();
        self.as_mut().set_file_name(QString::from(&name));
        for setter in [Self::set_md5, Self::set_sha1, Self::set_sha256, Self::set_blake3, Self::set_published] {
            setter(self.as_mut(), QString::default());
        }
        self.as_mut().set_verdict(QString::default());
        self.as_mut().set_error_message(QString::default());
        self.as_mut().set_outcome(Outcome::NotChecked);
        self.as_mut().set_progress(0.0);
        self.rust_mut().digests.clear();
    }

    fn show(mut self: Pin<&mut Self>, digests: Vec<Digest>, published: Option<String>) {
        for digest in &digests {
            let hex = QString::from(digest.hex());
            match digest.algorithm() {
                HashAlgorithm::Md5 => self.as_mut().set_md5(hex),
                HashAlgorithm::Sha1 => self.as_mut().set_sha1(hex),
                HashAlgorithm::Sha256 => self.as_mut().set_sha256(hex),
                HashAlgorithm::Blake3 => self.as_mut().set_blake3(hex),
            }
        }
        self.as_mut().rust_mut().digests = digests;
        if let Some(published) = published {
            self.as_mut().set_published(QString::from(&published));
            self.check(&QString::from(&published));
        }
    }
}

async fn file_size(services: &AppServices, file: &StoragePath) -> u64 {
    let Ok(lister) = services.mounts.resolve(file.mount()).and_then(|m| m.lister()) else { return 0 };
    lister.stat_boxed(file).await.ok().and_then(|e| e.metadata().size).map_or(0, |s| s.0)
}

fn describe(verification: Verification) -> (Outcome, String) {
    match verification {
        Verification::Matches(algorithm) => {
            let caveat = if algorithm.is_collision_resistant() {
                ""
            } else {
                " This shows the file isn't damaged; it can't prove nobody tampered with it."
            };
            (Outcome::Matches, format!("Matches the {} checksum.{caveat}", algorithm.name()))
        }
        Verification::Differs => {
            (Outcome::Differs, "Doesn't match. The file is damaged or not the one that was published.".to_owned())
        }
        Verification::NotADigest => {
            (Outcome::NotADigest, "That isn't an MD5, SHA-1, SHA-256 or BLAKE3 checksum.".to_owned())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn weak_algorithms_carry_a_caveat() {
        assert!(describe(Verification::Matches(HashAlgorithm::Md5)).1.contains("tampered"));
        assert!(!describe(Verification::Matches(HashAlgorithm::Sha256)).1.contains("tampered"));
        assert!(describe(Verification::Differs).0 == Outcome::Differs);
    }
}
