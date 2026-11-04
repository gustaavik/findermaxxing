//! `OperationsController`: one per window. Starts file-operation jobs, shows
//! their progress, asks about conflicts, and drives undo/redo and the clipboard.
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
use application::use_cases::TransferRequest;
use cxx_qt::{CxxQtType, Threading};
use cxx_qt_lib::{QList, QString, QStringList};
use domain::{Conflict, ConflictChoice, ConflictResolution, ConflictScope, EntryKind, PathSegment, StoragePath};
use futures::channel::oneshot;

use crate::clipboard_bridge::{clipboard_urls, set_clipboard_urls};
use crate::error_messages::{open_message, operation_message};
use crate::file_urls::{from_file_url, to_file_url};
use crate::job_reporting::{QtConflictResolver, QtJobObserver};
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
        fn dismiss_error(self: Pin<&mut OperationsController>);
        /// Re-reads the undo/redo labels after work done outside this controller.
        #[qinvokable]
        fn refresh_history(self: Pin<&mut OperationsController>);

        /// A folder made by `newFolder`; the view selects it and starts renaming.
        #[qsignal]
        fn folder_created(self: Pin<&mut OperationsController>, location: QString);
        /// An item now lives at `location` after `rename`.
        #[qsignal]
        fn renamed(self: Pin<&mut OperationsController>, location: QString);
    }

    impl cxx_qt::Threading for OperationsController {}
    impl cxx_qt::Initialize for OperationsController {}
}

use qobject::{AnswerReach, ConflictAnswer, JobListModel, PasteMode, ServicesHolder};

const NOT_READY_MESSAGE: &str = "The application is still starting.";
const INVALID_NAME_MESSAGE: &str = "Names can't be empty or contain \u{201c}/\u{201d}.";
const NOTHING_LOCAL_MESSAGE: &str = "Only items on this computer can be copied to the clipboard for now.";
const PREPARING_DETAIL: &str = "Preparing…";

/// A conflict waiting for the user, oldest first.
struct PendingConflict {
    job_id: i32,
    conflict: Conflict,
    reply: oneshot::Sender<ConflictResolution>,
}

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
    running: HashMap<i32, CancellationSource>,
    next_job_id: i32,
    conflicts: VecDeque<PendingConflict>,
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
            running: HashMap::new(),
            next_job_id: 1,
            conflicts: VecDeque::new(),
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
    pub fn copy_to_clipboard(self: Pin<&mut Self>, locations: &QStringList) {
        let Some(services) = self.app_services() else { return self.report(NOT_READY_MESSAGE) };
        let urls: Vec<String> =
            decode_all(locations).iter().filter_map(|path| to_file_url(path, services.home.mount())).collect();
        if urls.is_empty() {
            return self.report(NOTHING_LOCAL_MESSAGE);
        }
        set_clipboard_urls(&urls);
    }

    pub fn can_paste(&self) -> bool {
        !clipboard_urls().is_empty()
    }

    pub fn paste(self: Pin<&mut Self>, destination: &QString, mode: PasteMode) {
        let Some(services) = self.app_services() else { return self.report(NOT_READY_MESSAGE) };
        let Ok(destination) = StoragePath::decode(&destination.to_string()) else { return };
        let local = services.home.mount();
        let sources: Vec<StoragePath> = clipboard_urls().iter().filter_map(|url| from_file_url(url, local)).collect();
        if sources.is_empty() {
            return;
        }
        let ops = services.operations;
        let request = TransferRequest { sources, destination };
        match mode {
            PasteMode::Move => {
                let title = transfer_title(TransferVerb::Moving, &request.sources, &request.destination);
                let use_case = ops.move_entries;
                self.spawn_job(title, move |ctx| async move { use_case.execute(&request, &ctx).await.result });
            }
            PasteMode::Copy | PasteMode { .. } => {
                let title = transfer_title(TransferVerb::Copying, &request.sources, &request.destination);
                let use_case = ops.copy;
                self.spawn_job(title, move |ctx| async move { use_case.execute(&request, &ctx).await.result });
            }
        }
    }

    pub fn move_to_trash(self: Pin<&mut Self>, locations: &QStringList) {
        let Some(services) = self.app_services() else { return self.report(NOT_READY_MESSAGE) };
        let items = decode_all(locations);
        if items.is_empty() {
            return;
        }
        let use_case = services.operations.trash;
        self.spawn_job(trash_title(&items), move |ctx| async move { use_case.execute(&items, &ctx).await.result });
    }

    pub fn delete_permanently(self: Pin<&mut Self>, locations: &QStringList) {
        let Some(services) = self.app_services() else { return self.report(NOT_READY_MESSAGE) };
        let items = decode_all(locations);
        if items.is_empty() {
            return;
        }
        let use_case = services.operations.delete;
        self.spawn_job(delete_title(&items), move |ctx| async move { use_case.execute(&items, &ctx).await });
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
        self.spawn_job(undo_title(entry.kind), move |ctx| async move {
            use_case.execute(&ctx).await.and_then(|report| report.result)
        });
    }

    pub fn redo(self: Pin<&mut Self>) {
        let Some(services) = self.app_services() else { return self.report(NOT_READY_MESSAGE) };
        let Some(entry) = services.operations.history.next_redo() else { return };
        let use_case = services.operations.redo;
        self.spawn_job(redo_title(entry.kind), move |ctx| async move {
            use_case.execute(&ctx).await.and_then(|report| report.result)
        });
    }

    pub fn cancel_job(mut self: Pin<&mut Self>, job_id: i32) {
        if let Some(source) = self.running.get(&job_id) {
            source.cancel();
        }
        // A job waiting for a conflict answer would otherwise never notice.
        self.as_mut().drop_conflicts_of(job_id, Some(STOP_ALL));
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
        self.as_mut().rust_mut().running.remove(&job_id);
        if let Some(model) = self.as_mut().rust_mut().owned_jobs.as_mut() {
            model.remove_job(job_id);
        }
        self.as_mut().drop_conflicts_of(job_id, None);
        if let Err(error) = result {
            self.as_mut().report_operation_error(&error);
        }
        self.refresh_history();
    }

    fn app_services(&self) -> Option<AppServices> {
        // SAFETY: the holder is created by the bootstrap before the QML engine
        // and destroyed after it, so it outlives every controller.
        let holder = unsafe { self.services().as_ref() }?;
        Some(holder.services().clone())
    }

    fn report(self: Pin<&mut Self>, message: &str) {
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

    fn spawn_job<Fut>(mut self: Pin<&mut Self>, title: String, work: impl FnOnce(JobContext) -> Fut)
    where
        Fut: Future<Output = Result<(), OperationError>> + Send + 'static,
    {
        let Some(services) = self.app_services() else { return self.report(NOT_READY_MESSAGE) };
        let job_id = self.next_job_id;
        let source = CancellationSource::new();
        let thread = self.qt_thread();
        let ctx = JobContext {
            observer: Arc::new(QtJobObserver::new(job_id, thread.clone())),
            resolver: Arc::new(QtConflictResolver::new(job_id, thread.clone())),
            cancel: source.token(),
        };
        let job = work(ctx);
        let mut rust = self.as_mut().rust_mut();
        rust.next_job_id = job_id.wrapping_add(1);
        rust.running.insert(job_id, source);
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

fn decode_all(locations: &QStringList) -> Vec<StoragePath> {
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
    let detail = format!(
        "Existing: {}. New: {}. Replacing moves the existing item to the Trash.",
        describe(&conflict.existing),
        describe(&conflict.incoming)
    );
    (title, detail)
}
