//! `BulkRenameController`: backs the "Rename N Items" dialog. Loads the
//! items and their siblings once, then recomputes the live preview on every
//! keystroke with the pure domain planner; applying runs the use case.

use std::collections::HashSet;
use std::pin::Pin;
use std::sync::Arc;

use application::CancelToken;
use application::operations::{JobContext, NullObserver};
use application::use_cases::{HiddenFiles, ListRequest};
use cxx_qt::{CxxQtType, Threading};
use cxx_qt_lib::{QList, QString, QStringList};
use domain::{
    AddPosition, BulkRename, CaseChange, CaseSensitivity, FileEntry, Matching, NamePart, PlannedRename, RenameOutcome,
    RenameProblem, StoragePath,
};
use futures::TryStreamExt;

use crate::error_messages::operation_message;
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

    #[qenum(BulkRenameController)]
    enum TextMatch {
        PlainText,
        RegularExpression,
    }

    #[qenum(BulkRenameController)]
    enum LetterCase {
        IgnoreCase,
        MatchCase,
    }

    #[qenum(BulkRenameController)]
    enum Placement {
        BeforeName,
        AfterName,
    }

    #[qenum(BulkRenameController)]
    enum CaseStyle {
        Lowercase,
        Uppercase,
        TitleCase,
    }

    #[qenum(BulkRenameController)]
    enum NameScope {
        NameOnly,
        NameAndExtension,
    }

    #[auto_cxx_name]
    unsafe extern "RustQt" {
        #[qobject]
        #[qml_element]
        #[qproperty(*mut ServicesHolder, services)]
        #[qproperty(QStringList, original_names)]
        #[qproperty(QStringList, new_names)]
        #[qproperty(QStringList, problems)]
        #[qproperty(QString, summary)]
        #[qproperty(QString, rule_error)]
        #[qproperty(bool, can_apply)]
        #[qproperty(bool, busy)]
        type BulkRenameController = super::BulkRenameControllerRust;
    }

    #[auto_cxx_name]
    extern "RustQt" {
        #[qinvokable]
        fn load(self: Pin<&mut BulkRenameController>, locations: &QStringList);
        #[qinvokable]
        fn use_replace(
            self: Pin<&mut BulkRenameController>,
            find: &QString,
            replacement: &QString,
            matching: TextMatch,
            letter_case: LetterCase,
            scope: NameScope,
        );
        #[qinvokable]
        fn use_add(self: Pin<&mut BulkRenameController>, text: &QString, placement: Placement);
        #[qinvokable]
        fn use_format(self: Pin<&mut BulkRenameController>, base: &QString, start: i32, digits: i32);
        #[qinvokable]
        fn use_case_change(self: Pin<&mut BulkRenameController>, style: CaseStyle, scope: NameScope);
        #[qinvokable]
        fn apply(self: Pin<&mut BulkRenameController>);

        /// Renaming finished; `error` is empty on success.
        #[qsignal]
        fn finished(self: Pin<&mut BulkRenameController>, error: QString);
    }

    impl cxx_qt::Threading for BulkRenameController {}
}

use qobject::{CaseStyle, LetterCase, NameScope, Placement, ServicesHolder, TextMatch};

pub struct BulkRenameControllerRust {
    services: *mut ServicesHolder,
    original_names: QStringList,
    new_names: QStringList,
    problems: QStringList,
    summary: QString,
    rule_error: QString,
    can_apply: bool,
    busy: bool,

    items: Vec<FileEntry>,
    /// Every path that exists in the items' folders.
    existing: HashSet<StoragePath>,
    rule: Option<BulkRename>,
    plan: Vec<PlannedRename>,
}

impl Default for BulkRenameControllerRust {
    fn default() -> Self {
        Self {
            services: std::ptr::null_mut(),
            original_names: QStringList::default(),
            new_names: QStringList::default(),
            problems: QStringList::default(),
            summary: QString::default(),
            rule_error: QString::default(),
            can_apply: false,
            busy: false,
            items: Vec::new(),
            existing: HashSet::new(),
            rule: None,
            plan: Vec::new(),
        }
    }
}

impl qobject::BulkRenameController {
    pub fn load(mut self: Pin<&mut Self>, locations: &QStringList) {
        let Some(services) = self.app_services() else { return };
        let wanted: Vec<StoragePath> =
            QList::<QString>::from(locations).iter().filter_map(|l| StoragePath::decode(&l.to_string()).ok()).collect();
        self.as_mut().set_busy(true);
        let thread = self.qt_thread();
        let list = Arc::clone(&services.list_directory);
        services.runtime.spawn(async move {
            let (items, existing) = load_items(&list, &wanted).await;
            let _ = thread.queue(move |mut controller| {
                let mut rust = controller.as_mut().rust_mut();
                rust.items = items;
                rust.existing = existing;
                controller.as_mut().set_busy(false);
                controller.recompute();
            });
        });
    }

    pub fn use_replace(
        self: Pin<&mut Self>,
        find: &QString,
        replacement: &QString,
        matching: TextMatch,
        letter_case: LetterCase,
        scope: NameScope,
    ) {
        let matching = match matching {
            TextMatch::RegularExpression => Matching::Pattern,
            TextMatch::PlainText | TextMatch { .. } => Matching::Text,
        };
        let case = match letter_case {
            LetterCase::MatchCase => CaseSensitivity::Sensitive,
            LetterCase::IgnoreCase | LetterCase { .. } => CaseSensitivity::Insensitive,
        };
        match BulkRename::replace(&find.to_string(), &replacement.to_string(), matching, case, name_part(scope)) {
            Ok(rule) => self.set_rule(Some(rule), ""),
            Err(error) => self.set_rule(None, &error.to_string()),
        }
    }

    pub fn use_add(self: Pin<&mut Self>, text: &QString, placement: Placement) {
        let position = match placement {
            Placement::BeforeName => AddPosition::BeforeName,
            Placement::AfterName | Placement { .. } => AddPosition::AfterName,
        };
        self.set_rule(Some(BulkRename::add(&text.to_string(), position)), "");
    }

    pub fn use_format(self: Pin<&mut Self>, base: &QString, start: i32, digits: i32) {
        let start = u64::try_from(start).unwrap_or_default();
        let digits = usize::try_from(digits).unwrap_or_default();
        self.set_rule(Some(BulkRename::format(&base.to_string(), start, digits)), "");
    }

    pub fn use_case_change(self: Pin<&mut Self>, style: CaseStyle, scope: NameScope) {
        let change = match style {
            CaseStyle::Uppercase => CaseChange::Upper,
            CaseStyle::TitleCase => CaseChange::Title,
            CaseStyle::Lowercase | CaseStyle { .. } => CaseChange::Lower,
        };
        self.set_rule(Some(BulkRename::change_case(change, name_part(scope))), "");
    }

    pub fn apply(mut self: Pin<&mut Self>) {
        let Some(services) = self.app_services() else { return };
        if !*self.can_apply() {
            return;
        }
        self.as_mut().set_busy(true);
        let plan = self.plan.clone();
        let use_case = Arc::clone(&services.operations.bulk_rename);
        let thread = self.qt_thread();
        services.runtime.spawn(async move {
            let ctx = JobContext::unattended(Arc::new(NullObserver), CancelToken::never());
            let report = use_case.execute(&plan, &ctx).await;
            let message = report.result.err().and_then(|error| operation_message(&error)).unwrap_or_default();
            let _ = thread.queue(move |mut controller| {
                controller.as_mut().set_busy(false);
                controller.finished(QString::from(&message));
            });
        });
    }

    fn app_services(&self) -> Option<AppServices> {
        // SAFETY: the holder outlives every QML object (see bootstrap.cpp).
        let holder = unsafe { self.services().as_ref() }?;
        Some(holder.services().clone())
    }

    fn set_rule(mut self: Pin<&mut Self>, rule: Option<BulkRename>, error: &str) {
        self.as_mut().rust_mut().rule = rule;
        self.as_mut().set_rule_error(QString::from(error));
        self.recompute();
    }

    fn recompute(mut self: Pin<&mut Self>) {
        let plan = match &self.rule {
            Some(rule) => rule.plan(&self.items, |path| self.existing.contains(path)),
            None => Vec::new(),
        };
        let originals: QStringList = self.items.iter().map(|e| QString::from(&*e.name().display_name())).collect();
        let new_names: QStringList = plan.iter().map(|p| QString::from(&new_name_text(p))).collect();
        let problems: QStringList = plan.iter().map(|p| QString::from(problem_text(&p.outcome))).collect();
        let renaming = plan.iter().filter(|p| matches!(p.outcome, RenameOutcome::Rename(_))).count();
        let blocked = plan.iter().filter(|p| matches!(p.outcome, RenameOutcome::Problem(_))).count();
        self.as_mut().set_original_names(originals);
        self.as_mut().set_new_names(new_names);
        self.as_mut().set_problems(problems);
        self.as_mut().set_summary(QString::from(&summary_text(renaming, blocked)));
        self.as_mut().set_can_apply(renaming > 0);
        self.rust_mut().plan = plan;
    }
}

async fn load_items(
    list: &application::use_cases::ListDirectory,
    wanted: &[StoragePath],
) -> (Vec<FileEntry>, HashSet<StoragePath>) {
    let parents: HashSet<StoragePath> = wanted.iter().filter_map(StoragePath::parent).collect();
    let mut siblings: Vec<FileEntry> = Vec::new();
    for parent in parents {
        let request = ListRequest { dir: parent, hidden: HiddenFiles::Show };
        if let Ok(batches) = list.execute(&request, CancelToken::never()).try_collect::<Vec<_>>().await {
            siblings.extend(batches.into_iter().flatten());
        }
    }
    let existing: HashSet<StoragePath> = siblings.iter().map(|e| e.path().clone()).collect();
    // Keep the user's order (the order of the selection in the view).
    let items = wanted.iter().filter_map(|path| siblings.iter().find(|e| e.path() == path).cloned()).collect();
    (items, existing)
}

fn name_part(scope: NameScope) -> NamePart {
    match scope {
        NameScope::NameAndExtension => NamePart::WholeName,
        NameScope::NameOnly | NameScope { .. } => NamePart::Stem,
    }
}

fn new_name_text(planned: &PlannedRename) -> String {
    match &planned.outcome {
        RenameOutcome::Rename(name) => name.display_name().into_owned(),
        RenameOutcome::Unchanged | RenameOutcome::Problem(_) => {
            planned.item.name().map(|n| n.display_name().into_owned()).unwrap_or_default()
        }
    }
}

fn problem_text(outcome: &RenameOutcome) -> &'static str {
    match outcome {
        RenameOutcome::Rename(_) | RenameOutcome::Unchanged => "",
        RenameOutcome::Problem(RenameProblem::NotText) => "Name isn't text; left as is",
        RenameOutcome::Problem(RenameProblem::InvalidName) => "Not a valid name",
        RenameOutcome::Problem(RenameProblem::DuplicateName) => "Same new name as another item",
        RenameOutcome::Problem(RenameProblem::NameTaken) => "An item with this name exists",
    }
}

fn summary_text(renaming: usize, blocked: usize) -> String {
    let renamed = match renaming {
        0 => "No items will be renamed".to_owned(),
        1 => "1 item will be renamed".to_owned(),
        n => format!("{n} items will be renamed"),
    };
    match blocked {
        0 => format!("{renamed}."),
        1 => format!("{renamed}; 1 item has a problem and will be skipped."),
        n => format!("{renamed}; {n} items have problems and will be skipped."),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summary_counts_renames_and_problems() {
        assert_eq!(summary_text(3, 0), "3 items will be renamed.");
        assert_eq!(summary_text(1, 2), "1 item will be renamed; 2 items have problems and will be skipped.");
    }
}
