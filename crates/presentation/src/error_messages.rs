use application::operations::OperationError;
use domain::{PathSegment, StoragePath, VfsError};

/// Maps domain errors to wording suitable for the window, never raw OS text
/// as the headline.
pub(crate) fn user_message(error: &VfsError) -> String {
    match error {
        VfsError::NotFound => "This folder no longer exists.".to_owned(),
        VfsError::PermissionDenied => "You don't have permission to see the contents of this folder.".to_owned(),
        VfsError::NotADirectory => "This item is not a folder.".to_owned(),
        VfsError::Unsupported(capability) => format!("This location doesn't support {capability:?}."),
        VfsError::UnknownScheme(scheme) => format!("No storage provider is available for \u{201c}{scheme}\u{201d}."),
        VfsError::InvalidPath(_) => "That isn't a valid location.".to_owned(),
        VfsError::Transient { .. } => "The location is temporarily unavailable. Try again.".to_owned(),
        VfsError::Io { message } => format!("The folder couldn't be read ({message})."),
        VfsError::AlreadyExists
        | VfsError::IsADirectory
        | VfsError::Conflict
        | VfsError::CrossDevice
        | VfsError::NotInTrash
        | VfsError::Cancelled => format!("The folder couldn't be opened ({error})."),
    }
}

/// Message for a failed file operation, or `None` when the user ended it on purpose.
pub(crate) fn operation_message(error: &OperationError) -> Option<String> {
    let text = match error {
        OperationError::Cancelled | OperationError::Stopped => return None,
        OperationError::Failed { error, item } => format!("{} {}", quoted(item), item_problem(error)),
        OperationError::IntoItself { item } => format!("{} can't be moved or copied into itself.", quoted(item)),
        OperationError::CrossMount => {
            "Copying between different storage locations will be available in a later version.".to_owned()
        }
        OperationError::NothingToUndo => "There is nothing to undo.".to_owned(),
        OperationError::NothingToRedo => "There is nothing to redo.".to_owned(),
    };
    Some(text)
}

/// Message for an item that could not be opened.
pub(crate) fn open_message(item: &StoragePath, error: &VfsError) -> String {
    format!("{} couldn't be opened: {}", quoted(item), item_problem(error))
}

fn item_problem(error: &VfsError) -> String {
    match error {
        VfsError::NotFound => "no longer exists.".to_owned(),
        VfsError::PermissionDenied => "can't be changed with your permissions.".to_owned(),
        VfsError::AlreadyExists => "can't be put there because an item with that name already exists.".to_owned(),
        VfsError::NotInTrash => "is no longer in the Trash.".to_owned(),
        VfsError::Unsupported(capability) => format!("is on a location that doesn't support {capability:?}."),
        VfsError::Transient { .. } => "is temporarily unavailable. Try again.".to_owned(),
        VfsError::Io { message } => format!("couldn't be processed ({message})."),
        other => format!("couldn't be processed ({other})."),
    }
}

fn quoted(path: &StoragePath) -> String {
    let name = path.name().map_or_else(|| "/".into(), PathSegment::display_name);
    format!("\u{201c}{name}\u{201d}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn common_errors_have_friendly_text() {
        assert!(user_message(&VfsError::PermissionDenied).contains("permission"));
        assert!(user_message(&VfsError::Io { message: "disk on fire".into() }).contains("disk on fire"));
    }

    #[test]
    fn deliberate_endings_are_silent() {
        assert_eq!(operation_message(&OperationError::Cancelled), None);
        assert_eq!(operation_message(&OperationError::Stopped), None);
        let item = StoragePath::parse("file:///d/a.txt").unwrap();
        let failed = OperationError::Failed { error: VfsError::AlreadyExists, item };
        assert_eq!(
            operation_message(&failed).unwrap(),
            "“a.txt” can't be put there because an item with that name already exists."
        );
    }
}
