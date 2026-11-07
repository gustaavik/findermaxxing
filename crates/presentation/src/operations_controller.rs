//! `OperationsController`: one per window. Starts file-operation jobs, shows
//! their progress, asks about conflicts, and drives undo/redo, the clipboard,
//! drag and drop, offline copies and resuming transfers a quit interrupted.
//!
//! Thin by design: every decision about *what* happens to files is made by
//! the use cases; this object turns QML intents into use-case calls and job
//! events into model rows and properties.

use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use application::CancellationSource;
use application::operations::{JobContext, OperationError};
use application::ports::{InterruptedTransfer, JournaledKind};
use application::use_cases::TransferRequest;
use cxx_qt::{CxxQtType, Threading};
use cxx_qt_lib::{QList, QString, QStringList};
use domain::{
    AccountId, Conflict, ConflictChoice, ConflictResolution, ConflictScope, CostDecision, CostReason, CostWarning,
    EntryKind, Operation, PathSegment, S3Location, StoragePath,
};
use futures::channel::oneshot;

use crate::clipboard_bridge::{clipboard_urls, set_clipboard_urls};
use crate::error_messages::{open_message, operation_message};
use crate::file_urls::{from_file_url, native_file_url, to_file_url};
use crate::input_bridge::{DropModifier, drop_modifier};
use crate::job_reporting::{QtConflictResolver, QtCostConfirmer, QtJobObserver};
use crate::job_text::{
    TransferVerb, delete_title, redo_menu_text, redo_title, transfer_title, trash_title, undo_menu_text, undo_title,
};
use crate::row_format::format_size;
use crate::rows_bridge::JobRow;
use crate::services::AppServices;

#[cxx_qt::bridge]
pub mod qobject {
    unsafe extern "C++" {
        include!("cxx-qt-lib/qstring.h");
        type QString = cxx_qt_lib::QString;

        include!("cxx-qt-lib/qstringlist.h");
        type QStringList = cxx_qt_lib::QStringList;

        include!("presentation/src/rows_bridge.cxx.h");
        type JobRow = crate::rows_bridge::JobRow;

        include!("presentation/cpp/services_holder.h");
        type ServicesHolder = crate::services_bridge::ServicesHolder;
    }

    unsafe extern "C++Qt" {
        include!("presentation/cpp/job_list_model.h");
        #[qobject]
        type JobListModel;

        #[cxx_name = "addJob"]
        fn add_job(self: Pin<&mut JobListModel>, row: JobRow);
        #[cxx_name = "updateProgress"]
        fn update_progress(self: Pin<&mut JobListModel>, job_id: i32, detail: String, fraction: f64, determinate: bool);
        #[cxx_name = "removeJob"]
        fn remove_job(self: Pin<&mut JobListModel>, job_id: i32);
    }

    unsafe extern "C++" {
        #[cxx_name = "newJobListModel"]
        fn new_job_list_model() -> UniquePtr<JobListModel>;
    }

    #[qenum(OperationsController)]
    enum PasteMode {
        Copy,
        Move,
    }

    #[qenum(OperationsController)]
    enum ConflictAnswer {
        Replace,
        Skip,
        KeepBoth,
        Stop,
    }

    #[qenum(OperationsController)]
    enum CostAnswer {
        Continue,
        CancelTransfer,
    }

    #[qenum(OperationsController)]
    enum CostReach {
        AskAgain,
        StopAsking,
    }

    /// What a drop does: Finder's rules (move within a volume, copy across)
    /// unless a modifier key says otherwise. QML shares one namespace for all
    /// of a type's enum values, so these must not reuse `PasteMode`'s names.
    #[qenum(OperationsController)]
    enum DropOperation {
        FollowKeys,
        AlwaysCopy,
        AlwaysMove,
    }

    /// How converted images are sized.
    #[qenum(OperationsController)]
    enum ResizeMode {
        KeepSize,
        ScalePercent,
        FitWithin,
    }

    /// Which items a permission change reaches.
    #[qenum(OperationsController)]
    enum PermissionScope {
        SelectedOnly,
        AllInside,
        FilesInside,
        FoldersInside,
    }

    #[qenum(OperationsController)]
    enum AnswerReach {
        ThisItem,
        AllRemaining,
    }

    #[auto_cxx_name]
    unsafe extern "RustQt" {
        #[qobject]
        #[qml_element]
        #[qproperty(*mut ServicesHolder, services)]
        #[qproperty(*mut JobListModel, jobs)]
        #[qproperty(bool, can_undo)]
        #[qproperty(bool, can_redo)]
        #[qproperty(QString, undo_text)]
        #[qproperty(QString, redo_text)]
        #[qproperty(bool, conflict_pending)]
        #[qproperty(QString, conflict_title)]
        #[qproperty(QString, conflict_detail)]
        #[qproperty(QString, error_message)]
        #[qproperty(bool, cost_pending)]
        #[qproperty(QString, cost_title)]
        #[qproperty(QString, cost_detail)]
        /// Copy and move jobs a quit or crash cut short, waiting for Resume or Discard.
        #[qproperty(i32, interrupted_count)]
        #[qproperty(QString, interrupted_text)]
        type OperationsController = super::OperationsControllerRust;
    }

    #[auto_cxx_name]
    extern "RustQt" {
        #[qinvokable]
        fn copy_to_clipboard(self: Pin<&mut OperationsController>, locations: &QStringList);
        #[qinvokable]
        fn can_paste(self: &OperationsController) -> bool;
        #[qinvokable]
        fn paste(self: Pin<&mut OperationsController>, destination: &QString, mode: PasteMode);
        #[qinvokable]
        fn move_to_trash(self: Pin<&mut OperationsController>, locations: &QStringList);
        #[qinvokable]
        fn delete_permanently(self: Pin<&mut OperationsController>, locations: &QStringList);
        #[qinvokable]
        fn rename(self: Pin<&mut OperationsController>, location: &QString, new_name: &QString);
        #[qinvokable]
        fn new_folder(self: Pin<&mut OperationsController>, parent: &QString, base_name: &QString);
        #[qinvokable]
        fn open(self: Pin<&mut OperationsController>, location: &QString);
        #[qinvokable]
        fn undo(self: Pin<&mut OperationsController>);
        #[qinvokable]
        fn redo(self: Pin<&mut OperationsController>);
        #[qinvokable]
        fn cancel_job(self: Pin<&mut OperationsController>, job_id: i32);
        #[qinvokable]
        fn answer_conflict(self: Pin<&mut OperationsController>, answer: ConflictAnswer, reach: AnswerReach);
        #[qinvokable]
        fn answer_cost(self: Pin<&mut OperationsController>, answer: CostAnswer, reach: CostReach);
        #[qinvokable]
        fn dismiss_error(self: Pin<&mut OperationsController>);
        /// Re-reads the undo/redo labels after work done outside this controller.
        #[qinvokable]
        fn refresh_history(self: Pin<&mut OperationsController>);
        /// Downloads items (folders recursively) and keeps them in the cache.
        #[qinvokable]
        fn make_available_offline(self: Pin<&mut OperationsController>, locations: &QStringList);
        #[qinvokable]
        fn remove_offline_copy(self: Pin<&mut OperationsController>, location: &QString);
        /// Looks for transfers a quit or crash interrupted (at start-up).
        /// Archive formats for "Compress", by display name.
        #[qinvokable]
        fn archive_formats(self: &OperationsController) -> QStringList;
        #[qinvokable]
        fn compress(self: Pin<&mut OperationsController>, locations: &QStringList, format: i32);
        #[qinvokable]
        fn can_extract(self: &OperationsController, location: &QString) -> bool;
        #[qinvokable]
        fn extract(self: Pin<&mut OperationsController>, location: &QString);
        #[qinvokable]
        fn image_formats(self: &OperationsController) -> QStringList;
        #[qinvokable]
        fn is_image(self: &OperationsController, location: &QString) -> bool;
        /// `size` is a percentage for `ScalePercent`, the longest side in pixels for `FitWithin`.
        #[qinvokable]
        fn convert_images(
            self: Pin<&mut OperationsController>,
            locations: &QStringList,
            format: i32,
            mode: ResizeMode,
            size: i32,
            quality: i32,
        );
        #[qinvokable]
        fn clean_metadata(self: Pin<&mut OperationsController>, locations: &QStringList);
        /// `edit`: nine characters (owner, group, others × read, write, execute),
        /// each `+` set, `-` clear or `=` leave as is.
        #[qinvokable]
        fn change_permissions(
            self: Pin<&mut OperationsController>,
            locations: &QStringList,
            edit: &QString,
            scope: PermissionScope,
        );
        #[qinvokable]
        fn secure_delete(self: Pin<&mut OperationsController>, locations: &QStringList);

        #[qinvokable]
        fn check_interrupted(self: Pin<&mut OperationsController>);
        #[qinvokable]
        fn resume_interrupted(self: Pin<&mut OperationsController>);
        #[qinvokable]
        fn discard_interrupted(self: Pin<&mut OperationsController>);
        /// A drop on `destination`: `internal` holds our own locations (one per
        /// line) when the drag came from this app; otherwise `urls` are
        /// `file://` URLs from another app.
        #[qinvokable]
        fn drop_items(
            self: Pin<&mut OperationsController>,
            urls: &QStringList,
            internal: &QString,
            destination: &QString,
            operation: DropOperation,
        );
        /// Gets local files for dragging items out to other apps; remote items
        /// are downloaded first. Answers with `drag_ready`.
        #[qinvokable]
        fn prepare_drag(self: Pin<&mut OperationsController>, locations: &QStringList);

        /// A folder made by `newFolder`; the view selects it and starts renaming.
        #[qsignal]
        fn folder_created(self: Pin<&mut OperationsController>, location: QString);
        /// An item now lives at `location` after `rename`.
        #[qsignal]
        fn renamed(self: Pin<&mut OperationsController>, location: QString);
        /// Work finished that changed these folders; views of folders that
        /// can't be watched (S3) reload.
        #[qsignal]
        fn folders_changed(self: Pin<&mut OperationsController>, locations: QStringList);
        /// `urls` are local `file://` URLs for other apps; `internal` the
        /// original locations, one per line, for drops inside this app.
        #[qsignal]
        fn drag_ready(self: Pin<&mut OperationsController>, urls: QStringList, internal: QString);
    }

    impl cxx_qt::Threading for OperationsController {}
    impl cxx_qt::Initialize for OperationsController {}
}

use qobject::{
    AnswerReach, ConflictAnswer, CostAnswer, CostReach, DropOperation, JobListModel, PasteMode, ServicesHolder,
};

pub(crate) const NOT_READY_MESSAGE: &str = "The application is still starting.";
const INVALID_NAME_MESSAGE: &str = "Names can't be empty or contain \u{201c}/\u{201d}.";
const PREPARING_DETAIL: &str = "Preparing…";
const LOCATION_SEPARATOR: char = '\n';

/// A conflict waiting for the user, oldest first.
/// A cost confirmation waiting for the user.
struct PendingCost {
    job_id: i32,
    warning: CostWarning,
    accounts: Vec<AccountId>,
    reply: oneshot::Sender<CostDecision>,
}

/// What the controller tracks per running job.
struct RunningJob {
    cancellation: CancellationSource,
    /// Folders the job changes, announced when it ends.
    affected: Vec<StoragePath>,
}

struct PendingConflict {
    job_id: i32,
    conflict: Conflict,
    reply: oneshot::Sender<ConflictResolution>,
}

// The bools are Q_PROPERTY storage, which cxx-qt requires as plain fields.
#[allow(clippy::struct_excessive_bools, reason = "mirrors Qt properties one-to-one")]
pub struct OperationsControllerRust {
    services: *mut ServicesHolder,
    jobs: *mut JobListModel,
    can_undo: bool,
    can_redo: bool,
    undo_text: QString,
    redo_text: QString,
    conflict_pending: bool,
    conflict_title: QString,
    conflict_detail: QString,
    error_message: QString,

    owned_jobs: cxx::UniquePtr<JobListModel>,
    cost_pending: bool,
    cost_title: QString,
    cost_detail: QString,
    running: HashMap<i32, RunningJob>,
    costs: VecDeque<PendingCost>,
    /// Everything last copied, including items without a file URL (S3)…
    clipboard: Vec<StoragePath>,
    /// …and the file URLs put on the system clipboard at that moment, to tell
    /// whether another app has copied something since.
    clipboard_urls_sent: Vec<String>,
    next_job_id: i32,
    conflicts: VecDeque<PendingConflict>,
    interrupted_count: i32,
    interrupted_text: QString,
    interrupted: Vec<InterruptedTransfer>,
}

impl Default for OperationsControllerRust {
    fn default() -> Self {
        Self {
            services: std::ptr::null_mut(),
            jobs: std::ptr::null_mut(),
            can_undo: false,
            can_redo: false,
            undo_text: QString::default(),
            redo_text: QString::default(),
            conflict_pending: false,
            conflict_title: QString::default(),
            conflict_detail: QString::default(),
            error_message: QString::default(),
            owned_jobs: cxx::UniquePtr::null(),
            cost_pending: false,
            cost_title: QString::default(),
            cost_detail: QString::default(),
            running: HashMap::new(),
            costs: VecDeque::new(),
            clipboard: Vec::new(),
            clipboard_urls_sent: Vec::new(),
            next_job_id: 1,
            conflicts: VecDeque::new(),
            interrupted_count: 0,
            interrupted_text: QString::default(),
            interrupted: Vec::new(),
        }
    }
}

impl cxx_qt::Initialize for qobject::OperationsController {
    fn initialize(mut self: Pin<&mut Self>) {
        let owned_jobs = qobject::new_job_list_model();
        let jobs = owned_jobs.as_mut_ptr();
        self.as_mut().rust_mut().owned_jobs = owned_jobs;
        self.set_jobs(jobs);
    }
}

impl qobject::OperationsController {
    pub fn copy_to_clipboard(mut self: Pin<&mut Self>, locations: &QStringList) {
        let Some(services) = self.app_services() else { return self.report(NOT_READY_MESSAGE) };
        let items = decode_all(locations);
        let urls: Vec<String> = items.iter().filter_map(|path| to_file_url(path, services.home.mount())).collect();
        set_clipboard_urls(&urls);
        let mut rust = self.as_mut().rust_mut();
        rust.clipboard = items;
        rust.clipboard_urls_sent = urls;
    }

    pub fn can_paste(&self) -> bool {
        !self.clipboard.is_empty() || !clipboard_urls().is_empty()
    }

    /// Our own clipboard while the system clipboard still holds what we put
    /// there; otherwise whatever another app copied.
    fn clipboard_items(&self, local: &domain::MountId) -> Vec<StoragePath> {
        let system = clipboard_urls();
        if system == self.clipboard_urls_sent || system.is_empty() {
            self.clipboard.clone()
        } else {
            system.iter().filter_map(|url| from_file_url(url, local)).collect()
        }
    }

    pub fn paste(self: Pin<&mut Self>, destination: &QString, mode: PasteMode) {
        let Some(services) = self.app_services() else { return self.report(NOT_READY_MESSAGE) };
        let Ok(destination) = StoragePath::decode(&destination.to_string()) else { return };
        let sources = self.clipboard_items(services.home.mount());
        if sources.is_empty() {
            return;
        }
        let ops = services.operations;
        let mut affected = parents(&sources);
        affected.push(destination.clone());
        let request = TransferRequest { sources, destination };
        match mode {
            PasteMode::Move => {
                let title = transfer_title(TransferVerb::Moving, &request.sources, &request.destination);
                let use_case = ops.move_entries;
                self.spawn_job(
                    title,
                    affected,
                    move |ctx| async move { use_case.execute(&request, &ctx).await.result },
                );
            }
            PasteMode::Copy | PasteMode { .. } => {
                let title = transfer_title(TransferVerb::Copying, &request.sources, &request.destination);
                let use_case = ops.copy;
                self.spawn_job(
                    title,
                    affected,
                    move |ctx| async move { use_case.execute(&request, &ctx).await.result },
                );
            }
        }
    }

    pub fn make_available_offline(mut self: Pin<&mut Self>, locations: &QStringList) {
        let Some(services) = self.app_services() else { return self.report(NOT_READY_MESSAGE) };
        for location in decode_all(locations) {
            let use_case = services.offline.make_available.clone();
            let title = offline_title(&location);
            self.as_mut().spawn_job(
                title,
                Vec::new(),
                move |ctx| async move { use_case.execute(&location, &ctx).await },
            );
        }
    }

    pub fn remove_offline_copy(self: Pin<&mut Self>, location: &QString) {
        let Some(services) = self.app_services() else { return self.report(NOT_READY_MESSAGE) };
        let Ok(location) = StoragePath::decode(&location.to_string()) else { return };
        let use_case = services.offline.remove;
        self.spawn_task(async move { use_case.execute(location).await }, |controller, result| {
            if let Err(error) = result {
                controller.report(&format!("The offline copy couldn't be removed: {error}"));
            }
        });
    }

    pub fn check_interrupted(self: Pin<&mut Self>) {
        let Some(services) = self.app_services() else { return };
        let use_case = services.operations.interrupted;
        self.spawn_task(async move { use_case.execute().await.unwrap_or_default() }, |mut controller, found| {
            let text = interrupted_text(&found);
            controller.as_mut().set_interrupted_text(QString::from(&text));
            controller.as_mut().set_interrupted_count(i32::try_from(found.len()).unwrap_or(i32::MAX));
            controller.rust_mut().interrupted = found;
        });
    }

    pub fn resume_interrupted(mut self: Pin<&mut Self>) {
        let Some(services) = self.app_services() else { return self.report(NOT_READY_MESSAGE) };
        let interrupted = std::mem::take(&mut self.as_mut().rust_mut().interrupted);
        self.as_mut().set_interrupted_count(0);
        for transfer in interrupted {
            let entry = &transfer.entry;
            let verb = match entry.kind {
                JournaledKind::Copy => TransferVerb::Copying,
                JournaledKind::Move => TransferVerb::Moving,
            };
            let title = transfer_title(verb, &entry.sources, &entry.destination);
            let mut affected = parents(&entry.sources);
            affected.push(entry.destination.clone());
            let use_case = services.operations.resume.clone();
            self.as_mut()
                .spawn_job(title, affected, move |ctx| async move { use_case.execute(&transfer, &ctx).await.result });
        }
    }

    pub fn discard_interrupted(mut self: Pin<&mut Self>) {
        let Some(services) = self.app_services() else { return };
        let interrupted = std::mem::take(&mut self.as_mut().rust_mut().interrupted);
        self.as_mut().set_interrupted_count(0);
        let use_case = services.operations.discard;
        services.runtime.spawn(async move {
            for transfer in &interrupted {
                // A row that can't be cleared shows up again next start.
                let _ = use_case.execute(transfer).await;
            }
        });
    }

    pub fn drop_items(
        self: Pin<&mut Self>,
        urls: &QStringList,
        internal: &QString,
        destination: &QString,
        operation: DropOperation,
    ) {
        let Some(services) = self.app_services() else { return self.report(NOT_READY_MESSAGE) };
        let Ok(destination) = StoragePath::decode(&destination.to_string()) else { return };
        let internal = internal.to_string();
        let sources: Vec<StoragePath> = if internal.trim().is_empty() {
            let local = services.home.mount().clone();
            QList::<QString>::from(urls).iter().filter_map(|url| from_file_url(&url.to_string(), &local)).collect()
        } else {
            internal.split(LOCATION_SEPARATOR).filter_map(|line| StoragePath::decode(line.trim()).ok()).collect()
        };
        // Dropping items back into the folder they came from does nothing.
        let sources: Vec<StoragePath> =
            sources.into_iter().filter(|source| source.parent().as_ref() != Some(&destination)).collect();
        if sources.is_empty() {
            return;
        }
        let mode = drop_mode(drop_operation_now(operation), &sources, &destination);
        let mut affected = parents(&sources);
        affected.push(destination.clone());
        let request = TransferRequest { sources, destination };
        let ops = services.operations;
        if mode == PasteMode::Move {
            let title = transfer_title(TransferVerb::Moving, &request.sources, &request.destination);
            let use_case = ops.move_entries;
            self.spawn_job(title, affected, move |ctx| async move { use_case.execute(&request, &ctx).await.result });
        } else {
            let title = transfer_title(TransferVerb::Copying, &request.sources, &request.destination);
            let use_case = ops.copy;
            self.spawn_job(title, affected, move |ctx| async move { use_case.execute(&request, &ctx).await.result });
        }
    }

    pub fn prepare_drag(self: Pin<&mut Self>, locations: &QStringList) {
        let Some(services) = self.app_services() else { return self.report(NOT_READY_MESSAGE) };
        let items = decode_all(locations);
        let internal = items.iter().map(StoragePath::encode).collect::<Vec<_>>().join(&LOCATION_SEPARATOR.to_string());
        let (mounts, local) = (services.mounts.clone(), services.home.mount().clone());
        self.spawn_task(
            async move {
                let mut urls = Vec::new();
                for item in &items {
                    if let Some(url) = to_file_url(item, &local) {
                        urls.push(url);
                    } else if let Ok(materializer) = mounts.resolve(item.mount()).and_then(|m| m.materializer()) {
                        // Folders can't be materialised yet; other apps get the files.
                        if let Ok(file) = materializer.local_file_boxed(item).await {
                            urls.push(native_file_url(file.path()));
                        }
                    }
                }
                (urls, internal)
            },
            |controller, (urls, internal)| {
                let urls: QStringList = urls.iter().map(QString::from).collect();
                controller.drag_ready(urls, QString::from(&internal));
            },
        );
    }

    pub fn move_to_trash(self: Pin<&mut Self>, locations: &QStringList) {
        let Some(services) = self.app_services() else { return self.report(NOT_READY_MESSAGE) };
        let items = decode_all(locations);
        if items.is_empty() {
            return;
        }
        let use_case = services.operations.trash;
        let (title, affected) = (trash_title(&items), parents(&items));
        self.spawn_job(title, affected, move |ctx| async move { use_case.execute(&items, &ctx).await.result });
    }

    pub fn delete_permanently(self: Pin<&mut Self>, locations: &QStringList) {
        let Some(services) = self.app_services() else { return self.report(NOT_READY_MESSAGE) };
        let items = decode_all(locations);
        if items.is_empty() {
            return;
        }
        let use_case = services.operations.delete;
        let (title, affected) = (delete_title(&items), parents(&items));
        self.spawn_job(title, affected, move |ctx| async move { use_case.execute(&items, &ctx).await });
    }

    pub fn rename(self: Pin<&mut Self>, location: &QString, new_name: &QString) {
        let Some(services) = self.app_services() else { return self.report(NOT_READY_MESSAGE) };
        let Ok(item) = StoragePath::decode(&location.to_string()) else { return };
        let Ok(name) = PathSegment::new(new_name.to_string().trim()) else {
            return self.report(INVALID_NAME_MESSAGE);
        };
        let use_case = services.operations.rename;
        self.spawn_task(async move { use_case.execute(&item, name).await }, |mut controller, result| match result {
            Ok(renamed) => {
                controller.as_mut().announce_changes(&parents(std::slice::from_ref(&renamed)));
                controller.as_mut().renamed(QString::from(&renamed.encode()));
                controller.refresh_history();
            }
            Err(error) => controller.report_operation_error(&error),
        });
    }

    pub fn new_folder(self: Pin<&mut Self>, parent: &QString, base_name: &QString) {
        let Some(services) = self.app_services() else { return self.report(NOT_READY_MESSAGE) };
        let (Ok(parent), Ok(base)) =
            (StoragePath::decode(&parent.to_string()), PathSegment::new(base_name.to_string().trim()))
        else {
            return;
        };
        let use_case = services.operations.create_folder;
        self.spawn_task(async move { use_case.execute(&parent, &base).await }, |mut controller, result| match result {
            Ok(created) => {
                controller.as_mut().announce_changes(&parents(std::slice::from_ref(&created)));
                controller.as_mut().folder_created(QString::from(&created.encode()));
                controller.refresh_history();
            }
            Err(error) => controller.report_operation_error(&error),
        });
    }

    pub fn open(self: Pin<&mut Self>, location: &QString) {
        let Some(services) = self.app_services() else { return self.report(NOT_READY_MESSAGE) };
        let Ok(item) = StoragePath::decode(&location.to_string()) else { return };
        let use_case = services.operations.open;
        self.spawn_task(async move { (use_case.execute(&item).await, item) }, |controller, (result, item)| {
            if let Err(error) = result {
                controller.report(&open_message(&item, &error));
            }
        });
    }

    pub fn undo(self: Pin<&mut Self>) {
        let Some(services) = self.app_services() else { return self.report(NOT_READY_MESSAGE) };
        let Some(entry) = services.operations.history.next_undo() else { return };
        let use_case = services.operations.undo;
        self.spawn_job(undo_title(entry.kind), affected_folders(&entry.operation), move |ctx| async move {
            use_case.execute(&ctx).await.and_then(|report| report.result)
        });
    }

    pub fn redo(self: Pin<&mut Self>) {
        let Some(services) = self.app_services() else { return self.report(NOT_READY_MESSAGE) };
        let Some(entry) = services.operations.history.next_redo() else { return };
        let use_case = services.operations.redo;
        self.spawn_job(redo_title(entry.kind), affected_folders(&entry.operation), move |ctx| async move {
            use_case.execute(&ctx).await.and_then(|report| report.result)
        });
    }

    pub fn cancel_job(mut self: Pin<&mut Self>, job_id: i32) {
        if let Some(job) = self.running.get(&job_id) {
            job.cancellation.cancel();
        }
        // A job waiting for an answer would otherwise never notice.
        self.as_mut().drop_conflicts_of(job_id, Some(STOP_ALL));
        self.drop_costs_of(job_id, Some(CostDecision::Cancel));
    }

    pub fn answer_cost(mut self: Pin<&mut Self>, answer: CostAnswer, reach: CostReach) {
        let Some(pending) = self.as_mut().rust_mut().costs.pop_front() else { return };
        let decision = match (answer, reach) {
            (CostAnswer::Continue, CostReach::StopAsking) => CostDecision::ProceedAndStopAsking,
            (CostAnswer::Continue, CostReach::AskAgain | CostReach { .. }) => CostDecision::Proceed,
            (CostAnswer::CancelTransfer | CostAnswer { .. }, _) => CostDecision::Cancel,
        };
        if decision == CostDecision::ProceedAndStopAsking {
            self.as_mut().stop_cost_warnings(&pending.accounts);
        }
        let _ = pending.reply.send(decision);
        self.show_next_cost();
    }

    pub(crate) fn enqueue_cost(
        mut self: Pin<&mut Self>,
        job_id: i32,
        warning: CostWarning,
        accounts: Vec<AccountId>,
        reply: oneshot::Sender<CostDecision>,
    ) {
        if !self.running.contains_key(&job_id) {
            return;
        }
        self.as_mut().rust_mut().costs.push_back(PendingCost { job_id, warning, accounts, reply });
        self.show_next_cost();
    }

    fn show_next_cost(mut self: Pin<&mut Self>) {
        let texts = self.costs.front().map(|pending| cost_texts(&pending.warning));
        let (title, detail) = texts.clone().unwrap_or_default();
        self.as_mut().set_cost_title(QString::from(&title));
        self.as_mut().set_cost_detail(QString::from(&detail));
        self.set_cost_pending(texts.is_some());
    }

    fn drop_costs_of(mut self: Pin<&mut Self>, job_id: i32, answer: Option<CostDecision>) {
        let pending = std::mem::take(&mut self.as_mut().rust_mut().costs);
        let (theirs, others): (VecDeque<_>, VecDeque<_>) = pending.into_iter().partition(|p| p.job_id == job_id);
        for cost in theirs {
            if let Some(answer) = answer {
                let _ = cost.reply.send(answer);
            }
        }
        self.as_mut().rust_mut().costs = others;
        self.show_next_cost();
    }

    fn stop_cost_warnings(self: Pin<&mut Self>, accounts: &[AccountId]) {
        let Some(services) = self.app_services() else { return };
        let (use_case, accounts) = (services.accounts.cost_warnings, accounts.to_vec());
        services.runtime.spawn(async move {
            for account in accounts {
                // Losing this preference only means being asked again.
                let _ = use_case.execute(account, true).await;
            }
        });
    }

    fn announce_changes(self: Pin<&mut Self>, folders: &[StoragePath]) {
        let encoded: QStringList = folders.iter().map(|f| QString::from(&f.encode())).collect();
        self.folders_changed(encoded);
    }

    pub fn answer_conflict(mut self: Pin<&mut Self>, answer: ConflictAnswer, reach: AnswerReach) {
        if let Some(pending) = self.as_mut().rust_mut().conflicts.pop_front() {
            let resolution = ConflictResolution { choice: conflict_choice(answer), scope: conflict_scope(reach) };
            // A dropped receiver means the job already ended.
            let _ = pending.reply.send(resolution);
        }
        self.show_next_conflict();
    }

    pub fn dismiss_error(self: Pin<&mut Self>) {
        self.set_error_message(QString::default());
    }

    pub(crate) fn update_job(mut self: Pin<&mut Self>, job_id: i32, detail: String, fraction: Option<f64>) {
        if !self.running.contains_key(&job_id) {
            return;
        }
        if let Some(model) = self.as_mut().rust_mut().owned_jobs.as_mut() {
            model.update_progress(job_id, detail, fraction.unwrap_or_default(), fraction.is_some());
        }
    }

    pub(crate) fn enqueue_conflict(
        mut self: Pin<&mut Self>,
        job_id: i32,
        conflict: Conflict,
        reply: oneshot::Sender<ConflictResolution>,
    ) {
        if !self.running.contains_key(&job_id) {
            return;
        }
        self.as_mut().rust_mut().conflicts.push_back(PendingConflict { job_id, conflict, reply });
        self.show_next_conflict();
    }

    pub(crate) fn finish_job(mut self: Pin<&mut Self>, job_id: i32, result: Result<(), OperationError>) {
        let finished = self.as_mut().rust_mut().running.remove(&job_id);
        if let Some(model) = self.as_mut().rust_mut().owned_jobs.as_mut() {
            model.remove_job(job_id);
        }
        self.as_mut().drop_conflicts_of(job_id, None);
        self.as_mut().drop_costs_of(job_id, None);
        if let Some(job) = finished {
            self.as_mut().announce_changes(&job.affected);
        }
        if let Err(error) = result {
            self.as_mut().report_operation_error(&error);
        }
        self.refresh_history();
    }

    pub(crate) fn app_services(&self) -> Option<AppServices> {
        // SAFETY: the holder is created by the bootstrap before the QML engine
        // and destroyed after it, so it outlives every controller.
        let holder = unsafe { self.services().as_ref() }?;
        Some(holder.services().clone())
    }

    pub(crate) fn report(self: Pin<&mut Self>, message: &str) {
        self.set_error_message(QString::from(message));
    }

    fn report_operation_error(self: Pin<&mut Self>, error: &OperationError) {
        if let Some(message) = operation_message(error) {
            self.report(&message);
        }
    }

    pub fn refresh_history(mut self: Pin<&mut Self>) {
        let Some(services) = self.app_services() else { return };
        let (undo, redo) = (services.operations.history.next_undo(), services.operations.history.next_redo());
        self.as_mut().set_can_undo(undo.is_some());
        self.as_mut().set_can_redo(redo.is_some());
        self.as_mut().set_undo_text(QString::from(&undo.map(|e| undo_menu_text(e.kind)).unwrap_or_default()));
        self.set_redo_text(QString::from(&redo.map(|e| redo_menu_text(e.kind)).unwrap_or_default()));
    }

    fn show_next_conflict(mut self: Pin<&mut Self>) {
        let texts = self.conflicts.front().map(|pending| conflict_texts(&pending.conflict));
        let (title, detail) = texts.clone().unwrap_or_default();
        self.as_mut().set_conflict_title(QString::from(&title));
        self.as_mut().set_conflict_detail(QString::from(&detail));
        self.set_conflict_pending(texts.is_some());
    }

    /// Answers (or, with `None`, just drops) every pending conflict of a job.
    fn drop_conflicts_of(mut self: Pin<&mut Self>, job_id: i32, answer: Option<ConflictResolution>) {
        let pending = std::mem::take(&mut self.as_mut().rust_mut().conflicts);
        let (theirs, others): (VecDeque<_>, VecDeque<_>) = pending.into_iter().partition(|p| p.job_id == job_id);
        for conflict in theirs {
            if let Some(answer) = answer {
                let _ = conflict.reply.send(answer);
            }
        }
        self.as_mut().rust_mut().conflicts = others;
        self.show_next_conflict();
    }

    pub(crate) fn spawn_job<Fut>(
        mut self: Pin<&mut Self>,
        title: String,
        affected: Vec<StoragePath>,
        work: impl FnOnce(JobContext) -> Fut,
    ) where
        Fut: Future<Output = Result<(), OperationError>> + Send + 'static,
    {
        let Some(services) = self.app_services() else { return self.report(NOT_READY_MESSAGE) };
        let job_id = self.next_job_id;
        let source = CancellationSource::new();
        let thread = self.qt_thread();
        let accounts = accounts_of(&affected);
        let ctx = JobContext {
            observer: Arc::new(QtJobObserver::new(job_id, thread.clone())),
            resolver: Arc::new(QtConflictResolver::new(job_id, thread.clone())),
            costs: Arc::new(QtCostConfirmer::new(job_id, thread.clone(), accounts, services.accounts.list.clone())),
            cancel: source.token(),
        };
        let job = work(ctx);
        let mut rust = self.as_mut().rust_mut();
        rust.next_job_id = job_id.wrapping_add(1);
        rust.running.insert(job_id, RunningJob { cancellation: source, affected });
        if let Some(model) = rust.owned_jobs.as_mut() {
            model.add_job(job_row(job_id, title, PREPARING_DETAIL.to_owned(), None));
        }
        services.runtime.spawn(async move {
            let result = job.await;
            // Fails only if the window is gone; the result is moot then.
            let _ = thread.queue(move |controller| controller.finish_job(job_id, result));
        });
    }

    /// Short operations without a jobs-panel row (rename, new folder, open).
    fn spawn_task<T, Fut>(
        self: Pin<&mut Self>,
        task: Fut,
        on_done: impl FnOnce(Pin<&mut qobject::OperationsController>, T) + Send + 'static,
    ) where
        T: Send + 'static,
        Fut: Future<Output = T> + Send + 'static,
    {
        let Some(services) = self.app_services() else { return self.report(NOT_READY_MESSAGE) };
        let thread = self.qt_thread();
        services.runtime.spawn(async move {
            let outcome = task.await;
            let _ = thread.queue(move |controller| on_done(controller, outcome));
        });
    }
}

const STOP_ALL: ConflictResolution =
    ConflictResolution { choice: ConflictChoice::Stop, scope: ConflictScope::AllRemaining };

fn job_row(id: i32, title: String, detail: String, fraction: Option<f64>) -> JobRow {
    JobRow { id, title, detail, fraction: fraction.unwrap_or_default(), determinate: fraction.is_some() }
}

/// The folders containing `items` (deduplicated, in order).
pub(crate) fn parents(items: &[StoragePath]) -> Vec<StoragePath> {
    let mut folders: Vec<StoragePath> = Vec::new();
    for parent in items.iter().filter_map(StoragePath::parent) {
        if !folders.contains(&parent) {
            folders.push(parent);
        }
    }
    folders
}

/// Folders an undo or redo of `operation` will change.
fn affected_folders(operation: &Operation) -> Vec<StoragePath> {
    let mut items = Vec::new();
    collect_items(operation, &mut items);
    parents(&items)
}

fn collect_items(operation: &Operation, items: &mut Vec<StoragePath>) {
    match operation {
        Operation::Copied(moves) | Operation::Moved(moves) => {
            items.extend(moves.iter().flat_map(|m| [m.from.clone(), m.to.clone()]));
        }
        Operation::Trashed(tickets) | Operation::Restored(tickets) => {
            items.extend(tickets.iter().map(|t| t.original.clone()));
        }
        Operation::CreatedFolder(path) => items.push(path.clone()),
        Operation::Deleted(paths) | Operation::Created(paths) => items.extend(paths.iter().cloned()),
        Operation::PermissionsChanged(changes) => items.extend(changes.iter().map(|(path, _)| path.clone())),
        Operation::Sequence(parts) => parts.iter().for_each(|part| collect_items(part, items)),
    }
}

/// The S3 accounts whose buckets these folders are in.
fn accounts_of(folders: &[StoragePath]) -> Vec<AccountId> {
    let mut accounts: Vec<AccountId> =
        folders.iter().filter_map(|f| S3Location::of(f.mount())).map(|l| l.account).collect();
    accounts.sort();
    accounts.dedup();
    accounts
}

/// Finder's rule: dragging within one volume moves, across volumes copies.
fn drop_mode(operation: DropOperation, sources: &[StoragePath], destination: &StoragePath) -> PasteMode {
    match operation {
        DropOperation::AlwaysCopy => PasteMode::Copy,
        DropOperation::AlwaysMove => PasteMode::Move,
        DropOperation::FollowKeys | DropOperation { .. } => {
            if sources.iter().all(|source| source.mount() == destination.mount()) {
                PasteMode::Move
            } else {
                PasteMode::Copy
            }
        }
    }
}

/// Automatic drops follow the modifier keys held at the moment of the drop.
fn drop_operation_now(requested: DropOperation) -> DropOperation {
    if requested != DropOperation::FollowKeys {
        return requested;
    }
    match drop_modifier() {
        DropModifier::ForceCopy => DropOperation::AlwaysCopy,
        DropModifier::ForceMove => DropOperation::AlwaysMove,
        DropModifier::None => DropOperation::FollowKeys,
    }
}

fn offline_title(location: &StoragePath) -> String {
    let name = location.name().map_or_else(|| "/".into(), PathSegment::display_name);
    format!("Making \u{201c}{name}\u{201d} available offline")
}

/// "Copying “a.mov” to “media”" (plus how many more), for the resume prompt.
fn interrupted_text(found: &[InterruptedTransfer]) -> String {
    let Some(first) = found.first() else { return String::new() };
    let verb = match first.entry.kind {
        JournaledKind::Copy => TransferVerb::Copying,
        JournaledKind::Move => TransferVerb::Moving,
    };
    let headline = transfer_title(verb, &first.entry.sources, &first.entry.destination);
    match found.len() {
        1 => headline,
        2 => format!("{headline}, and 1 more transfer"),
        count => format!("{headline}, and {} more transfers", count - 1),
    }
}

pub(crate) fn decode_all(locations: &QStringList) -> Vec<StoragePath> {
    QList::<QString>::from(locations).iter().filter_map(|text| StoragePath::decode(&text.to_string()).ok()).collect()
}

fn conflict_choice(answer: ConflictAnswer) -> ConflictChoice {
    match answer {
        ConflictAnswer::Replace => ConflictChoice::Replace,
        ConflictAnswer::KeepBoth => ConflictChoice::KeepBoth,
        ConflictAnswer::Skip => ConflictChoice::Skip,
        // Stop, and any value QML might send that is not a known answer.
        ConflictAnswer::Stop | ConflictAnswer { .. } => ConflictChoice::Stop,
    }
}

fn conflict_scope(reach: AnswerReach) -> ConflictScope {
    match reach {
        AnswerReach::AllRemaining => ConflictScope::AllRemaining,
        AnswerReach::ThisItem | AnswerReach { .. } => ConflictScope::ThisItem,
    }
}

fn conflict_texts(conflict: &Conflict) -> (String, String) {
    let name = conflict.existing.name().display_name();
    let place = conflict.existing.path().parent().and_then(|p| p.name().map(|n| n.display_name().into_owned()));
    let title = format!(
        "An item named \u{201c}{name}\u{201d} already exists in \u{201c}{}\u{201d}.",
        place.unwrap_or_else(|| "/".to_owned())
    );
    let describe = |entry: &domain::FileEntry| match (entry.kind(), entry.metadata().size) {
        (EntryKind::Directory, _) => "a folder".to_owned(),
        (_, Some(size)) => format_size(size),
        (_, None) => "a file".to_owned(),
    };
    let consequence = if conflict.replace_is_permanent {
        "This location has no Trash: replacing deletes the existing item permanently."
    } else {
        "Replacing moves the existing item to the Trash."
    };
    let detail =
        format!("Existing: {}. New: {}. {consequence}", describe(&conflict.existing), describe(&conflict.incoming));
    (title, detail)
}

fn cost_texts(warning: &CostWarning) -> (String, String) {
    let reasons: Vec<String> = warning
        .reasons
        .iter()
        .map(|reason| match reason {
            CostReason::LargeTransfer(size) => format!("transfers {}", format_size(*size)),
            CostReason::CrossRegion { from, to } => format!("crosses regions ({from} → {to})"),
        })
        .collect();
    let title = format!("This transfer {} and may be billed by your provider.", reasons.join(" and "));
    let items = if warning.items == 1 { "1 item".to_owned() } else { format!("{} items", warning.items) };
    let detail = format!(
        "{items}, {} in total. Storage providers charge for data transfer and requests.",
        format_size(warning.bytes)
    );
    (title, detail)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drops_move_within_a_mount_and_copy_across() {
        let folder = StoragePath::parse("file:///Users/me/Desktop").unwrap();
        let local = vec![StoragePath::parse("file:///Users/me/a.txt").unwrap()];
        let remote = vec![StoragePath::parse("s3://1@photos/b.jpg").unwrap()];
        assert!(drop_mode(DropOperation::FollowKeys, &local, &folder) == PasteMode::Move);
        assert!(drop_mode(DropOperation::FollowKeys, &remote, &folder) == PasteMode::Copy);
        assert!(drop_mode(DropOperation::AlwaysCopy, &local, &folder) == PasteMode::Copy);
        assert!(drop_mode(DropOperation::AlwaysMove, &remote, &folder) == PasteMode::Move);
    }
}
