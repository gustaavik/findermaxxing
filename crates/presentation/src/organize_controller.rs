//! `OrganizeController`: the Organize window. Analyses a folder, keeps which
//! suggestions the user accepted, and shows the before/after preview of
//! exactly those. Applying is a job on the window's `OperationsController`
//! (one undoable step), handed the plan as JSON (`acceptedPlan`).

use std::collections::HashSet;
use std::pin::Pin;
use std::sync::Arc;

use application::CancellationSource;
use application::use_cases::{Analysis, CloudUse};
use cxx_qt::{CxxQtType, Threading};
use cxx_qt_lib::QString;
use domain::{Change, StoragePath, Suggestion, SuggestionId, SuggestionSource, resolve};
use serde_json::json;

use crate::clock_bridge::query_clock;
use crate::job_reporting::ThrottledObserver;
use crate::organize_json::resolution_to_json;
use crate::services::AppServices;

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
        /// Negative while the amount of work is unknown.
        #[qproperty(f64, progress)]
        #[qproperty(QString, status)]
        #[qproperty(QString, folder_title)]
        /// `[{id, section, name, change, reason, source, accepted}]`.
        #[qproperty(QString, suggestions_json)]
        /// `[{before, after, tags}]` for the accepted suggestions; `after` is
        /// empty for the Trash.
        #[qproperty(QString, preview_json)]
        #[qproperty(i32, accepted_count)]
        #[qproperty(QString, notes)]
        #[qproperty(bool, used_cloud)]
        #[qproperty(QString, error_message)]
        type OrganizeController = super::OrganizeControllerRust;
    }

    #[auto_cxx_name]
    extern "RustQt" {
        /// Analyses `location`; `allow_cloud` asks the cloud model if the
        /// folder is opted in.
        #[qinvokable]
        fn analyze(self: Pin<&mut OrganizeController>, location: &QString, allow_cloud: bool);
        #[qinvokable]
        fn cancel(self: Pin<&mut OrganizeController>);
        #[qinvokable]
        fn set_accepted(self: Pin<&mut OrganizeController>, id: i32, accepted: bool);
        /// Every suggestion of a section (`cleanup`, `folders`, `renames`,
        /// `tags`), or all of them for an empty section.
        #[qinvokable]
        fn set_section_accepted(self: Pin<&mut OrganizeController>, section: &QString, accepted: bool);
        /// The accepted suggestions, resolved, for `OperationsController.applyOrganization`.
        #[qinvokable]
        fn accepted_plan(self: &OrganizeController) -> QString;
    }

    impl cxx_qt::Threading for OrganizeController {}
}

use qobject::ServicesHolder;

const UNKNOWN_PROGRESS: f64 = -1.0;

pub struct OrganizeControllerRust {
    services: *mut ServicesHolder,
    running: bool,
    progress: f64,
    status: QString,
    folder_title: QString,
    suggestions_json: QString,
    preview_json: QString,
    accepted_count: i32,
    notes: QString,
    used_cloud: bool,
    error_message: QString,

    analysis: Option<Analysis>,
    accepted: HashSet<SuggestionId>,
    job: Option<CancellationSource>,
}

impl Default for OrganizeControllerRust {
    fn default() -> Self {
        Self {
            services: std::ptr::null_mut(),
            running: false,
            progress: 0.0,
            status: QString::default(),
            folder_title: QString::default(),
            suggestions_json: QString::from("[]"),
            preview_json: QString::from("[]"),
            accepted_count: 0,
            notes: QString::default(),
            used_cloud: false,
            error_message: QString::default(),
            analysis: None,
            accepted: HashSet::new(),
            job: None,
        }
    }
}

impl qobject::OrganizeController {
    pub fn analyze(mut self: Pin<&mut Self>, location: &QString, allow_cloud: bool) {
        let (Some(services), Ok(folder)) = (self.app_services(), StoragePath::decode(&location.to_string())) else {
            return;
        };
        self.as_mut().cancel();
        let title = folder.name().map_or_else(|| "/".to_owned(), |n| n.display_name().into_owned());
        self.as_mut().set_folder_title(QString::from(&title));
        self.as_mut().set_error_message(QString::default());
        self.as_mut().set_notes(QString::default());
        self.as_mut().rust_mut().analysis = None;
        self.as_mut().refresh();
        let source = CancellationSource::new();
        let cancel = source.token();
        self.as_mut().rust_mut().job = Some(source);
        self.as_mut().set_running(true);
        self.as_mut().set_progress(UNKNOWN_PROGRESS);
        self.as_mut().set_status(QString::from("Reading the folder…"));
        let thread = self.qt_thread();
        let progress_thread = thread.clone();
        let observer = Arc::new(ThrottledObserver::new(move |detail: String, fraction: Option<f64>| {
            let _ = progress_thread.queue(move |mut controller| {
                controller.as_mut().set_status(QString::from(&detail));
                controller.set_progress(fraction.unwrap_or(UNKNOWN_PROGRESS));
            });
        }));
        let cloud = if allow_cloud { CloudUse::IfOptedIn } else { CloudUse::Never };
        let use_case = services.organize.analyze.clone();
        services.runtime.spawn(async move {
            let analysis = use_case.execute(&folder, query_clock(), cloud, observer, cancel).await;
            let _ = thread.queue(move |mut controller| {
                controller.as_mut().set_running(false);
                controller.as_mut().rust_mut().job = None;
                match analysis {
                    Ok(analysis) => controller.show(analysis),
                    Err(application::use_cases::OrganizeError::Vfs(domain::VfsError::Cancelled)) => {
                        controller.set_status(QString::from("Stopped."));
                    }
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

    pub fn set_accepted(mut self: Pin<&mut Self>, id: i32, accepted: bool) {
        let Ok(id) = u32::try_from(id) else { return };
        if accepted {
            self.as_mut().rust_mut().accepted.insert(SuggestionId(id));
        } else {
            self.as_mut().rust_mut().accepted.remove(&SuggestionId(id));
        }
        self.refresh();
    }

    pub fn set_section_accepted(mut self: Pin<&mut Self>, section: &QString, accepted: bool) {
        let section = section.to_string();
        let ids: Vec<SuggestionId> = self
            .analysis
            .iter()
            .flat_map(|a| a.plan.suggestions.iter())
            .filter(|s| section.is_empty() || section_of(s) == section)
            .map(|s| s.id)
            .collect();
        let chosen = &mut self.as_mut().rust_mut().accepted;
        for id in ids {
            if accepted {
                chosen.insert(id);
            } else {
                chosen.remove(&id);
            }
        }
        self.refresh();
    }

    pub fn accepted_plan(&self) -> QString {
        let Some(analysis) = &self.analysis else { return QString::default() };
        match resolve(&analysis.plan, &self.accepted, &analysis.existing) {
            Ok(resolution) if !resolution.is_empty() => QString::from(&resolution_to_json(&resolution)),
            _ => QString::default(),
        }
    }

    fn app_services(&self) -> Option<AppServices> {
        // SAFETY: the holder outlives every QML object (see bootstrap.cpp).
        let holder = unsafe { self.services().as_ref() }?;
        Some(holder.services().clone())
    }

    /// New suggestions start out not accepted: nothing changes unless chosen.
    fn show(mut self: Pin<&mut Self>, analysis: Analysis) {
        let summary = match analysis.plan.suggestions.len() {
            0 => "This folder looks tidy: nothing to suggest.".to_owned(),
            1 => "1 suggestion.".to_owned(),
            n => format!("{n} suggestions."),
        };
        self.as_mut().set_status(QString::from(&summary));
        self.as_mut().set_progress(1.0);
        self.as_mut().set_notes(QString::from(&analysis.notes.join("\n")));
        self.as_mut().set_used_cloud(analysis.used_cloud);
        self.as_mut().rust_mut().accepted.clear();
        self.as_mut().rust_mut().analysis = Some(analysis);
        self.refresh();
    }

    fn refresh(mut self: Pin<&mut Self>) {
        let (suggestions, preview) = match &self.analysis {
            Some(analysis) => (suggestions_json(analysis, &self.accepted), preview_json(analysis, &self.accepted)),
            None => ("[]".to_owned(), "[]".to_owned()),
        };
        let count = i32::try_from(self.accepted.len()).unwrap_or(i32::MAX);
        self.as_mut().set_suggestions_json(QString::from(&suggestions));
        self.as_mut().set_preview_json(QString::from(&preview));
        self.set_accepted_count(count);
    }
}

fn section_of(suggestion: &Suggestion) -> &'static str {
    match suggestion.change {
        Change::Trash(_) => "cleanup",
        Change::MoveInto(_) => "folders",
        Change::Rename(_) => "renames",
        Change::AddTags(_) => "tags",
    }
}

fn change_text(change: &Change) -> String {
    match change {
        Change::MoveInto(folder) => format!("→ {}/", folder.display_name()),
        Change::Rename(name) => format!("→ {}", name.display_name()),
        Change::AddTags(tags) => format!("+ {}", tags.iter().map(domain::Tag::as_str).collect::<Vec<_>>().join(", ")),
        Change::Trash(_) => "→ Trash".to_owned(),
    }
}

fn suggestions_json(analysis: &Analysis, accepted: &HashSet<SuggestionId>) -> String {
    let rows: Vec<serde_json::Value> = analysis
        .plan
        .suggestions
        .iter()
        .map(|s| {
            json!({
                "id": s.id.0,
                "section": section_of(s),
                "name": s.item.name().map(|n| n.display_name().into_owned()).unwrap_or_default(),
                "change": change_text(&s.change),
                "reason": s.reason,
                "source": match s.source {
                    SuggestionSource::Rules => "rules",
                    SuggestionSource::LocalModel => "local model",
                    SuggestionSource::CloudModel => "cloud model",
                },
                "accepted": accepted.contains(&s.id),
            })
        })
        .collect();
    serde_json::Value::Array(rows).to_string()
}

/// Paths relative to the analysed folder, as the preview shows them.
fn relative(folder: Option<&StoragePath>, path: &StoragePath) -> String {
    let depth = folder.map_or(0, |f| f.segments().len());
    path.segments().iter().skip(depth).map(|s| s.display_name().into_owned()).collect::<Vec<_>>().join("/")
}

fn preview_json(analysis: &Analysis, accepted: &HashSet<SuggestionId>) -> String {
    let folder = analysis.plan.folder.as_ref();
    let rows: Vec<serde_json::Value> = resolve(&analysis.plan, accepted, &analysis.existing)
        .map(|r| r.preview)
        .unwrap_or_default()
        .iter()
        .map(|row| {
            json!({
                "before": relative(folder, &row.before),
                "after": row.after.as_ref().map(|a| relative(folder, a)).unwrap_or_default(),
                "tags": row.tags.iter().map(domain::Tag::as_str).collect::<Vec<_>>(),
            })
        })
        .collect();
    serde_json::Value::Array(rows).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use domain::{EntryKind, PathSegment, Plan};

    #[test]
    fn previews_show_paths_inside_the_folder_for_accepted_suggestions_only() {
        let p = |s: &str| StoragePath::parse(&format!("file://{s}")).unwrap();
        let mut plan = Plan { folder: Some(p("/d")), suggestions: Vec::new() };
        let folder = PathSegment::new("Bills").unwrap();
        plan.suggest(p("/d/a.pdf"), EntryKind::File, Change::MoveInto(folder), String::new(), SuggestionSource::Rules);
        plan.suggest(
            p("/d/b.pdf"),
            EntryKind::File,
            Change::Rename(PathSegment::new("c.pdf").unwrap()),
            String::new(),
            SuggestionSource::Rules,
        );
        let existing = ["a.pdf", "b.pdf"].iter().map(|n| PathSegment::new(n).unwrap()).collect();
        let analysis = Analysis { plan, existing, notes: Vec::new(), used_cloud: false };
        let accepted: HashSet<_> = [SuggestionId(0)].into_iter().collect();
        let rows: serde_json::Value = serde_json::from_str(&preview_json(&analysis, &accepted)).unwrap();
        assert_eq!(rows.as_array().unwrap().len(), 1);
        assert_eq!(rows[0]["after"], "Bills/a.pdf");
        let suggestions: serde_json::Value = serde_json::from_str(&suggestions_json(&analysis, &accepted)).unwrap();
        assert_eq!(
            (suggestions[0]["accepted"].as_bool(), suggestions[1]["section"].as_str()),
            (Some(true), Some("renames"))
        );
    }
}
