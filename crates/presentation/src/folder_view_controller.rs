//! `FolderViewController`: the QML-facing object behind one tab.
//!
//! Thin by design: it owns navigation history and the active `FolderSession`,
//! forwards UI intents to them, and applies session events to the C++ list
//! model on the Qt thread. All listing/sorting/watching logic lives elsewhere.

use std::pin::Pin;

use application::use_cases::HiddenFiles;
use cxx_qt::{CxxQtType, Threading};
use cxx_qt_lib::QString;
use domain::{SortKey, SortOrder, SortSpec, StoragePath};

use crate::error_messages::user_message;
use crate::history::History;
use crate::services::AppServices;
use crate::session::{EventSink, FolderSession, SessionConfig, SessionEvent};
use crate::view_model::ModelUpdate;

#[cxx_qt::bridge]
pub mod qobject {
    unsafe extern "C++" {
        include!("cxx-qt-lib/qstring.h");
        type QString = cxx_qt_lib::QString;

        include!("presentation/src/rows_bridge.cxx.h");
        type EntryRow = crate::rows_bridge::EntryRow;

        include!("presentation/cpp/services_holder.h");
        type ServicesHolder = crate::services_bridge::ServicesHolder;
    }

    unsafe extern "C++Qt" {
        include!("presentation/cpp/entry_list_model.h");
        #[qobject]
        type EntryListModel;

        #[cxx_name = "replaceAll"]
        fn replace_all(self: Pin<&mut EntryListModel>, rows: Vec<EntryRow>);
        #[cxx_name = "insertRowAt"]
        fn insert_row_at(self: Pin<&mut EntryListModel>, index: usize, row: EntryRow);
        #[cxx_name = "removeRowAt"]
        fn remove_row_at(self: Pin<&mut EntryListModel>, index: usize);
        #[cxx_name = "replaceRowAt"]
        fn replace_row_at(self: Pin<&mut EntryListModel>, index: usize, row: EntryRow);
        fn rows(self: &EntryListModel) -> &Vec<EntryRow>;
    }

    unsafe extern "C++" {
        #[cxx_name = "newEntryListModel"]
        fn new_entry_list_model() -> UniquePtr<EntryListModel>;
    }

    #[qenum(FolderViewController)]
    enum SortColumn {
        Name,
        Size,
        Modified,
        Kind,
    }

    #[auto_cxx_name]
    unsafe extern "RustQt" {
        #[qobject]
        #[qml_element]
        #[qproperty(*mut ServicesHolder, services)]
        #[qproperty(*mut EntryListModel, model)]
        #[qproperty(QString, location)]
        #[qproperty(QString, title)]
        #[qproperty(QString, error_message)]
        #[qproperty(bool, loading)]
        #[qproperty(bool, can_go_back)]
        #[qproperty(bool, can_go_forward)]
        #[qproperty(bool, can_go_up)]
        #[qproperty(bool, show_hidden)]
        #[qproperty(i32, item_count)]
        #[qproperty(i32, sort_column)]
        #[qproperty(bool, sort_ascending)]
        type FolderViewController = super::FolderViewControllerRust;
    }

    #[auto_cxx_name]
    extern "RustQt" {
        #[qinvokable]
        fn open_home(self: Pin<&mut FolderViewController>);
        #[qinvokable]
        fn open_location(self: Pin<&mut FolderViewController>, location: &QString);
        #[qinvokable]
        fn activate(self: Pin<&mut FolderViewController>, row: i32);
        #[qinvokable]
        fn go_up(self: Pin<&mut FolderViewController>);
        #[qinvokable]
        fn go_back(self: Pin<&mut FolderViewController>);
        #[qinvokable]
        fn go_forward(self: Pin<&mut FolderViewController>);
        #[qinvokable]
        fn reload(self: Pin<&mut FolderViewController>);
        #[qinvokable]
        fn toggle_hidden_files(self: Pin<&mut FolderViewController>);
        #[qinvokable]
        fn sort_by(self: Pin<&mut FolderViewController>, column: SortColumn);
    }

    impl cxx_qt::Threading for FolderViewController {}
    impl cxx_qt::Initialize for FolderViewController {}
}

use qobject::{EntryListModel, ServicesHolder, SortColumn};

const SCHEME_SEPARATOR: &str = "://";
const NOT_READY_MESSAGE: &str = "The application is still starting.";
const INVALID_LOCATION_MESSAGE: &str = "That isn't a valid location.";
const ROOT_TITLE: &str = "/";

/// Whether a navigation creates a history entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HistoryMode {
    Record,
    Skip,
}

// The bools are Q_PROPERTY storage, which cxx-qt requires as plain fields.
#[allow(clippy::struct_excessive_bools, reason = "mirrors Qt properties one-to-one")]
pub struct FolderViewControllerRust {
    services: *mut ServicesHolder,
    model: *mut EntryListModel,
    location: QString,
    title: QString,
    error_message: QString,
    loading: bool,
    can_go_back: bool,
    can_go_forward: bool,
    can_go_up: bool,
    show_hidden: bool,
    item_count: i32,
    sort_column: i32,
    sort_ascending: bool,

    owned_model: cxx::UniquePtr<EntryListModel>,
    history: History,
    session: Option<FolderSession>,
    generation: u64,
    sort: SortSpec,
}

impl Default for FolderViewControllerRust {
    fn default() -> Self {
        Self {
            services: std::ptr::null_mut(),
            model: std::ptr::null_mut(),
            location: QString::default(),
            title: QString::default(),
            error_message: QString::default(),
            loading: false,
            can_go_back: false,
            can_go_forward: false,
            can_go_up: false,
            show_hidden: false,
            item_count: 0,
            sort_column: SortColumn::Name.repr,
            sort_ascending: true,
            owned_model: cxx::UniquePtr::null(),
            history: History::default(),
            session: None,
            generation: 0,
            sort: SortSpec::default(),
        }
    }
}

impl cxx_qt::Initialize for qobject::FolderViewController {
    fn initialize(mut self: Pin<&mut Self>) {
        let owned_model = qobject::new_entry_list_model();
        let model = owned_model.as_mut_ptr();
        self.as_mut().rust_mut().owned_model = owned_model;
        self.set_model(model);
    }
}

impl qobject::FolderViewController {
    pub fn open_home(self: Pin<&mut Self>) {
        match self.app_services() {
            Some(services) => self.navigate(services.home, HistoryMode::Record),
            None => self.set_error_message(QString::from(NOT_READY_MESSAGE)),
        }
    }

    pub fn open_location(self: Pin<&mut Self>, location: &QString) {
        let Some(services) = self.app_services() else {
            return self.set_error_message(QString::from(NOT_READY_MESSAGE));
        };
        match parse_location(&location.to_string(), &services.home) {
            Some(path) => self.navigate(path, HistoryMode::Record),
            None => self.set_error_message(QString::from(INVALID_LOCATION_MESSAGE)),
        }
    }

    pub fn activate(self: Pin<&mut Self>, row: i32) {
        let target = self.owned_model.as_ref().and_then(|model| {
            let entry = model.rows().get(usize::try_from(row).ok()?)?;
            entry.is_container.then(|| StoragePath::parse(&entry.location).ok()).flatten()
        });
        // Opening files with their default application is milestone 2 (needs a Launcher port).
        if let Some(target) = target {
            self.navigate(target, HistoryMode::Record);
        }
    }

    pub fn go_up(self: Pin<&mut Self>) {
        if let Some(parent) = self.history.current().and_then(StoragePath::parent) {
            self.navigate(parent, HistoryMode::Record);
        }
    }

    pub fn go_back(mut self: Pin<&mut Self>) {
        if let Some(target) = self.as_mut().rust_mut().history.go_back().cloned() {
            self.navigate(target, HistoryMode::Skip);
        }
    }

    pub fn go_forward(mut self: Pin<&mut Self>) {
        if let Some(target) = self.as_mut().rust_mut().history.go_forward().cloned() {
            self.navigate(target, HistoryMode::Skip);
        }
    }

    pub fn reload(self: Pin<&mut Self>) {
        if let Some(session) = &self.session {
            session.reload();
        }
    }

    pub fn toggle_hidden_files(mut self: Pin<&mut Self>) {
        let show = !*self.show_hidden();
        self.as_mut().set_show_hidden(show);
        if let Some(current) = self.history.current().cloned() {
            self.navigate(current, HistoryMode::Skip);
        }
    }

    pub fn sort_by(mut self: Pin<&mut Self>, column: SortColumn) {
        let key = sort_key(column);
        let current = self.sort;
        let order = match (current.key == key, current.order) {
            (true, SortOrder::Ascending) => SortOrder::Descending,
            _ => SortOrder::Ascending,
        };
        let sort = SortSpec { key, order, ..current };
        self.as_mut().rust_mut().sort = sort;
        self.as_mut().set_sort_column(column.repr);
        self.as_mut().set_sort_ascending(order == SortOrder::Ascending);
        if let Some(session) = &self.session {
            session.sort(sort);
        }
    }

    fn app_services(&self) -> Option<AppServices> {
        // SAFETY: the holder is created by the bootstrap before the QML engine
        // and destroyed after it, so it outlives every controller; QML only
        // ever assigns that one object.
        let holder = unsafe { self.services().as_ref() }?;
        Some(holder.services().clone())
    }

    fn hidden_policy(&self) -> HiddenFiles {
        if *self.show_hidden() { HiddenFiles::Show } else { HiddenFiles::Hide }
    }

    fn navigate(mut self: Pin<&mut Self>, target: StoragePath, mode: HistoryMode) {
        let Some(services) = self.app_services() else {
            return self.set_error_message(QString::from(NOT_READY_MESSAGE));
        };
        if mode == HistoryMode::Record {
            self.as_mut().rust_mut().history.visit(target.clone());
        }
        self.as_mut().publish_location(&target, &services.home);
        self.start_session(&services, target);
    }

    fn start_session(mut self: Pin<&mut Self>, services: &AppServices, dir: StoragePath) {
        let generation = self.generation.wrapping_add(1);
        let config = SessionConfig { dir, hidden: self.hidden_policy(), sort: self.sort };
        let qt_thread = self.qt_thread();
        let sink: EventSink = Box::new(move |event| {
            // Fails only if the controller is already destroyed; the event is moot then.
            let _ = qt_thread.queue(move |controller| controller.on_session_event(generation, event));
        });
        let mut rust = self.as_mut().rust_mut();
        rust.generation = generation;
        // Replacing the session drops (and thereby cancels) the previous one.
        rust.session = Some(FolderSession::start(services, config, sink));
        self.as_mut().set_error_message(QString::default());
        // Never show (or let the user activate) rows of the folder being left.
        self.apply_model_updates(vec![ModelUpdate::Reset(Vec::new())]);
    }

    fn publish_location(mut self: Pin<&mut Self>, location: &StoragePath, home: &StoragePath) {
        let title = location.name().map_or_else(|| ROOT_TITLE.to_owned(), ToString::to_string);
        let can_go_back = self.history.can_go_back();
        let can_go_forward = self.history.can_go_forward();
        self.as_mut().set_location(QString::from(&display_location(location, home)));
        self.as_mut().set_title(QString::from(&title));
        self.as_mut().set_can_go_up(!location.is_root());
        self.as_mut().set_can_go_back(can_go_back);
        self.set_can_go_forward(can_go_forward);
    }

    fn on_session_event(mut self: Pin<&mut Self>, generation: u64, event: SessionEvent) {
        if generation != self.generation {
            return;
        }
        match event {
            SessionEvent::Model(updates) => self.apply_model_updates(updates),
            SessionEvent::Loading(loading) => self.set_loading(loading),
            SessionEvent::Failed(error) => self.as_mut().set_error_message(QString::from(&user_message(&error))),
        }
    }

    fn apply_model_updates(mut self: Pin<&mut Self>, updates: Vec<ModelUpdate>) {
        let count = {
            let mut rust = self.as_mut().rust_mut();
            let Some(mut model) = rust.owned_model.as_mut() else {
                return;
            };
            for update in updates {
                match update {
                    ModelUpdate::Reset(rows) => model.as_mut().replace_all(rows),
                    ModelUpdate::Insert { index, row } => model.as_mut().insert_row_at(index, row),
                    ModelUpdate::Remove { index } => model.as_mut().remove_row_at(index),
                    ModelUpdate::Replace { index, row } => model.as_mut().replace_row_at(index, row),
                }
            }
            i32::try_from(model.rows().len()).unwrap_or(i32::MAX)
        };
        self.set_item_count(count);
    }
}

fn sort_key(column: SortColumn) -> SortKey {
    match column {
        SortColumn::Size => SortKey::Size,
        SortColumn::Modified => SortKey::Modified,
        SortColumn::Kind => SortKey::Kind,
        _ => SortKey::Name,
    }
}

/// Accepts a full location (`s3://bucket/x`) or a bare path, which is
/// resolved on the same mount as `home`.
fn parse_location(text: &str, home: &StoragePath) -> Option<StoragePath> {
    let text = text.trim();
    if text.contains(SCHEME_SEPARATOR) {
        return StoragePath::parse(text).ok();
    }
    let mount = home.mount();
    StoragePath::parse(&format!("{}{SCHEME_SEPARATOR}{}/{text}", mount.scheme(), mount.authority())).ok()
}

/// Bare paths on the home mount, full locations for everything else.
fn display_location(location: &StoragePath, home: &StoragePath) -> String {
    if location.mount() == home.mount() { location.path_string() } else { location.to_string() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bare_paths_resolve_on_the_home_mount() {
        let home = StoragePath::parse("file:///Users/me").unwrap();
        assert_eq!(parse_location(" /tmp ", &home).unwrap().to_string(), "file:///tmp");
        assert_eq!(parse_location("s3://b/k", &home).unwrap().to_string(), "s3://b/k");
        assert!(parse_location("/a/../b", &home).is_none());
    }
}
