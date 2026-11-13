//! Contract cases for `PermissionEditor`: the mode set is the mode listed.

use application::ports::{Lister, PermissionEditor};
use domain::{UnixMode, VfsError};

use crate::{VfsFixture, child};

/// Expands to the permission-editing contract cases.
#[macro_export]
macro_rules! permission_contract_tests {
    ($make_fixture:expr $(, #[$attr:meta])*) => {
        $crate::vfs_contract_tests!(@cases $make_fixture; [$(#[$attr])*];
            set_permissions_changes_the_listed_mode,
            set_permissions_missing_is_not_found,
        );
    };
}

pub async fn set_permissions_changes_the_listed_mode<F>(fixture: &F)
where
    F: VfsFixture,
    F::Vfs: PermissionEditor + Lister,
{
    fixture.create_file("script.sh", b"echo hi").await;
    let file = child(fixture, "script.sh");
    for mode in [0o700, 0o640] {
        fixture.vfs().set_permissions(&file, UnixMode::from_mode(mode)).await.unwrap();
        let entry = fixture.vfs().stat(&file).await.unwrap();
        assert_eq!(entry.metadata().permissions.map(UnixMode::bits), Some(mode));
    }
}

pub async fn set_permissions_missing_is_not_found<F>(fixture: &F)
where
    F: VfsFixture,
    F::Vfs: PermissionEditor,
{
    let missing = child(fixture, "missing");
    let result = fixture.vfs().set_permissions(&missing, UnixMode::from_mode(0o644)).await;
    assert_eq!(result, Err(VfsError::NotFound));
}

/// Expands to the native-tag contract cases.
#[macro_export]
macro_rules! tag_contract_tests {
    ($make_fixture:expr $(, #[$attr:meta])*) => {
        $crate::vfs_contract_tests!(@cases $make_fixture; [$(#[$attr])*];
            tags_round_trip_and_clear,
            tags_of_missing_items_are_not_found,
        );
    };
}

pub async fn tags_round_trip_and_clear<F>(fixture: &F)
where
    F: VfsFixture,
    F::Vfs: application::ports::TagEditor,
{
    use application::ports::TagEditor;
    fixture.create_file("bill.pdf", b"x").await;
    let file = child(fixture, "bill.pdf");
    assert!(fixture.vfs().tags(&file).await.unwrap().is_empty());
    let tags = vec![domain::Tag::new("Invoice").unwrap(), domain::Tag::new("Home").unwrap()];
    fixture.vfs().set_tags(&file, tags.clone()).await.unwrap();
    assert_eq!(fixture.vfs().tags(&file).await.unwrap(), tags);
    fixture.vfs().set_tags(&file, Vec::new()).await.unwrap();
    assert!(fixture.vfs().tags(&file).await.unwrap().is_empty());
}

pub async fn tags_of_missing_items_are_not_found<F>(fixture: &F)
where
    F: VfsFixture,
    F::Vfs: application::ports::TagEditor,
{
    use application::ports::TagEditor;
    let missing = child(fixture, "missing");
    assert_eq!(fixture.vfs().tags(&missing).await, Err(VfsError::NotFound));
    assert_eq!(fixture.vfs().set_tags(&missing, Vec::new()).await, Err(VfsError::NotFound));
}
