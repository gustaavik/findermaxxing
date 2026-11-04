//! `FolderViewController`: the QML-facing object behind one tab.
//!
//! Thin by design: it owns navigation history and the active `FolderSession`,
//! forwards UI intents to them, and applies session events to the C++ list
//! model on the Qt thread. All listing/sorting/watching logic lives elsewhere.

use std::pin::Pin;

use application::ports::NewSavedSearch;
use application::use_cases::HiddenFiles;
use cxx_qt::{CxxQtType, Threading};
use cxx_qt_lib::{QList, QString, QStringList};
use domain::{
    Capability, DriveLocation, S3Location, SavedSearch, SearchQuery, ServerLocation, SortKey, SortOrder, SortSpec,
    StoragePath,
};

use crate::error_messages::user_message;
use crate::history::History;
use crate::search_options;
use crate::services::AppServices;
use crate::session::{EventSink, FolderSession, SessionConfig, SessionEvent, SessionMode};
use crate::view_model::ModelUpdate;

#[cxx_qt::bridge]
pub mod qobject {
    unsafe extern "C++" {
        include!("cxx-qt-lib/qstring.h");
        type QString = cxx_qt_lib::QString;

        include!("cxx-qt-lib/qstringlist.h");
        type QStringList = cxx_qt_lib::QStringList;

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

    /// Values of the "Kind" search filter.
    #[qenum(FolderViewController)]
    enum SearchKind {
        AnyKind,
        Folders,
        Images,
        Pdfs,
        Videos,
        Music,
        Text,
        Archives,
        Documents,
    }

    #[qenum(FolderViewController)]
    enum SearchDate {
        AnyDate,
        PastDay,
        PastWeek,
        PastMonth,
        PastYear,
    }

    #[qenum(FolderViewController)]
    enum SearchSize {
        AnySize,
        Small,
        Medium,
        Large,
        Huge,
    }

    #[auto_cxx_name]
    unsafe extern "RustQt" {
        #[qobject]
        #[qml_element]
        #[qproperty(*mut ServicesHolder, services)]
        #[qproperty(*mut EntryListModel, model)]
        #[qproperty(QString, location)]
        #[qproperty(QString, location_id)]
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
        #[qproperty(bool, searching)]
        /// The location reports changes itself; otherwise views reload after jobs.
        #[qproperty(bool, watched)]
        #[qproperty(bool, can_trash)]
        /// A billed remote location (S3): share links and metadata are available.
        #[qproperty(bool, remote)]
        /// A remote location whose items can be kept available offline.
        #[qproperty(bool, offline_capable)]
        #[qproperty(QString, search_status)]
        #[qproperty(QString, search_text)]
        #[qproperty(i32, search_kind)]
        #[qproperty(i32, search_date)]
        #[qproperty(i32, search_size)]
        #[qproperty(QStringList, saved_search_names)]
        type FolderViewController = super::FolderViewControllerRust;
    }

    #[auto_cxx_name]
    extern "RustQt" {
        #[qinvokable]
        fn open_home(self: Pin<&mut FolderViewController>);
        #[qinvokable]
        fn open_location(self: Pin<&mut FolderViewController>, location: &QString);
        /// Opens a location in its lossless encoded form (sidebar, saved items).
        #[qinvokable]
        fn open_location_id(self: Pin<&mut FolderViewController>, location_id: &QString);
        /// Reloads if this view shows one of `folders` and can't watch it.
        #[qinvokable]
        fn reload_if_showing(self: Pin<&mut FolderViewController>, folders: &QStringList);
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
        /// Searches below the current folder; an empty query returns to browsing.
        #[qinvokable]
        fn search(
            self: Pin<&mut FolderViewController>,
            text: &QString,
            kind: SearchKind,
            date: SearchDate,
            size: SearchSize,
        );
        #[qinvokable]
        fn clear_search(self: Pin<&mut FolderViewController>);
        #[qinvokable]
        fn save_search(self: Pin<&mut FolderViewController>, name: &QString);
        #[qinvokable]
        fn refresh_saved_searches(self: Pin<&mut FolderViewController>);
        #[qinvokable]
        fn run_saved_search(self: Pin<&mut FolderViewController>, index: i32);
        #[qinvokable]
        fn delete_saved_search(self: Pin<&mut FolderViewController>, index: i32);

        /// A file (not a folder) was activated; the window's OperationsController opens it.
        #[qsignal]
        fn open_requested(self: Pin<&mut FolderViewController>, location: QString);
    }

    impl cxx_qt::Threading for FolderViewController {}
    impl cxx_qt::Initialize for FolderViewController {}
}

use qobject::{EntryListModel, SearchDate, SearchKind, SearchSize, ServicesHolder, SortColumn};

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
    location_id: QString,
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
    searching: bool,
    watched: bool,
    can_trash: bool,
    remote: bool,
    offline_capable: bool,
    search_status: QString,
    search_text: QString,
    search_kind: i32,
    search_date: i32,
    search_size: i32,
    saved_search_names: QStringList,

    owned_model: cxx::UniquePtr<EntryListModel>,
    history: History,
    session: Option<FolderSession>,
    generation: u64,
    sort: SortSpec,
    /// The active search, if the view shows search results.
    query: Option<SearchQuery>,
    saved: Vec<SavedSearch>,
}

impl Default for FolderViewControllerRust {
    fn default() -> Self {
        Self {
            services: std::ptr::null_mut(),
            model: std::ptr::null_mut(),
            location: QString::default(),
            location_id: QString::default(),
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
            searching: false,
            watched: false,
            can_trash: false,
            remote: false,
            offline_capable: false,
            search_status: QString::default(),
            search_text: QString::default(),
            search_kind: SearchKind::AnyKind.repr,
            search_date: SearchDate::AnyDate.repr,
            search_size: SearchSize::AnySize.repr,
            saved_search_names: QStringList::default(),
            owned_model: cxx::UniquePtr::null(),
            history: History::default(),
            session: None,
            generation: 0,
            sort: SortSpec::default(),
            query: None,
            saved: Vec::new(),
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

    pub fn open_location_id(self: Pin<&mut Self>, location_id: &QString) {
        match StoragePath::decode(&location_id.to_string()) {
            Ok(path) => self.navigate(path, HistoryMode::Record),
            Err(_) => self.set_error_message(QString::from(INVALID_LOCATION_MESSAGE)),
        }
    }

    pub fn reload_if_showing(self: Pin<&mut Self>, folders: &QStringList) {
        let current = self.location_id().to_string();
        let showing = QList::<QString>::from(folders).iter().any(|f| f.to_string() == current);
        if showing && !*self.watched() {
            self.reload();
        }
    }

    pub fn activate(self: Pin<&mut Self>, row: i32) {
        let activated = self.owned_model.as_ref().and_then(|model| {
            let entry = model.rows().get(usize::try_from(row).ok()?)?;
            Some((entry.is_container, entry.location.clone()))
        });
        match activated {
            Some((true, location)) => {
                if let Ok(target) = StoragePath::decode(&location) {
                    self.navigate(target, HistoryMode::Record);
                }
            }
            Some((false, location)) => self.open_requested(QString::from(&location)),
            None => {}
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
        let query = self.query.clone().map(|query| search_options::with_hidden(query, show));
        self.as_mut().rust_mut().query = query;
        self.restart_session();
    }

    pub fn search(mut self: Pin<&mut Self>, text: &QString, kind: SearchKind, date: SearchDate, size: SearchSize) {
        let options = search_options::SearchOptions { text: text.to_string(), kind, date, size };
        let query = search_options::to_query(&options, *self.show_hidden());
        if query.is_empty() {
            return self.clear_search();
        }
        self.as_mut().set_query(Some(query));
        self.restart_session();
    }

    pub fn clear_search(mut self: Pin<&mut Self>) {
        if self.query.is_some() {
            self.as_mut().set_query(None);
            self.restart_session();
        }
    }

    pub fn save_search(self: Pin<&mut Self>, name: &QString) {
        let (Some(services), Some(query), Some(scope)) =
            (self.app_services(), self.query.clone(), self.history.current().cloned())
        else {
            return;
        };
        let search = NewSavedSearch { name: name.to_string(), scope, query };
        let use_case = services.saved_searches.save;
        let thread = self.qt_thread();
        services.runtime.spawn(async move {
            let saved = use_case.execute(search).await;
            let _ = thread.queue(move |controller| match saved {
                Ok(_) => controller.refresh_saved_searches(),
                Err(error) => controller.set_error_message(QString::from(&error.to_string())),
            });
        });
    }

    pub fn refresh_saved_searches(self: Pin<&mut Self>) {
        let Some(services) = self.app_services() else { return };
        let use_case = services.saved_searches.list;
        let thread = self.qt_thread();
        services.runtime.spawn(async move {
            let listed = use_case.execute().await;
            let _ = thread.queue(move |mut controller| match listed {
                Ok(saved) => {
                    let names: QStringList = saved.iter().map(|s| QString::from(&s.name)).collect();
                    controller.as_mut().rust_mut().saved = saved;
                    controller.set_saved_search_names(names);
                }
                Err(error) => controller.set_error_message(QString::from(&error.to_string())),
            });
        });
    }

    pub fn run_saved_search(mut self: Pin<&mut Self>, index: i32) {
        let Some(saved) = usize::try_from(index).ok().and_then(|i| self.saved.get(i)).cloned() else { return };
        let Some(services) = self.app_services() else { return };
        self.as_mut().rust_mut().history.visit(saved.scope.clone());
        self.as_mut().publish_location(&saved.scope, &services.home);
        self.as_mut().set_query(Some(saved.query));
        self.start_session(&services, saved.scope);
    }

    pub fn delete_saved_search(self: Pin<&mut Self>, index: i32) {
        let Some(saved) = usize::try_from(index).ok().and_then(|i| self.saved.get(i)).cloned() else { return };
        let Some(services) = self.app_services() else { return };
        let use_case = services.saved_searches.delete;
        let thread = self.qt_thread();
        services.runtime.spawn(async move {
            let deleted = use_case.execute(saved.id).await;
            let _ = thread.queue(move |controller| match deleted {
                Ok(()) => controller.refresh_saved_searches(),
                Err(error) => controller.set_error_message(QString::from(&error.to_string())),
            });
        });
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
        self.as_mut().set_query(None);
        self.as_mut().publish_location(&target, &services.home);
        self.start_session(&services, target);
    }

    /// Records the active search (or browsing) and mirrors it into the properties.
    fn set_query(mut self: Pin<&mut Self>, query: Option<SearchQuery>) {
        let options = query.as_ref().map(search_options::from_query).unwrap_or_default();
        self.as_mut().set_searching(query.is_some());
        self.as_mut().set_search_text(QString::from(&options.text));
        self.as_mut().set_search_kind(options.kind.repr);
        self.as_mut().set_search_date(options.date.repr);
        self.as_mut().set_search_size(options.size.repr);
        self.as_mut().set_search_status(QString::default());
        self.rust_mut().query = query;
    }

    fn restart_session(self: Pin<&mut Self>) {
        let Some(services) = self.app_services() else { return };
        if let Some(current) = self.history.current().cloned() {
            self.start_session(&services, current);
        }
    }

    fn start_session(mut self: Pin<&mut Self>, services: &AppServices, dir: StoragePath) {
        let generation = self.generation.wrapping_add(1);
        let mode = self.query.clone().map_or(SessionMode::Browse, SessionMode::Search);
        let config = SessionConfig { dir, hidden: self.hidden_policy(), sort: self.sort, mode };
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
        let title = location_title(location);
        if let Some(services) = self.app_services() {
            let capabilities = services.mounts.resolve(location.mount()).map(|m| m.capabilities()).unwrap_or_default();
            self.as_mut().set_watched(capabilities.contains(Capability::Watch));
            self.as_mut().set_can_trash(capabilities.contains(Capability::Trash));
            self.as_mut().set_remote(capabilities.contains(Capability::Metered));
            let is_remote = location.mount() != home.mount();
            self.as_mut().set_offline_capable(is_remote && capabilities.contains(Capability::LocalAccess));
        }
        let can_go_back = self.history.can_go_back();
        let can_go_forward = self.history.can_go_forward();
        self.as_mut().set_location(QString::from(&display_location(location, home)));
        self.as_mut().set_location_id(QString::from(&location.encode()));
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
            SessionEvent::SearchProgress { folders_searched, unreadable_folders } => {
                let status = search_options::status_text(folders_searched, unreadable_folders);
                self.set_search_status(QString::from(&status));
            }
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

/// The folder's name; a bucket root is named after its bucket.
fn location_title(location: &StoragePath) -> String {
    let mount = location.mount();
    if let Some(name) = location.name() {
        return name.to_string();
    }
    if let Some(bucket) = S3Location::of(mount) {
        return bucket.bucket.to_string();
    }
    if let Some(server) = ServerLocation::of(mount) {
        return server.protocol.to_string();
    }
    DriveLocation::of(mount).map_or_else(|| ROOT_TITLE.to_owned(), |drive| drive.provider.to_string())
}

/// Bare paths on the home mount, `bucket/path` in S3, full locations otherwise.
fn display_location(location: &StoragePath, home: &StoragePath) -> String {
    if let Some(bucket) = S3Location::of(location.mount()) {
        return format!("{}{}", bucket.bucket, location.path_string());
    }
    if location.mount() == home.mount() { location.path_string() } else { location.to_string() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn s3_locations_read_as_bucket_paths() {
        let home = StoragePath::parse("file:///Users/me").unwrap();
        let object = StoragePath::parse("s3://3@photos/2024/beach").unwrap();
        assert_eq!(display_location(&object, &home), "photos/2024/beach");
        assert_eq!(location_title(&StoragePath::parse("s3://3@photos/").unwrap()), "photos");
    }

    #[test]
    fn bare_paths_resolve_on_the_home_mount() {
        let home = StoragePath::parse("file:///Users/me").unwrap();
        assert_eq!(parse_location(" /tmp ", &home).unwrap().to_string(), "file:///tmp");
        assert_eq!(parse_location("s3://b/k", &home).unwrap().to_string(), "s3://b/k");
        assert!(parse_location("/a/../b", &home).is_none());
    }
}
