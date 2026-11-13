//! `DuplicatesController`: the Find Duplicates window. Runs the search and
//! hands the groups to QML as JSON; removing goes through the Trash (the
//! window's `OperationsController`), so it can be undone.

use std::pin::Pin;
use std::sync::Arc;

use application::CancellationSource;
use application::use_cases::DuplicateSearch;
use cxx_qt::{CxxQtType, Threading};
use cxx_qt_lib::{QString, QStringList};
use domain::{ByteSize, DuplicateGroup, DuplicateMatch, FileEntry, StoragePath, VfsError};
use serde_json::json;

use crate::job_reporting::ThrottledObserver;
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
        #[qproperty(bool, running)]
        /// Negative while the amount of work is still unknown.
        #[qproperty(f64, progress)]
        #[qproperty(QString, status)]
        /// `[{matching, reclaimable, items: [{location, name, folder, size, modified}]}]`.
        #[qproperty(QString, groups_json)]
        #[qproperty(QString, summary)]
        #[qproperty(QString, error_message)]
        type DuplicatesController = super::DuplicatesControllerRust;
    }

    #[auto_cxx_name]
    extern "RustQt" {
        #[qinvokable]
        fn start(
            self: Pin<&mut DuplicatesController>,
            folders: &QStringList,
            min_size_kib: i32,
            include_hidden: bool,
            similar_images: bool,
        );
        #[qinvokable]
        fn cancel(self: Pin<&mut DuplicatesController>);
        /// Drops removed items from the shown groups (after they went to the Trash).
        #[qinvokable]
        fn forget(self: Pin<&mut DuplicatesController>, locations: &QStringList);
    }

    impl cxx_qt::Threading for DuplicatesController {}
}

use qobject::ServicesHolder;

const BYTES_PER_KIB: u64 = 1024;
const UNKNOWN_PROGRESS: f64 = -1.0;

pub struct DuplicatesControllerRust {
    services: *mut ServicesHolder,
    running: bool,
    progress: f64,
    status: QString,
    groups_json: QString,
    summary: QString,
    error_message: QString,

    groups: Vec<DuplicateGroup>,
    job: Option<CancellationSource>,
}

impl Default for DuplicatesControllerRust {
    fn default() -> Self {
        Self {
            services: std::ptr::null_mut(),
            running: false,
            progress: 0.0,
            status: QString::default(),
            groups_json: QString::default(),
            summary: QString::default(),
            error_message: QString::default(),
            groups: Vec::new(),
            job: None,
        }
    }
}

impl qobject::DuplicatesController {
    pub fn start(
        mut self: Pin<&mut Self>,
        folders: &QStringList,
        min_size_kib: i32,
        include_hidden: bool,
        similar_images: bool,
    ) {
        let Some(services) = self.app_services() else { return };
        let folders: Vec<StoragePath> =
            folders.iter().filter_map(|f| StoragePath::decode(&f.to_string()).ok()).collect();
        if folders.is_empty() {
            return;
        }
        self.as_mut().cancel();
        let search = DuplicateSearch {
            folders,
            // 0 means any size; empty files never count.
            min_size: ByteSize((u64::try_from(min_size_kib).unwrap_or(0) * BYTES_PER_KIB).max(1)),
            include_hidden,
            similar_images,
            top_level_only: false,
        };
        let source = CancellationSource::new();
        let cancel = source.token();
        self.as_mut().rust_mut().job = Some(source);
        self.as_mut().set_running(true);
        self.as_mut().set_progress(UNKNOWN_PROGRESS);
        self.as_mut().set_status(QString::from("Looking at sizes…"));
        self.as_mut().set_error_message(QString::default());
        let thread = self.qt_thread();
        let progress_thread = thread.clone();
        let observer = Arc::new(ThrottledObserver::new(move |detail: String, fraction: Option<f64>| {
            let _ = progress_thread.queue(move |mut controller| {
                controller.as_mut().set_status(QString::from(&detail));
                controller.set_progress(fraction.unwrap_or(UNKNOWN_PROGRESS));
            });
        }));
        let use_case = services.utilities.duplicates.clone();
        services.runtime.spawn(async move {
            let found = use_case.execute(&search, observer, cancel).await;
            let _ = thread.queue(move |mut controller| {
                controller.as_mut().set_running(false);
                controller.as_mut().rust_mut().job = None;
                match found {
                    Ok(groups) => controller.show(groups),
                    Err(VfsError::Cancelled) => controller.set_status(QString::from("Stopped.")),
                    Err(error) => controller.set_error_message(QString::from(&error.to_string())),
                }
            });
        });
    }

    pub fn cancel(mut self: Pin<&mut Self>) {
        if let Some(job) = self.as_mut().rust_mut().job.take() {
            job.cancel();
        }
    }

    pub fn forget(mut self: Pin<&mut Self>, locations: &QStringList) {
        let removed: Vec<StoragePath> =
            locations.iter().filter_map(|l| StoragePath::decode(&l.to_string()).ok()).collect();
        let groups = std::mem::take(&mut self.as_mut().rust_mut().groups);
        let remaining = groups
            .into_iter()
            .map(|mut group| {
                group.items.retain(|item| !removed.contains(item.path()));
                group
            })
            .filter(|group| group.items.len() > 1)
            .collect();
        self.show(remaining);
    }

    fn app_services(&self) -> Option<AppServices> {
        // SAFETY: the holder outlives every QML object (see bootstrap.cpp).
        let holder = unsafe { self.services().as_ref() }?;
        Some(holder.services().clone())
    }

    fn show(mut self: Pin<&mut Self>, groups: Vec<DuplicateGroup>) {
        let reclaimable = ByteSize(groups.iter().map(|g| g.reclaimable().0).sum());
        let summary = match groups.len() {
            0 => "No duplicates found.".to_owned(),
            count => format!("{count} groups; {} can be freed.", format_size(reclaimable)),
        };
        self.as_mut().set_summary(QString::from(&summary));
        self.as_mut().set_status(QString::default());
        self.as_mut().set_progress(1.0);
        self.as_mut().set_groups_json(QString::from(&groups_json(&groups)));
        self.rust_mut().groups = groups;
    }
}

fn item_json(item: &FileEntry) -> serde_json::Value {
    let folder = item.path().parent().map(|p| p.path_string()).unwrap_or_default();
    json!({
        "location": item.path().encode(),
        "name": item.name().display_name(),
        "folder": folder,
        "size": format_size(item.metadata().size.unwrap_or_default()),
        "modified": item.metadata().modified.map(|m| m.unix_millis()),
    })
}

fn groups_json(groups: &[DuplicateGroup]) -> String {
    let groups: Vec<serde_json::Value> = groups
        .iter()
        .map(|group| {
            json!({
                "matching": match group.matching {
                    DuplicateMatch::Identical => "identical",
                    DuplicateMatch::Similar => "similar",
                },
                "reclaimable": format_size(group.reclaimable()),
                "items": group.items.iter().map(item_json).collect::<Vec<_>>(),
            })
        })
        .collect();
    serde_json::Value::Array(groups).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use domain::{EntryKind, EntryMetadata, keeper_first};

    #[test]
    fn groups_serialise_with_the_keeper_first() {
        let file = |path: &str| {
            let path = StoragePath::parse(&format!("file://{path}")).unwrap();
            let metadata = EntryMetadata { size: Some(ByteSize(2048)), ..EntryMetadata::default() };
            FileEntry::new(&path.parent().unwrap(), path.name().unwrap().clone(), EntryKind::File, metadata)
        };
        let group = keeper_first(DuplicateMatch::Identical, vec![file("/b/deep/x"), file("/a/x")]);
        let parsed: serde_json::Value = serde_json::from_str(&groups_json(&[group])).unwrap();
        assert_eq!(parsed[0]["matching"], "identical");
        assert_eq!(parsed[0]["items"][0]["folder"], "/a");
        assert_eq!(parsed[0]["reclaimable"], "2.0 KB");
    }
}
