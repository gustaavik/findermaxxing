//! `AiSearchController`: the AI search settings and status. Turning search
//! on downloads its model only after the user agreed in the dialog, which
//! shows the size and source first; everything after that runs locally.

use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use application::CancellationSource;
use application::ports::{AiSettings, ModelPack};
use application::use_cases::{AiSearchError, ScopeChange};
use cxx_qt::{CxxQtType, Threading};
use cxx_qt_lib::{QString, QStringList};
use domain::{ByteSize, IndexStatus, StoragePath};

use crate::file_urls::from_file_url;
use crate::row_format::format_size;
use crate::services::AppServices;

#[cxx_qt::bridge]
pub mod qobject {
    unsafe extern "C++" {
        include!("cxx-qt-lib/qstring.h");
        type QString = cxx_qt_lib::QString;

        include!("cxx-qt-lib/qstringlist.h");
        type QStringList = cxx_qt_lib::QStringList;

        include!("presentation/cpp/services_holder.h");
        type ServicesHolder = crate::services_bridge::ServicesHolder;
    }

    #[auto_cxx_name]
    unsafe extern "RustQt" {
        #[qobject]
        #[qml_element]
        #[qproperty(*mut ServicesHolder, services)]
        #[qproperty(bool, enabled)]
        #[qproperty(bool, images_enabled)]
        #[qproperty(bool, paused)]
        #[qproperty(bool, installing)]
        /// 0..1 while a model downloads.
        #[qproperty(f64, install_progress)]
        #[qproperty(QString, install_label)]
        #[qproperty(QString, status_text)]
        /// 0..1 while indexing; negative when there is no measure (scanning, idle).
        #[qproperty(f64, progress)]
        #[qproperty(bool, working)]
        /// Folders searched, for display, and their locations.
        #[qproperty(QStringList, folders)]
        #[qproperty(QStringList, folder_ids)]
        #[qproperty(QStringList, excluded)]
        #[qproperty(QStringList, excluded_ids)]
        /// "488 MB from huggingface.co (…)", shown before agreeing.
        #[qproperty(QString, text_offer)]
        #[qproperty(QString, image_offer)]
        #[qproperty(QString, error_message)]
        type AiSearchController = super::AiSearchControllerRust;
    }

    #[auto_cxx_name]
    extern "RustQt" {
        #[qinvokable]
        fn refresh(self: Pin<&mut AiSearchController>);
        /// After the user agreed: downloads the text model, then turns search on.
        #[qinvokable]
        fn enable_text(self: Pin<&mut AiSearchController>);
        /// After the user agreed: downloads the image models, then searches photos too.
        #[qinvokable]
        fn enable_images(self: Pin<&mut AiSearchController>);
        #[qinvokable]
        fn cancel_install(self: Pin<&mut AiSearchController>);
        /// `forget` also deletes the index and the downloaded models.
        #[qinvokable]
        fn turn_off(self: Pin<&mut AiSearchController>, forget: bool);
        #[qinvokable]
        fn turn_off_images(self: Pin<&mut AiSearchController>, forget: bool);
        #[qinvokable]
        fn set_indexing_paused(self: Pin<&mut AiSearchController>, paused: bool);
        #[qinvokable]
        fn include_folder(self: Pin<&mut AiSearchController>, location_id: &QString);
        #[qinvokable]
        fn exclude_folder(self: Pin<&mut AiSearchController>, location_id: &QString);
        #[qinvokable]
        fn forget_folder(self: Pin<&mut AiSearchController>, location_id: &QString);
        #[qinvokable]
        fn rescan(self: Pin<&mut AiSearchController>);
    }

    impl cxx_qt::Threading for AiSearchController {}
}

use qobject::ServicesHolder;

// The bools are Q_PROPERTY storage, which cxx-qt requires as plain fields.
#[allow(clippy::struct_excessive_bools, reason = "mirrors Qt properties one-to-one")]
pub struct AiSearchControllerRust {
    services: *mut ServicesHolder,
    enabled: bool,
    images_enabled: bool,
    paused: bool,
    installing: bool,
    install_progress: f64,
    install_label: QString,
    status_text: QString,
    progress: f64,
    working: bool,
    folders: QStringList,
    folder_ids: QStringList,
    excluded: QStringList,
    excluded_ids: QStringList,
    text_offer: QString,
    image_offer: QString,
    error_message: QString,

    listening: bool,
    install: Option<CancellationSource>,
}

impl Default for AiSearchControllerRust {
    fn default() -> Self {
        Self {
            services: std::ptr::null_mut(),
            enabled: false,
            images_enabled: false,
            paused: false,
            installing: false,
            install_progress: 0.0,
            install_label: QString::default(),
            status_text: QString::default(),
            progress: -1.0,
            working: false,
            folders: QStringList::default(),
            folder_ids: QStringList::default(),
            excluded: QStringList::default(),
            excluded_ids: QStringList::default(),
            text_offer: QString::default(),
            image_offer: QString::default(),
            error_message: QString::default(),
            listening: false,
            install: None,
        }
    }
}

impl qobject::AiSearchController {
    pub fn refresh(mut self: Pin<&mut Self>) {
        let Some(services) = self.app_services() else { return };
        if !self.listening {
            self.as_mut().rust_mut().listening = true;
            let thread = self.qt_thread();
            services.ai.status.listen(Box::new(move |status| {
                let status = status.clone();
                let _ = thread.queue(move |controller| controller.show_status(&status));
            }));
        }
        let models = &services.ai.models;
        let offer = |pack| format!("{} from {}", format_size(models.download_size(pack)), models.source(pack));
        self.as_mut().set_text_offer(QString::from(&offer(ModelPack::Text)));
        self.as_mut().set_image_offer(QString::from(&offer(ModelPack::Images)));
        let use_case = services.ai.settings.clone();
        self.spawn(async move { use_case.execute().await });
    }

    pub fn enable_text(self: Pin<&mut Self>) {
        self.install(ModelPack::Text);
    }

    pub fn enable_images(self: Pin<&mut Self>) {
        self.install(ModelPack::Images);
    }

    fn install(mut self: Pin<&mut Self>, pack: ModelPack) {
        let Some(services) = self.app_services() else { return };
        if self.installing {
            return;
        }
        let cancellation = CancellationSource::new();
        let cancel = cancellation.token();
        self.as_mut().rust_mut().install = Some(cancellation);
        let total = services.ai.models.download_size(pack);
        self.as_mut().set_installing(true);
        self.as_mut().set_install_progress(0.0);
        self.as_mut().set_error_message(QString::default());
        self.as_mut().set_install_label(QString::from(&download_label(ByteSize(0), total)));
        let (thread, received) = (self.qt_thread(), Arc::new(AtomicU64::new(0)));
        let progress_thread = thread.clone();
        let progress: application::ports::ProgressCallback = Arc::new(move |bytes| {
            let done = ByteSize(received.fetch_add(bytes.0, Ordering::Relaxed) + bytes.0);
            let _ = progress_thread.queue(move |mut controller| {
                #[allow(clippy::cast_precision_loss, reason = "a progress bar")]
                let fraction = done.0 as f64 / total.0.max(1) as f64;
                controller.as_mut().set_install_progress(fraction.min(1.0));
                controller.set_install_label(QString::from(&download_label(done, total)));
            });
        });
        let use_case = services.ai.enable.clone();
        services.runtime.spawn(async move {
            let result = use_case.execute(pack, progress, cancel).await;
            let _ = thread.queue(move |mut controller| {
                controller.as_mut().rust_mut().install = None;
                controller.as_mut().set_installing(false);
                controller.apply(result);
            });
        });
    }

    pub fn cancel_install(mut self: Pin<&mut Self>) {
        if let Some(cancellation) = self.as_mut().rust_mut().install.take() {
            cancellation.cancel();
        }
    }

    pub fn turn_off(self: Pin<&mut Self>, forget: bool) {
        let Some(services) = self.app_services() else { return };
        let use_case = services.ai.disable;
        self.spawn(async move { use_case.execute(ModelPack::Text, forget).await });
    }

    pub fn turn_off_images(self: Pin<&mut Self>, forget: bool) {
        let Some(services) = self.app_services() else { return };
        let use_case = services.ai.disable;
        self.spawn(async move { use_case.execute(ModelPack::Images, forget).await });
    }

    pub fn set_indexing_paused(self: Pin<&mut Self>, paused: bool) {
        let Some(services) = self.app_services() else { return };
        let use_case = services.ai.pause;
        self.spawn(async move { use_case.execute(paused).await });
    }

    pub fn include_folder(self: Pin<&mut Self>, location_id: &QString) {
        self.change_scope(location_id, ScopeChange::Include);
    }

    pub fn exclude_folder(self: Pin<&mut Self>, location_id: &QString) {
        self.change_scope(location_id, ScopeChange::Exclude);
    }

    pub fn forget_folder(self: Pin<&mut Self>, location_id: &QString) {
        self.change_scope(location_id, ScopeChange::Forget);
    }

    pub fn rescan(self: Pin<&mut Self>) {
        if let Some(services) = self.app_services() {
            services.ai.control.rescan();
        }
    }

    fn change_scope(self: Pin<&mut Self>, location_id: &QString, change: fn(StoragePath) -> ScopeChange) {
        let Some(services) = self.app_services() else { return };
        // The folder picker hands over file URLs; the dialog's lists, our own locations.
        let text = location_id.to_string();
        let Some(location) = from_file_url(&text, services.home.mount()).or_else(|| StoragePath::decode(&text).ok())
        else {
            return;
        };
        let use_case = services.ai.scope;
        self.spawn(async move { use_case.execute(change(location)).await });
    }

    fn spawn(
        self: Pin<&mut Self>,
        work: impl std::future::Future<Output = Result<AiSettings, AiSearchError>> + Send + 'static,
    ) {
        let Some(services) = self.app_services() else { return };
        let thread = self.qt_thread();
        services.runtime.spawn(async move {
            let result = work.await;
            let _ = thread.queue(move |controller| controller.apply(result));
        });
    }

    fn apply(mut self: Pin<&mut Self>, result: Result<AiSettings, AiSearchError>) {
        let settings = match result {
            Ok(settings) => settings,
            Err(AiSearchError::Install(domain::VfsError::Cancelled)) => return,
            Err(error) => return self.set_error_message(QString::from(&error.to_string())),
        };
        let home = self.app_services().map(|s| s.home);
        let show = |locations: &[StoragePath]| -> (QStringList, QStringList) {
            (
                locations.iter().map(|l| QString::from(&shown(l, home.as_ref()))).collect(),
                locations.iter().map(|l| QString::from(&l.encode())).collect(),
            )
        };
        let (folders, folder_ids) = show(&settings.scope.include);
        let (excluded, excluded_ids) = show(&settings.scope.exclude);
        self.as_mut().set_enabled(settings.enabled);
        self.as_mut().set_images_enabled(settings.images);
        self.as_mut().set_paused(settings.paused);
        self.as_mut().set_folders(folders);
        self.as_mut().set_folder_ids(folder_ids);
        self.as_mut().set_excluded(excluded);
        self.set_excluded_ids(excluded_ids);
    }

    fn show_status(mut self: Pin<&mut Self>, status: &IndexStatus) {
        let (text, progress, working) = status_line(status);
        self.as_mut().set_status_text(QString::from(&text));
        self.as_mut().set_progress(progress);
        self.set_working(working);
    }

    fn app_services(&self) -> Option<AppServices> {
        // SAFETY: the holder outlives every QML object (see bootstrap.cpp).
        let holder = unsafe { self.services().as_ref() }?;
        Some(holder.services().clone())
    }
}

fn download_label(done: ByteSize, total: ByteSize) -> String {
    format!("Downloading {} of {}…", format_size(done), format_size(total))
}

/// `~/Documents` for folders below home, the full location otherwise.
fn shown(location: &StoragePath, home: Option<&StoragePath>) -> String {
    match home {
        Some(home) if location.is_within(home) => {
            let rest = location.segments()[home.segments().len()..]
                .iter()
                .map(|s| s.display_name().into_owned())
                .collect::<Vec<_>>()
                .join("/");
            if rest.is_empty() { "~".to_owned() } else { format!("~/{rest}") }
        }
        _ => location.to_string(),
    }
}

/// Text, progress (negative: none) and whether work is going on.
fn status_line(status: &IndexStatus) -> (String, f64, bool) {
    match status {
        IndexStatus::Off => ("AI search is off".to_owned(), -1.0, false),
        IndexStatus::NeedsModel => ("Waiting for the model download".to_owned(), -1.0, false),
        IndexStatus::Downloading { done, total } => {
            #[allow(clippy::cast_precision_loss, reason = "a progress bar")]
            let fraction = done.0 as f64 / total.0.max(1) as f64;
            (download_label(*done, *total), fraction, true)
        }
        IndexStatus::Scanning { seen } => (format!("Looking for changes ({seen} folders)"), -1.0, true),
        IndexStatus::Indexing { done, total, current } => {
            #[allow(clippy::cast_precision_loss, reason = "a progress bar")]
            let fraction = *done as f64 / (*total).max(1) as f64;
            (format!("Indexing {} of {total}: {current}", done + 1), fraction, true)
        }
        IndexStatus::Paused { indexed } => (format!("Paused · {indexed} items indexed"), -1.0, false),
        IndexStatus::UpToDate { indexed } => (format!("Up to date · {indexed} items indexed"), -1.0, false),
        IndexStatus::Failed(message) => (format!("Indexing stopped: {message}"), -1.0, false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn statuses_read_naturally() {
        let indexing = IndexStatus::Indexing { done: 4, total: 10, current: "notes.md".into() };
        assert_eq!(status_line(&indexing), ("Indexing 5 of 10: notes.md".to_owned(), 0.4, true));
        assert_eq!(status_line(&IndexStatus::UpToDate { indexed: 3 }).0, "Up to date · 3 items indexed");
        let home = StoragePath::parse("file:///Users/me").unwrap();
        assert_eq!(shown(&StoragePath::parse("file:///Users/me/Documents").unwrap(), Some(&home)), "~/Documents");
    }
}
