//! `PreviewController`: feeds the preview pane of one folder view and opens
//! Quick Look on macOS. Selection changes are debounced so scrolling through
//! a folder with the arrow keys never queues dozens of renders.

use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use application::ports::{ArchiveListing, LocalFile, PreviewTheme};
use application::use_cases::{Preview, PreviewBody};
use cxx_qt::Threading;
use cxx_qt_lib::{QList, QString, QStringList};
use domain::{FileCategory, PathSegment, StoragePath};

use crate::error_messages::user_message;
use crate::file_urls::{native_file_url, to_file_url};
use crate::quick_look_bridge::{refresh_quick_look, toggle_quick_look};
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

    #[qenum(PreviewController)]
    enum PreviewKind {
        Nothing,
        Image,
        Pdf,
        Video,
        Audio,
        Text,
        Archive,
        Folder,
        Summary,
    }

    #[qenum(PreviewController)]
    enum Appearance {
        Light,
        Dark,
    }

    #[auto_cxx_name]
    unsafe extern "RustQt" {
        #[qobject]
        #[qml_element]
        #[qproperty(*mut ServicesHolder, services)]
        #[qproperty(i32, kind)]
        #[qproperty(QString, title)]
        #[qproperty(QString, subtitle)]
        #[qproperty(f64, modified_ms)]
        #[qproperty(bool, has_modified)]
        #[qproperty(QString, source_url)]
        #[qproperty(QString, html)]
        #[qproperty(QString, note)]
        #[qproperty(QStringList, archive_lines)]
        type PreviewController = super::PreviewControllerRust;
    }

    #[auto_cxx_name]
    extern "RustQt" {
        #[qinvokable]
        fn show(self: Pin<&mut PreviewController>, location: &QString, appearance: Appearance);
        #[qinvokable]
        fn clear(self: Pin<&mut PreviewController>);
        /// Opens or closes Quick Look; false where Quick Look doesn't exist.
        #[qinvokable]
        fn toggle_quick_look(self: Pin<&mut PreviewController>, locations: &QStringList) -> bool;
        #[qinvokable]
        fn follow_selection(self: Pin<&mut PreviewController>, locations: &QStringList);
    }

    impl cxx_qt::Threading for PreviewController {}
}

use qobject::{Appearance, PreviewKind, ServicesHolder};

/// Wait this long after the selection settles before rendering.
const DEBOUNCE: Duration = Duration::from_millis(120);
const TRUNCATED_TEXT_NOTE: &str = "Showing the beginning of the file.";
const TRUNCATED_ARCHIVE_NOTE: &str = "Showing the first items only.";

pub struct PreviewControllerRust {
    services: *mut ServicesHolder,
    kind: i32,
    title: QString,
    subtitle: QString,
    modified_ms: f64,
    has_modified: bool,
    source_url: QString,
    html: QString,
    note: QString,
    archive_lines: QStringList,
    /// Shared with in-flight tasks so stale ones can skip their work.
    latest_request: Arc<AtomicU64>,
}

impl Default for PreviewControllerRust {
    fn default() -> Self {
        Self {
            services: std::ptr::null_mut(),
            kind: PreviewKind::Nothing.repr,
            title: QString::default(),
            subtitle: QString::default(),
            modified_ms: 0.0,
            has_modified: false,
            source_url: QString::default(),
            html: QString::default(),
            note: QString::default(),
            archive_lines: QStringList::default(),
            latest_request: Arc::default(),
        }
    }
}

/// Everything the pane shows, computed off the Qt thread.
struct PaneContent {
    kind: PreviewKind,
    title: String,
    subtitle: String,
    modified_ms: Option<f64>,
    source_url: String,
    html: String,
    note: String,
    archive_lines: Vec<String>,
}

impl qobject::PreviewController {
    pub fn show(self: Pin<&mut Self>, location: &QString, appearance: Appearance) {
        let (Some(services), Ok(item)) = (self.app_services(), StoragePath::decode(&location.to_string())) else {
            return self.clear();
        };
        let request = self.latest_request.fetch_add(1, Ordering::SeqCst) + 1;
        let latest = Arc::clone(&self.latest_request);
        let theme = match appearance {
            Appearance::Dark => PreviewTheme::Dark,
            Appearance::Light | Appearance { .. } => PreviewTheme::Light,
        };
        let thread = self.qt_thread();
        let use_case = Arc::clone(&services.preview);
        services.runtime.spawn(async move {
            tokio::time::sleep(DEBOUNCE).await;
            if latest.load(Ordering::SeqCst) != request {
                return;
            }
            let content = match use_case.execute(&item, theme).await {
                Ok(preview) => pane_content(&preview),
                Err(error) => error_content(&item, &user_message(&error)),
            };
            let _ = thread.queue(move |controller| {
                if controller.latest_request.load(Ordering::SeqCst) == request {
                    controller.publish(&content);
                }
            });
        });
    }

    pub fn clear(mut self: Pin<&mut Self>) {
        self.latest_request.fetch_add(1, Ordering::SeqCst);
        self.as_mut().publish(&PaneContent::nothing());
    }

    pub fn toggle_quick_look(self: Pin<&mut Self>, locations: &QStringList) -> bool {
        toggle_quick_look(&self.file_urls(locations))
    }

    pub fn follow_selection(self: Pin<&mut Self>, locations: &QStringList) {
        refresh_quick_look(&self.file_urls(locations));
    }

    fn file_urls(&self, locations: &QStringList) -> Vec<String> {
        let Some(services) = self.app_services() else { return Vec::new() };
        QList::<QString>::from(locations)
            .iter()
            .filter_map(|text| StoragePath::decode(&text.to_string()).ok())
            .filter_map(|path| to_file_url(&path, services.home.mount()))
            .collect()
    }

    fn app_services(&self) -> Option<AppServices> {
        // SAFETY: the holder outlives every QML object (see bootstrap.cpp).
        let holder = unsafe { self.services().as_ref() }?;
        Some(holder.services().clone())
    }

    fn publish(mut self: Pin<&mut Self>, content: &PaneContent) {
        self.as_mut().set_title(QString::from(&content.title));
        self.as_mut().set_subtitle(QString::from(&content.subtitle));
        self.as_mut().set_has_modified(content.modified_ms.is_some());
        self.as_mut().set_modified_ms(content.modified_ms.unwrap_or_default());
        self.as_mut().set_source_url(QString::from(&content.source_url));
        self.as_mut().set_html(QString::from(&content.html));
        self.as_mut().set_note(QString::from(&content.note));
        self.as_mut().set_archive_lines(content.archive_lines.iter().map(QString::from).collect());
        // Last, so QML switches views only once everything is in place.
        self.set_kind(content.kind.repr);
    }
}

impl PaneContent {
    fn nothing() -> Self {
        Self {
            kind: PreviewKind::Nothing,
            title: String::new(),
            subtitle: String::new(),
            modified_ms: None,
            source_url: String::new(),
            html: String::new(),
            note: String::new(),
            archive_lines: Vec::new(),
        }
    }
}

fn error_content(item: &StoragePath, message: &str) -> PaneContent {
    PaneContent {
        kind: PreviewKind::Summary,
        title: item.name().map(PathSegment::display_name).unwrap_or_default().into_owned(),
        note: message.to_owned(),
        ..PaneContent::nothing()
    }
}

#[allow(clippy::cast_precision_loss, reason = "milliseconds since 1970 fit an f64 exactly until year 287,000")]
fn pane_content(preview: &Preview) -> PaneContent {
    let entry = &preview.entry;
    let url = |file: &LocalFile| native_file_url(file.path());
    let mut content = PaneContent {
        title: entry.name().display_name().into_owned(),
        subtitle: subtitle(preview),
        modified_ms: entry.metadata().modified.map(|m| m.unix_millis() as f64),
        ..PaneContent::nothing()
    };
    match &preview.body {
        PreviewBody::Image(file) => (content.kind, content.source_url) = (PreviewKind::Image, url(file)),
        PreviewBody::Pdf(file) => (content.kind, content.source_url) = (PreviewKind::Pdf, url(file)),
        PreviewBody::Video(file) => (content.kind, content.source_url) = (PreviewKind::Video, url(file)),
        PreviewBody::Audio(file) => (content.kind, content.source_url) = (PreviewKind::Audio, url(file)),
        PreviewBody::Text(text) => {
            content.kind = PreviewKind::Text;
            content.html.clone_from(&text.html);
            content.note = if text.truncated { TRUNCATED_TEXT_NOTE.to_owned() } else { String::new() };
        }
        PreviewBody::Archive(listing) => {
            content.kind = PreviewKind::Archive;
            content.archive_lines = archive_lines(listing);
            content.note = if listing.truncated { TRUNCATED_ARCHIVE_NOTE.to_owned() } else { String::new() };
        }
        PreviewBody::Folder { .. } => content.kind = PreviewKind::Folder,
        PreviewBody::Summary { note } => {
            content.kind = PreviewKind::Summary;
            content.note = note.clone().unwrap_or_default();
        }
    }
    content
}

fn subtitle(preview: &Preview) -> String {
    let kind = match (&preview.body, preview.category) {
        (PreviewBody::Text(text), _) => text.language.clone(),
        (_, category) => category_label(category).to_owned(),
    };
    match (&preview.body, preview.entry.metadata().size) {
        (PreviewBody::Folder { items: 1 }, _) => format!("{kind} — 1 item"),
        (PreviewBody::Folder { items }, _) => format!("{kind} — {items} items"),
        (_, Some(size)) => format!("{kind} — {}", format_size(size)),
        (_, None) => kind,
    }
}

fn category_label(category: FileCategory) -> &'static str {
    match category {
        FileCategory::Folder => "Folder",
        FileCategory::Image => "Image",
        FileCategory::Pdf => "PDF document",
        FileCategory::Video => "Movie",
        FileCategory::Audio => "Audio",
        FileCategory::Text => "Text",
        FileCategory::Archive => "Archive",
        FileCategory::Document => "Document",
        FileCategory::Other => "File",
    }
}

fn archive_lines(listing: &ArchiveListing) -> Vec<String> {
    listing
        .items
        .iter()
        .map(|item| match (item.is_folder, item.size) {
            (true, _) => item.path.clone(),
            (false, Some(size)) => format!("{}    {}", item.path, format_size(size)),
            (false, None) => item.path.clone(),
        })
        .collect()
}
