//! `ItemDetailsController`: the "Get Info" dialog for items on providers with
//! metadata (S3): content type and custom metadata (editable), read-only
//! facts, version history, and expiring share links.

use std::collections::BTreeMap;
use std::pin::Pin;
use std::time::Duration;

use application::ports::{ItemProperties, ItemVersion, PropertyChanges};
use cxx_qt::{CxxQtType, Threading};
use cxx_qt_lib::{QString, QStringList};
use domain::{StoragePath, VfsError};

use crate::clipboard_bridge::set_clipboard_text;
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
        #[qproperty(QString, content_type)]
        /// Custom metadata as editable `key: value` lines.
        #[qproperty(QString, metadata_text)]
        #[qproperty(QStringList, facts)]
        #[qproperty(QStringList, versions)]
        #[qproperty(QString, share_link)]
        #[qproperty(bool, busy)]
        #[qproperty(QString, error_message)]
        type ItemDetailsController = super::ItemDetailsControllerRust;
    }

    #[auto_cxx_name]
    extern "RustQt" {
        #[qinvokable]
        fn load(self: Pin<&mut ItemDetailsController>, location: &QString);
        #[qinvokable]
        fn save(self: Pin<&mut ItemDetailsController>, content_type: &QString, metadata_text: &QString);
        #[qinvokable]
        fn make_share_link(self: Pin<&mut ItemDetailsController>, hours: i32);
        #[qinvokable]
        fn copy_share_link(self: Pin<&mut ItemDetailsController>);

        #[qsignal]
        fn saved(self: Pin<&mut ItemDetailsController>);
    }

    impl cxx_qt::Threading for ItemDetailsController {}
}

use qobject::ServicesHolder;

const SECONDS_PER_HOUR: u64 = 3600;
const METADATA_SEPARATOR: char = ':';
const UNSUPPORTED_NOTE: &str = "This location has no editable metadata.";

pub struct ItemDetailsControllerRust {
    services: *mut ServicesHolder,
    content_type: QString,
    metadata_text: QString,
    facts: QStringList,
    versions: QStringList,
    share_link: QString,
    busy: bool,
    error_message: QString,

    item: Option<StoragePath>,
}

impl Default for ItemDetailsControllerRust {
    fn default() -> Self {
        Self {
            services: std::ptr::null_mut(),
            content_type: QString::default(),
            metadata_text: QString::default(),
            facts: QStringList::default(),
            versions: QStringList::default(),
            share_link: QString::default(),
            busy: false,
            error_message: QString::default(),
            item: None,
        }
    }
}

impl qobject::ItemDetailsController {
    pub fn load(mut self: Pin<&mut Self>, location: &QString) {
        let (Some(services), Ok(item)) = (self.app_services(), StoragePath::decode(&location.to_string())) else {
            return;
        };
        self.as_mut().rust_mut().item = Some(item.clone());
        self.as_mut().set_share_link(QString::default());
        self.as_mut().set_error_message(QString::default());
        self.as_mut().set_busy(true);
        let (details, thread) = (services.item_details.clone(), self.qt_thread());
        services.runtime.spawn(async move {
            let properties = details.properties.execute(&item).await;
            // No version history (unversioned bucket, other providers) is not an error.
            let versions = details.versions.execute(&item).await.unwrap_or_default();
            let _ = thread.queue(move |mut controller| {
                controller.as_mut().set_busy(false);
                match properties {
                    Ok(properties) => controller.show(&properties, &versions),
                    Err(VfsError::Unsupported(_)) => controller.set_error_message(QString::from(UNSUPPORTED_NOTE)),
                    Err(error) => controller.set_error_message(QString::from(&error.to_string())),
                }
            });
        });
    }

    pub fn save(mut self: Pin<&mut Self>, content_type: &QString, metadata_text: &QString) {
        let (Some(services), Some(item)) = (self.app_services(), self.item.clone()) else { return };
        let changes = PropertyChanges {
            content_type: Some(content_type.to_string().trim().to_owned()).filter(|t| !t.is_empty()),
            user_metadata: parse_metadata(&metadata_text.to_string()),
        };
        self.as_mut().set_busy(true);
        let (details, thread) = (services.item_details.clone(), self.qt_thread());
        services.runtime.spawn(async move {
            let result = details.update.execute(&item, &changes).await;
            let _ = thread.queue(move |mut controller| {
                controller.as_mut().set_busy(false);
                match result {
                    Ok(()) => controller.saved(),
                    Err(error) => controller.set_error_message(QString::from(&error.to_string())),
                }
            });
        });
    }

    pub fn make_share_link(mut self: Pin<&mut Self>, hours: i32) {
        let (Some(services), Some(item)) = (self.app_services(), self.item.clone()) else { return };
        let valid_for = Duration::from_secs(u64::try_from(hours).unwrap_or(1).max(1) * SECONDS_PER_HOUR);
        self.as_mut().set_busy(true);
        let (details, thread) = (services.item_details.clone(), self.qt_thread());
        services.runtime.spawn(async move {
            let link = details.share_link.execute(&item, valid_for).await;
            let _ = thread.queue(move |mut controller| {
                controller.as_mut().set_busy(false);
                match link {
                    Ok(link) => controller.set_share_link(QString::from(&link)),
                    Err(error) => controller.set_error_message(QString::from(&error.to_string())),
                }
            });
        });
    }

    pub fn copy_share_link(self: Pin<&mut Self>) {
        set_clipboard_text(&self.share_link().to_string());
    }

    fn app_services(&self) -> Option<AppServices> {
        // SAFETY: the holder outlives every QML object (see bootstrap.cpp).
        let holder = unsafe { self.services().as_ref() }?;
        Some(holder.services().clone())
    }

    fn show(mut self: Pin<&mut Self>, properties: &ItemProperties, versions: &[ItemVersion]) {
        self.as_mut().set_content_type(QString::from(properties.content_type.as_deref().unwrap_or_default()));
        self.as_mut().set_metadata_text(QString::from(&format_metadata(&properties.user_metadata)));
        self.as_mut().set_facts(properties.facts.iter().map(|(k, v)| QString::from(&format!("{k}: {v}"))).collect());
        self.set_versions(versions.iter().map(|v| QString::from(&version_line(v))).collect());
    }
}

fn format_metadata(metadata: &BTreeMap<String, String>) -> String {
    metadata.iter().map(|(key, value)| format!("{key}{METADATA_SEPARATOR} {value}")).collect::<Vec<_>>().join("\n")
}

/// `key: value` per line; blank lines and lines without a key are ignored.
fn parse_metadata(text: &str) -> BTreeMap<String, String> {
    text.lines()
        .filter_map(|line| line.split_once(METADATA_SEPARATOR))
        .map(|(key, value)| (key.trim().to_lowercase(), value.trim().to_owned()))
        .filter(|(key, _)| !key.is_empty())
        .collect()
}

fn version_line(version: &ItemVersion) -> String {
    let marker = if version.is_latest { " (current)" } else { "" };
    let what = if version.is_delete_marker {
        "deleted".to_owned()
    } else {
        version.size.map_or_else(String::new, format_size)
    };
    let short_id: String = version.version_id.chars().take(12).collect();
    format!("{short_id}  {what}{marker}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metadata_round_trips_through_text() {
        let text = "Owner: Ada\n\n  project : apollo  \nnot a pair";
        let parsed = parse_metadata(text);
        assert_eq!(parsed.get("owner").map(String::as_str), Some("Ada"));
        assert_eq!(parsed.get("project").map(String::as_str), Some("apollo"));
        assert_eq!(parsed.len(), 2);
        assert_eq!(parse_metadata(&format_metadata(&parsed)), parsed);
    }
}
