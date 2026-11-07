use application::operations::OperationError;
use application::ports::{ArchiveError, ConnectError, ImageError, SignInError};
use application::use_cases::{DriveError, ServerError};
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
        OperationError::Store(problem) => format!("The app's settings couldn't be saved ({problem})."),
        OperationError::Archive(error) => archive_problem(error),
        OperationError::Image(ImageError::Unsupported) => "This image format can't be read.".to_owned(),
        OperationError::Image(error) => format!("The image couldn't be processed ({error})."),
        OperationError::SkippedEntries { count } => format!(
            "{count} item{} in the archive {} not extracted: links, special files and paths that would leave the \
             folder are skipped.",
            if *count == 1 { "" } else { "s" },
            if *count == 1 { "was" } else { "were" }
        ),
        OperationError::WrongKind(item) => format!("{} can't be used for this.", quoted(item)),
    };
    Some(text)
}

fn archive_problem(error: &ArchiveError) -> String {
    match error {
        ArchiveError::Encrypted => {
            "The archive is password-protected; protected archives can't be opened yet.".to_owned()
        }
        ArchiveError::Corrupt(..) => "The archive is damaged or isn't the kind its name says.".to_owned(),
        ArchiveError::CannotCreate(_) => "RAR archives can be extracted, not created.".to_owned(),
        ArchiveError::Io(error) => format!("The archive couldn't be read ({error})."),
    }
}

/// Why a server account couldn't be added or connected.
pub(crate) fn server_message(error: &ServerError) -> String {
    match error {
        ServerError::MissingSecret => "Enter the password.".to_owned(),
        ServerError::Keychain(error) => format!("The keychain couldn't be used: {error}"),
        ServerError::Store(error) => format!("The server couldn't be saved: {error}"),
        ServerError::NoProvider(protocol) => format!("{protocol} servers aren't supported in this build."),
        ServerError::Connect(ConnectError::UnknownHost(_)) => {
            "The server's identity hasn't been confirmed yet. Connect again to check it.".to_owned()
        }
        ServerError::Connect(ConnectError::HostKeyChanged(key)) => format!(
            "The server's identity has changed (now {}). Someone may be intercepting the connection. If the server \
             was reinstalled, remove its old key from ~/.ssh/known_hosts and connect again.",
            key.fingerprint
        ),
        ServerError::Connect(ConnectError::Failed(VfsError::PermissionDenied)) => {
            "The user name or password wasn't accepted.".to_owned()
        }
        ServerError::Connect(ConnectError::Failed(VfsError::NotFound)) => {
            "The server answered, but the folder or share doesn't exist.".to_owned()
        }
        ServerError::Connect(ConnectError::Failed(VfsError::Transient { .. })) => {
            "The server didn't answer. Check the address and that you're online.".to_owned()
        }
        ServerError::Connect(ConnectError::Failed(error)) => format!("Couldn't connect: {error}"),
    }
}

/// Why a cloud drive couldn't be added or connected; `None` when the user
/// cancelled the sign-in on purpose.
pub(crate) fn drive_message(error: &DriveError) -> Option<String> {
    Some(match error {
        DriveError::SignIn(SignInError::Cancelled) => return None,
        DriveError::SignIn(SignInError::NotConfigured(provider)) => format!(
            "{provider} isn't available in this build: it was built without an OAuth client ID (see docs/oauth-setup.md)."
        ),
        DriveError::SignIn(SignInError::Denied(reason)) => format!("Access wasn't granted: {reason}"),
        DriveError::SignIn(SignInError::Failed(VfsError::PermissionDenied)) | DriveError::MissingToken => {
            "The sign-in has expired or was revoked. Remove the account and add it again.".to_owned()
        }
        DriveError::SignIn(SignInError::Failed(VfsError::Transient { .. })) => {
            "The provider didn't answer. Check that you're online.".to_owned()
        }
        DriveError::SignIn(SignInError::Failed(error)) => format!("Signing in didn't work: {error}"),
        DriveError::NoProvider(provider) => format!("{provider} isn't supported in this build."),
        DriveError::Keychain(error) => format!("The keychain couldn't be used: {error}"),
        DriveError::Store(error) => format!("The account couldn't be saved: {error}"),
    })
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
