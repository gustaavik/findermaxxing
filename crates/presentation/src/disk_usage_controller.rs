//! `DiskUsageController`: the Disk Usage window. Measures a folder once,
//! then lets the user walk into its subfolders; each level is drawn as a
//! squarified treemap laid out here, so QML only places rectangles.

use std::pin::Pin;
use std::sync::Arc;

use application::CancellationSource;
use cxx_qt::{CxxQtType, Threading};
use cxx_qt_lib::QString;
use domain::{StoragePath, UsageKind, UsageNode, VfsError};
use serde_json::json;

use crate::row_format::format_size;
use crate::services::AppServices;
use crate::treemap::{Rect, squarify};

#[cxx_qt::bridge]
pub mod qobject {
    unsafe extern "C++" {
        include!("cxx-qt-lib/qstring.h");
        type QString = cxx_qt_lib::QString;

        include!("presentation/cpp/services_holder.h");
        type ServicesHolder = crate::services_bridge::ServicesHolder;
    }

    #[auto_cxx_name]
    unsafe extern "RustQt" {
        #[qobject]
        #[qml_element]
        #[qproperty(*mut ServicesHolder, services)]
        #[qproperty(bool, running)]
        #[qproperty(QString, status)]
        /// The folder being shown and its total.
        #[qproperty(QString, focus_location)]
        #[qproperty(QString, focus_title)]
        #[qproperty(QString, focus_summary)]
        #[qproperty(bool, can_go_up)]
        #[qproperty(QString, error_message)]
        type DiskUsageController = super::DiskUsageControllerRust;
    }

    #[auto_cxx_name]
    extern "RustQt" {
        #[qinvokable]
        fn measure(self: Pin<&mut DiskUsageController>, location: &QString);
        #[qinvokable]
        fn cancel(self: Pin<&mut DiskUsageController>);
        /// The focus folder's items as `[{x, y, w, h, name, size, kind, location, share}]`.
        #[qinvokable]
        fn tiles(self: &DiskUsageController, width: f64, height: f64) -> QString;
        #[qinvokable]
        fn open(self: Pin<&mut DiskUsageController>, location: &QString);
        #[qinvokable]
        fn up(self: Pin<&mut DiskUsageController>);

        /// The tree or the focus changed: lay out again.
        #[qsignal]
        fn changed(self: Pin<&mut DiskUsageController>);
    }

    impl cxx_qt::Threading for DiskUsageController {}
}

use qobject::ServicesHolder;

/// Items drawn per level; the rest are too small to see anyway.
const MAX_TILES: usize = 300;

pub struct DiskUsageControllerRust {
    services: *mut ServicesHolder,
    running: bool,
    status: QString,
    focus_location: QString,
    focus_title: QString,
    focus_summary: QString,
    can_go_up: bool,
    error_message: QString,

    tree: Option<UsageNode>,
    focus: Option<StoragePath>,
    job: Option<CancellationSource>,
}

impl Default for DiskUsageControllerRust {
    fn default() -> Self {
        Self {
            services: std::ptr::null_mut(),
            running: false,
            status: QString::default(),
            focus_location: QString::default(),
            focus_title: QString::default(),
            focus_summary: QString::default(),
            can_go_up: false,
            error_message: QString::default(),
            tree: None,
            focus: None,
            job: None,
        }
    }
}

impl qobject::DiskUsageController {
    pub fn measure(mut self: Pin<&mut Self>, location: &QString) {
        let (Some(services), Ok(folder)) = (self.app_services(), StoragePath::decode(&location.to_string())) else {
            return;
        };
        self.as_mut().cancel();
        let source = CancellationSource::new();
        let cancel = source.token();
        self.as_mut().rust_mut().job = Some(source);
        self.as_mut().set_running(true);
        self.as_mut().set_error_message(QString::default());
        self.as_mut().set_status(QString::from("Measuring…"));
        let thread = self.qt_thread();
        let progress_thread = thread.clone();
        let progress: application::use_cases::UsageProgress = Arc::new(move |files, folder: &StoragePath| {
            let text = format!("{files} files — {}", folder.path_string());
            let _ = progress_thread.queue(move |controller| controller.set_status(QString::from(&text)));
        });
        let use_case = services.utilities.disk_usage.clone();
        services.runtime.spawn(async move {
            let measured = use_case.execute(&folder, progress, cancel).await;
            let _ = thread.queue(move |mut controller| {
                controller.as_mut().set_running(false);
                controller.as_mut().rust_mut().job = None;
                match measured {
                    Ok(tree) => {
                        controller.as_mut().set_status(QString::default());
                        controller.as_mut().rust_mut().focus = Some(tree.path.clone());
                        controller.as_mut().rust_mut().tree = Some(tree);
                        controller.refocus();
                    }
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

    pub fn tiles(&self, width: f64, height: f64) -> QString {
        let Some(node) = self.focus_node() else { return QString::from("[]") };
        QString::from(&tiles_json(node, width, height))
    }

    pub fn open(mut self: Pin<&mut Self>, location: &QString) {
        let Ok(path) = StoragePath::decode(&location.to_string()) else { return };
        let is_folder = self.tree.as_ref().and_then(|t| t.find(&path)).is_some();
        if is_folder {
            self.as_mut().rust_mut().focus = Some(path);
            self.refocus();
        }
    }

    pub fn up(mut self: Pin<&mut Self>) {
        let root = self.tree.as_ref().map(|t| t.path.clone());
        let parent = self.focus.as_ref().and_then(StoragePath::parent);
        if let (Some(root), Some(parent)) = (root, parent)
            && parent.is_within(&root)
        {
            self.as_mut().rust_mut().focus = Some(parent);
            self.refocus();
        }
    }

    fn focus_node(&self) -> Option<&UsageNode> {
        let tree = self.tree.as_ref()?;
        self.focus.as_ref().and_then(|f| tree.find(f)).or(Some(tree))
    }

    fn refocus(mut self: Pin<&mut Self>) {
        let (title, summary, location, can_go_up) = match self.focus_node() {
            Some(node) => (
                node.path.name().map_or_else(|| "/".to_owned(), |n| n.display_name().into_owned()),
                format!("{} in {} files", format_size(node.size), node.files),
                node.path.encode(),
                self.tree.as_ref().is_some_and(|t| t.path != node.path),
            ),
            None => (String::new(), String::new(), String::new(), false),
        };
        self.as_mut().set_focus_title(QString::from(&title));
        self.as_mut().set_focus_summary(QString::from(&summary));
        self.as_mut().set_focus_location(QString::from(&location));
        self.as_mut().set_can_go_up(can_go_up);
        self.changed();
    }

    fn app_services(&self) -> Option<AppServices> {
        // SAFETY: the holder outlives every QML object (see bootstrap.cpp).
        let holder = unsafe { self.services().as_ref() }?;
        Some(holder.services().clone())
    }
}

fn tile_name(node: &UsageNode) -> String {
    match node.kind {
        UsageKind::SmallerFiles => format!("{} smaller files", node.files),
        _ => node.path.name().map_or_else(|| "/".to_owned(), |n| n.display_name().into_owned()),
    }
}

fn tiles_json(node: &UsageNode, width: f64, height: f64) -> String {
    let children: Vec<&UsageNode> = node.children.iter().filter(|c| c.size.0 > 0).take(MAX_TILES).collect();
    #[allow(clippy::cast_precision_loss, reason = "sizes only need to be proportional on screen")]
    let sizes: Vec<f64> = children.iter().map(|c| c.size.0 as f64).collect();
    let rects = squarify(&sizes, Rect { x: 0.0, y: 0.0, width, height });
    #[allow(clippy::cast_precision_loss, reason = "a percentage for display")]
    let total = node.size.0.max(1) as f64;
    let tiles: Vec<serde_json::Value> = children
        .iter()
        .zip(rects)
        .map(|(child, rect)| {
            let kind = match child.kind {
                UsageKind::Folder => "folder",
                UsageKind::File => "file",
                UsageKind::SmallerFiles => "smaller",
                UsageKind::Unreadable => "unreadable",
            };
            #[allow(clippy::cast_precision_loss, reason = "a percentage for display")]
            let share = child.size.0 as f64 / total;
            json!({
                "x": rect.x, "y": rect.y, "w": rect.width, "h": rect.height,
                "name": tile_name(child),
                "size": format_size(child.size),
                "kind": kind,
                "location": child.path.encode(),
                "share": share,
            })
        })
        .collect();
    serde_json::Value::Array(tiles).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use domain::{ByteSize, EntryKind, EntryMetadata, FileEntry, PathSegment};

    #[test]
    fn tiles_cover_the_folders_items_with_names_and_kinds() {
        let p = |s: &str| StoragePath::parse(&format!("file://{s}")).unwrap();
        let file = |parent: &str, name: &str, size: u64| {
            let metadata = EntryMetadata { size: Some(ByteSize(size)), ..EntryMetadata::default() };
            FileEntry::new(&p(parent), PathSegment::new(name).unwrap(), EntryKind::File, metadata)
        };
        let sub = UsageNode::folder(p("/a/Movies"), &[file("/a/Movies", "film.mkv", 300)], Vec::new());
        let node = UsageNode::folder(p("/a"), &[file("/a", "notes.txt", 100)], vec![sub]);
        let tiles: serde_json::Value = serde_json::from_str(&tiles_json(&node, 400.0, 100.0)).unwrap();
        assert_eq!(tiles[0]["name"], "Movies");
        assert_eq!(tiles[0]["kind"], "folder");
        assert!((tiles[0]["w"].as_f64().unwrap() * tiles[0]["h"].as_f64().unwrap() - 30_000.0).abs() < 1e-6);
        assert!((tiles[1]["share"].as_f64().unwrap() - 0.25).abs() < 1e-9);
    }
}
