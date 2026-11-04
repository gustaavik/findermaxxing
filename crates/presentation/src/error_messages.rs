use domain::VfsError;

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
        VfsError::AlreadyExists | VfsError::IsADirectory | VfsError::Conflict | VfsError::Cancelled => {
            format!("The folder couldn't be opened ({error}).")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn common_errors_have_friendly_text() {
        assert!(user_message(&VfsError::PermissionDenied).contains("permission"));
        assert!(user_message(&VfsError::Io { message: "disk on fire".into() }).contains("disk on fire"));
    }
}
