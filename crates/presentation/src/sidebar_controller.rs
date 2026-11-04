//! `SidebarController`: favourites, standard places, volumes, and every S3
//! account with its buckets: local and remote storage side by side. Also
//! adds and removes accounts and favourites.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;

use application::ports::{BucketInfo, NewS3Account, Secret, Volume};
use application::use_cases::AccountError;
use cxx_qt::{CxxQtType, Threading};
use cxx_qt_lib::{QList, QString, QStringList};
use domain::{AccountId, Addressing, BucketName, PathSegment, S3Account, S3Auth, S3Location, StoragePath};

use crate::rows_bridge::SidebarRow;
use crate::services::AppServices;

#[cxx_qt::bridge]
pub mod qobject {
    unsafe extern "C++" {
        include!("cxx-qt-lib/qstring.h");
        type QString = cxx_qt_lib::QString;

        include!("cxx-qt-lib/qstringlist.h");
        type QStringList = cxx_qt_lib::QStringList;

        include!("presentation/src/rows_bridge.cxx.h");
        type SidebarRow = crate::rows_bridge::SidebarRow;

        include!("presentation/cpp/services_holder.h");
        type ServicesHolder = crate::services_bridge::ServicesHolder;
    }

    unsafe extern "C++Qt" {
        include!("presentation/cpp/sidebar_model.h");
        #[qobject]
        type SidebarModel;

        #[cxx_name = "replaceAll"]
        fn replace_all(self: Pin<&mut SidebarModel>, rows: Vec<SidebarRow>);
    }

    unsafe extern "C++" {
        #[cxx_name = "newSidebarModel"]
        fn new_sidebar_model() -> UniquePtr<SidebarModel>;
    }

    #[qenum(SidebarController)]
    enum RowKind {
        Heading,
        Place,
        Favorite,
        Account,
        Bucket,
    }

    #[qenum(SidebarController)]
    enum AuthMode {
        DefaultChain,
        Profile,
        AccessKey,
    }

    #[qenum(SidebarController)]
    enum UrlStyle {
        VirtualHosted,
        PathStyle,
    }

    #[auto_cxx_name]
    unsafe extern "RustQt" {
        #[qobject]
        #[qml_element]
        #[qproperty(*mut ServicesHolder, services)]
        #[qproperty(*mut SidebarModel, model)]
        #[qproperty(bool, adding_account)]
        #[qproperty(QString, account_error)]
        type SidebarController = super::SidebarControllerRust;
    }

    #[auto_cxx_name]
    extern "RustQt" {
        #[qinvokable]
        fn refresh(self: Pin<&mut SidebarController>);
        #[qinvokable]
        fn add_favorite(self: Pin<&mut SidebarController>, location_id: &QString);
        #[qinvokable]
        fn remove_favorite(self: Pin<&mut SidebarController>, location_id: &QString);
        /// `fields` = name, endpoint, region, identity (profile or key id), secret.
        #[qinvokable]
        fn add_account(self: Pin<&mut SidebarController>, fields: &QStringList, auth: AuthMode, style: UrlStyle);
        #[qinvokable]
        fn remove_account(self: Pin<&mut SidebarController>, account_id: i64);
        /// For keys that may open a bucket but not list buckets.
        #[qinvokable]
        fn add_bucket(self: Pin<&mut SidebarController>, account_id: i64, bucket: &QString);

        #[qsignal]
        fn account_added(self: Pin<&mut SidebarController>);
    }

    impl cxx_qt::Threading for SidebarController {}
    impl cxx_qt::Initialize for SidebarController {}
}

use qobject::{AuthMode, RowKind, ServicesHolder, SidebarModel, UrlStyle};

const FAVORITES_HEADING: &str = "Favorites";
const LOCATIONS_HEADING: &str = "Locations";
const CLOUD_HEADING: &str = "Cloud Storage";
const HOME_LABEL: &str = "Home";
const COMPUTER_LABEL: &str = "Computer";
/// Folders every desktop has below home, in sidebar order.
const STANDARD_FOLDERS: &[&str] = &["Desktop", "Documents", "Downloads"];
const NO_BUCKETS: &str = "No buckets";
const INVALID_BUCKET_MESSAGE: &str = "Bucket names use 3–63 lowercase letters, digits, dots and hyphens.";

pub struct SidebarControllerRust {
    services: *mut ServicesHolder,
    model: *mut SidebarModel,
    adding_account: bool,
    account_error: QString,

    owned_model: cxx::UniquePtr<SidebarModel>,
    /// Saved accounts are connected on the first refresh only.
    connected: bool,
    /// Why an account failed to connect at start-up, shown under its name.
    connect_errors: HashMap<AccountId, String>,
}

impl Default for SidebarControllerRust {
    fn default() -> Self {
        Self {
            services: std::ptr::null_mut(),
            model: std::ptr::null_mut(),
            adding_account: false,
            account_error: QString::default(),
            owned_model: cxx::UniquePtr::null(),
            connected: false,
            connect_errors: HashMap::new(),
        }
    }
}

impl cxx_qt::Initialize for qobject::SidebarController {
    fn initialize(mut self: Pin<&mut Self>) {
        let owned = qobject::new_sidebar_model();
        let model = owned.as_mut_ptr();
        self.as_mut().rust_mut().owned_model = owned;
        self.set_model(model);
    }
}

/// Everything one refresh gathered, turned into rows by [`sidebar_rows`].
#[derive(Clone)]
pub(crate) struct SidebarData {
    pub home: StoragePath,
    pub favorites: Vec<StoragePath>,
    pub volumes: Vec<Volume>,
    pub accounts: Vec<(S3Account, Result<Vec<BucketInfo>, String>)>,
}

impl qobject::SidebarController {
    pub fn refresh(mut self: Pin<&mut Self>) {
        let Some(services) = self.app_services() else { return };
        let connect_first = !self.connected;
        self.as_mut().rust_mut().connected = true;
        let (thread, runtime) = (self.qt_thread(), services.runtime.clone());
        runtime.spawn(async move {
            // Local places first: a slow or keychain-prompting account must
            // never keep Home and Favorites out of the sidebar.
            let mut data = gather_places(&services).await;
            let local = SidebarData { accounts: Vec::new(), ..data.clone() };
            let _ = thread.queue(move |controller| controller.publish(&local));
            let errors = if connect_first { connect_saved(&services).await } else { None };
            data.accounts = gather_accounts(&services).await;
            let _ = thread.queue(move |mut controller| {
                if let Some(errors) = errors {
                    controller.as_mut().rust_mut().connect_errors = errors;
                }
                controller.publish(&data);
            });
        });
    }

    pub fn add_favorite(self: Pin<&mut Self>, location_id: &QString) {
        let (Some(services), Ok(location)) = (self.app_services(), StoragePath::decode(&location_id.to_string()))
        else {
            return;
        };
        let use_case = services.places.add_favorite;
        self.run_then_refresh(async move { use_case.execute(location).await.map_err(|e| e.to_string()) });
    }

    pub fn remove_favorite(self: Pin<&mut Self>, location_id: &QString) {
        let (Some(services), Ok(location)) = (self.app_services(), StoragePath::decode(&location_id.to_string()))
        else {
            return;
        };
        let use_case = services.places.remove_favorite;
        self.run_then_refresh(async move { use_case.execute(location).await.map_err(|e| e.to_string()) });
    }

    pub fn add_account(mut self: Pin<&mut Self>, fields: &QStringList, auth: AuthMode, style: UrlStyle) {
        let Some(services) = self.app_services() else { return };
        let form = AccountForm::from(fields);
        let identity = form.identity.trim().to_owned();
        let (auth, secret) = match auth {
            AuthMode::AccessKey => (S3Auth::AccessKey { access_key_id: identity }, Some(Secret::new(form.secret))),
            AuthMode::Profile => (S3Auth::Profile(identity), None),
            AuthMode::DefaultChain | AuthMode { .. } => (S3Auth::DefaultChain, None),
        };
        let account = NewS3Account {
            name: form.name.trim().to_owned(),
            endpoint: non_empty(&form.endpoint),
            region: non_empty(&form.region),
            auth,
            addressing: match style {
                UrlStyle::PathStyle => Addressing::PathStyle,
                UrlStyle::VirtualHosted | UrlStyle { .. } => Addressing::VirtualHosted,
            },
        };
        self.as_mut().set_adding_account(true);
        self.as_mut().set_account_error(QString::default());
        let (use_case, thread) = (services.accounts.add.clone(), self.qt_thread());
        services.runtime.spawn(async move {
            let added = use_case.execute(account, secret).await;
            let _ = thread.queue(move |mut controller| {
                controller.as_mut().set_adding_account(false);
                match added {
                    Ok(_) => {
                        controller.as_mut().account_added();
                        controller.refresh();
                    }
                    Err(error) => controller.set_account_error(QString::from(&account_error_text(&error))),
                }
            });
        });
    }

    pub fn remove_account(self: Pin<&mut Self>, account_id: i64) {
        let Some(services) = self.app_services() else { return };
        let use_case = services.accounts.remove;
        self.run_then_refresh(async move { use_case.execute(AccountId(account_id)).await.map_err(|e| e.to_string()) });
    }

    pub fn add_bucket(self: Pin<&mut Self>, account_id: i64, bucket: &QString) {
        let Some(services) = self.app_services() else { return };
        let Ok(bucket) = BucketName::new(bucket.to_string().trim()) else {
            return self.set_account_error(QString::from(INVALID_BUCKET_MESSAGE));
        };
        let use_case = services.accounts.add_bucket;
        self.run_then_refresh(async move {
            use_case.execute(AccountId(account_id), bucket).await.map_err(|e| e.to_string())
        });
    }

    fn run_then_refresh(self: Pin<&mut Self>, work: impl Future<Output = Result<(), String>> + Send + 'static) {
        let Some(services) = self.app_services() else { return };
        let thread = self.qt_thread();
        services.runtime.spawn(async move {
            let result = work.await;
            let _ = thread.queue(move |mut controller| {
                if let Err(message) = result {
                    controller.as_mut().set_account_error(QString::from(&message));
                }
                controller.refresh();
            });
        });
    }

    fn app_services(&self) -> Option<AppServices> {
        // SAFETY: the holder outlives every QML object (see bootstrap.cpp).
        let holder = unsafe { self.services().as_ref() }?;
        Some(holder.services().clone())
    }

    fn publish(mut self: Pin<&mut Self>, data: &SidebarData) {
        let mut rows = sidebar_rows(data);
        for row in rows.iter_mut().filter(|r| r.kind == RowKind::Account.repr) {
            if let Some(error) = self.connect_errors.get(&AccountId(row.account_id)) {
                row.detail.clone_from(error);
            }
        }
        if let Some(model) = self.as_mut().rust_mut().owned_model.as_mut() {
            model.replace_all(rows);
        }
    }
}

async fn connect_saved(services: &AppServices) -> Option<HashMap<AccountId, String>> {
    let connected = services.accounts.connect_saved.execute().await.ok()?;
    Some(
        connected
            .into_iter()
            .filter_map(|c| c.status.err().map(|error| (c.account.id, account_error_text(&error))))
            .collect(),
    )
}

async fn gather_places(services: &AppServices) -> SidebarData {
    let favorites = services.places.favorites.execute().await.unwrap_or_default();
    let volumes = services.places.volumes.execute().await;
    SidebarData { home: services.home.clone(), favorites, volumes, accounts: Vec::new() }
}

async fn gather_accounts(services: &AppServices) -> Vec<(S3Account, Result<Vec<BucketInfo>, String>)> {
    let mut accounts = Vec::new();
    for account in services.accounts.list.execute().await.unwrap_or_default() {
        let buckets = services.accounts.buckets.execute(account.id).await.map_err(|e| e.to_string());
        accounts.push((account, buckets));
    }
    accounts
}

/// The Add Account form, in the order QML sends its fields.
struct AccountForm {
    name: String,
    endpoint: String,
    region: String,
    identity: String,
    secret: String,
}

impl From<&QStringList> for AccountForm {
    fn from(fields: &QStringList) -> Self {
        let mut values = QList::<QString>::from(fields).iter().map(ToString::to_string).collect::<Vec<_>>().into_iter();
        let mut next = || values.next().unwrap_or_default();
        Self { name: next(), endpoint: next(), region: next(), identity: next(), secret: next() }
    }
}

fn non_empty(text: &str) -> Option<String> {
    let trimmed = text.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_owned())
}

fn account_error_text(error: &AccountError) -> String {
    match error {
        AccountError::MissingSecret => "Enter the secret access key.".to_owned(),
        AccountError::Keychain(error) => format!("The keychain couldn't be used: {error}"),
        AccountError::Store(error) => format!("The account couldn't be saved: {error}"),
        AccountError::Connection(error) => format!("Couldn't connect: {error}"),
    }
}

fn row(kind: RowKind, label: &str, location: Option<&StoragePath>, icon_name: &str) -> SidebarRow {
    SidebarRow {
        kind: kind.repr,
        label: label.to_owned(),
        detail: String::new(),
        location: location.map(StoragePath::encode).unwrap_or_default(),
        icon_name: icon_name.to_owned(),
        account_id: 0,
    }
}

fn place_label(location: &StoragePath) -> String {
    location.name().map_or_else(|| COMPUTER_LABEL.to_owned(), |n| n.display_name().into_owned())
}

pub(crate) fn sidebar_rows(data: &SidebarData) -> Vec<SidebarRow> {
    let mut rows = Vec::new();
    if !data.favorites.is_empty() {
        rows.push(row(RowKind::Heading, FAVORITES_HEADING, None, ""));
        rows.extend(data.favorites.iter().map(|f| row(RowKind::Favorite, &place_label(f), Some(f), "folder")));
    }
    rows.push(row(RowKind::Heading, LOCATIONS_HEADING, None, ""));
    rows.push(row(RowKind::Place, HOME_LABEL, Some(&data.home), "home"));
    for folder in STANDARD_FOLDERS {
        if let Ok(segment) = PathSegment::new(folder) {
            rows.push(row(RowKind::Place, folder, Some(&data.home.join(segment)), "folder"));
        }
    }
    rows.push(row(RowKind::Place, COMPUTER_LABEL, Some(&StoragePath::root(data.home.mount().clone())), "drive"));
    rows.extend(data.volumes.iter().map(|v| row(RowKind::Place, &v.name, Some(&v.location), "drive")));
    if !data.accounts.is_empty() {
        rows.push(row(RowKind::Heading, CLOUD_HEADING, None, ""));
    }
    for (account, buckets) in &data.accounts {
        rows.extend(account_rows(account, buckets));
    }
    rows
}

fn account_rows(account: &S3Account, buckets: &Result<Vec<BucketInfo>, String>) -> Vec<SidebarRow> {
    let mut header = row(RowKind::Account, &account.name, None, "cloud");
    header.account_id = account.id.0;
    let mut rows = vec![header];
    match buckets {
        Ok(buckets) if buckets.is_empty() => rows[0].detail = NO_BUCKETS.to_owned(),
        Ok(buckets) => {
            for bucket in buckets {
                let location = S3Location { account: account.id, bucket: bucket.name.clone() };
                let Ok(mount) = location.mount() else { continue };
                let mut line = row(RowKind::Bucket, bucket.name.as_str(), Some(&StoragePath::root(mount)), "bucket");
                line.account_id = account.id.0;
                rows.push(line);
            }
        }
        Err(message) => rows[0].detail.clone_from(message),
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    fn account(id: i64, name: &str) -> S3Account {
        S3Account {
            id: AccountId(id),
            name: name.into(),
            endpoint: None,
            region: None,
            auth: S3Auth::DefaultChain,
            addressing: Addressing::VirtualHosted,
            skip_cost_warnings: false,
            extra_buckets: Vec::new(),
        }
    }

    #[test]
    fn local_and_cloud_storage_share_the_sidebar() {
        let home = StoragePath::parse("file:///Users/me").unwrap();
        let data = SidebarData {
            favorites: vec![home.join(PathSegment::new("Projects").unwrap())],
            home,
            volumes: vec![Volume { name: "USB".into(), location: StoragePath::parse("file:///Volumes/USB").unwrap() }],
            accounts: vec![
                (account(1, "AWS"), Ok(vec![BucketInfo { name: BucketName::new("photos").unwrap(), region: None }])),
                (account(2, "Offline"), Err("timed out".into())),
            ],
        };
        let rows = sidebar_rows(&data);
        let labels: Vec<(&str, &str)> = rows.iter().map(|r| (r.label.as_str(), r.detail.as_str())).collect();
        assert_eq!(
            labels,
            [
                ("Favorites", ""),
                ("Projects", ""),
                ("Locations", ""),
                ("Home", ""),
                ("Desktop", ""),
                ("Documents", ""),
                ("Downloads", ""),
                ("Computer", ""),
                ("USB", ""),
                ("Cloud Storage", ""),
                ("AWS", ""),
                ("photos", ""),
                ("Offline", "timed out"),
            ]
        );
        assert_eq!(rows[11].location, "s3://1@photos/");
    }
}
