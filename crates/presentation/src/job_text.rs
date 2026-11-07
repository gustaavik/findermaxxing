//! Wording for jobs, progress and the Undo/Redo menu items.
//!
//! English only for now; these become translation keys when the app is
//! localised (QML strings already use qsTr).

use domain::{JobPhase, JobProgress, OperationKind, PathSegment, StoragePath};

use crate::row_format::format_size;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TransferVerb {
    Copying,
    Moving,
}

pub(crate) fn transfer_title(verb: TransferVerb, items: &[StoragePath], destination: &StoragePath) -> String {
    let verb = match verb {
        TransferVerb::Copying => "Copying",
        TransferVerb::Moving => "Moving",
    };
    format!("{verb} {} to {}", describe_items(items), quoted_name(destination))
}

pub(crate) fn trash_title(items: &[StoragePath]) -> String {
    format!("Moving {} to the Trash", describe_items(items))
}

pub(crate) fn delete_title(items: &[StoragePath]) -> String {
    format!("Deleting {}", describe_items(items))
}

/// Titles of the utility jobs (M7a).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum UtilityJob {
    Compress,
    Extract,
    ConvertImages,
    CleanMetadata,
    ChangePermissions,
    SecureDelete,
}

pub(crate) fn utility_title(job: UtilityJob, items: &[StoragePath]) -> String {
    let verb = match job {
        UtilityJob::Compress => "Compressing",
        UtilityJob::Extract => "Extracting",
        UtilityJob::ConvertImages => "Converting",
        UtilityJob::CleanMetadata => "Removing metadata from",
        UtilityJob::ChangePermissions => "Changing permissions of",
        UtilityJob::SecureDelete => "Securely deleting",
    };
    format!("{verb} {}", describe_items(items))
}

pub(crate) fn undo_title(kind: OperationKind) -> String {
    format!("Undoing {}", kind_label(kind))
}

pub(crate) fn redo_title(kind: OperationKind) -> String {
    format!("Redoing {}", kind_label(kind))
}

/// Menu text, e.g. "Undo Copy".
pub(crate) fn undo_menu_text(kind: OperationKind) -> String {
    format!("Undo {}", kind_label(kind))
}

pub(crate) fn redo_menu_text(kind: OperationKind) -> String {
    format!("Redo {}", kind_label(kind))
}

/// One line under a job's title, plus the bar fraction when known.
pub(crate) fn progress_detail(progress: &JobProgress) -> (String, Option<f64>) {
    if progress.phase == JobPhase::Preparing {
        return ("Preparing…".to_owned(), None);
    }
    let amount = if progress.bytes_total.0 > 0 {
        format!("{} of {}", format_size(progress.bytes_done), format_size(progress.bytes_total))
    } else {
        format!("{} of {} items", progress.items_done, progress.items_total)
    };
    let detail = match &progress.current_item {
        Some(name) => format!("{amount} — {}", name.display_name()),
        None => amount,
    };
    (detail, progress.fraction())
}

fn kind_label(kind: OperationKind) -> &'static str {
    match kind {
        OperationKind::Copy => "Copy",
        OperationKind::Move => "Move",
        OperationKind::Rename => "Rename",
        OperationKind::Trash => "Move to Trash",
        OperationKind::CreateFolder => "New Folder",
        OperationKind::Compress => "Compress",
        OperationKind::Extract => "Extract",
        OperationKind::ConvertImages => "Convert Images",
        OperationKind::CleanMetadata => "Remove Metadata",
        OperationKind::ChangePermissions => "Change Permissions",
    }
}

fn describe_items(items: &[StoragePath]) -> String {
    match items {
        [single] => quoted_name(single),
        many => format!("{} items", many.len()),
    }
}

fn quoted_name(path: &StoragePath) -> String {
    let name = path.name().map_or_else(|| "/".into(), PathSegment::display_name);
    format!("\u{201c}{name}\u{201d}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use domain::ByteSize;

    fn p(path: &str) -> StoragePath {
        StoragePath::parse(&format!("file://{path}")).unwrap()
    }

    #[test]
    fn titles_name_single_items_and_count_many() {
        assert_eq!(transfer_title(TransferVerb::Copying, &[p("/a/x.txt")], &p("/b")), "Copying “x.txt” to “b”");
        assert_eq!(trash_title(&[p("/a"), p("/b")]), "Moving 2 items to the Trash");
    }

    #[test]
    fn menu_text_names_the_original_action() {
        assert_eq!(undo_menu_text(OperationKind::Rename), "Undo Rename");
        assert_eq!(redo_menu_text(OperationKind::Copy), "Redo Copy");
    }

    #[test]
    fn progress_detail_prefers_bytes() {
        let mut progress = JobProgress::preparing();
        assert_eq!(progress_detail(&progress), ("Preparing…".to_owned(), None));
        progress.phase = JobPhase::Transferring;
        progress.bytes_total = ByteSize(2_000_000);
        progress.bytes_done = ByteSize(1_000_000);
        progress.current_item = Some(PathSegment::new("big.iso").unwrap());
        assert_eq!(progress_detail(&progress), ("1.0 MB of 2.0 MB — big.iso".to_owned(), Some(0.5)));
    }
}
