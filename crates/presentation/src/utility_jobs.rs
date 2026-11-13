//! The utilities that change files run as `OperationsController` jobs, with
//! the same progress rows, cancellation, refresh and undo as copying.

use std::pin::Pin;

use application::use_cases::{CompressRequest, ConvertRequest, ExtractRequest};
use cxx_qt_lib::{QString, QStringList};
use domain::{
    Access, ArchiveFormat, BitChange, CREATABLE_FORMATS, ConversionRequest, DEFAULT_JPEG_QUALITY, ImageFormat,
    PermissionClass, PermissionEdit, PermissionReach, Resize, StoragePath,
};

use crate::job_text::{UtilityJob, utility_title};
use crate::operations_controller::qobject::{OperationsController, PermissionScope, ResizeMode};
use crate::operations_controller::{NOT_READY_MESSAGE, decode_all, parents};
use crate::organize_json::resolution_from_json;

/// Characters of a permission edit string, per bit.
const SET: char = '+';
const CLEAR: char = '-';
/// Bits in an edit string: owner, group, others × read, write, execute.
const EDIT_LENGTH: usize = 9;

impl OperationsController {
    pub fn archive_formats(&self) -> QStringList {
        CREATABLE_FORMATS.iter().map(|f| QString::from(archive_label(*f))).collect()
    }

    pub fn compress(self: Pin<&mut Self>, locations: &QStringList, format: i32) {
        let items = decode_all(locations);
        let format = usize::try_from(format).ok().and_then(|i| CREATABLE_FORMATS.get(i).copied());
        let (Some(format), Some(destination)) = (format, items.first().and_then(StoragePath::parent)) else { return };
        let Some(services) = self.app_services() else { return self.report(NOT_READY_MESSAGE) };
        let title = utility_title(UtilityJob::Compress, &items);
        let request = CompressRequest { items, format, destination: destination.clone() };
        let use_case = services.utilities.compress;
        self.spawn_job(
            title,
            vec![destination],
            move |ctx| async move { use_case.execute(&request, &ctx).await.result },
        );
    }

    pub fn can_extract(&self, location: &QString) -> bool {
        StoragePath::decode(&location.to_string()).ok().and_then(|p| p.name().and_then(ArchiveFormat::of)).is_some()
    }

    pub fn extract(self: Pin<&mut Self>, location: &QString) {
        let Ok(archive) = StoragePath::decode(&location.to_string()) else { return };
        let Some(destination) = archive.parent() else { return };
        let Some(services) = self.app_services() else { return self.report(NOT_READY_MESSAGE) };
        let title = utility_title(UtilityJob::Extract, std::slice::from_ref(&archive));
        let request = ExtractRequest { archive, destination: destination.clone() };
        let use_case = services.utilities.extract;
        self.spawn_job(
            title,
            vec![destination],
            move |ctx| async move { use_case.execute(&request, &ctx).await.result },
        );
    }

    pub fn image_formats(&self) -> QStringList {
        ImageFormat::ALL.iter().map(|f| QString::from(&f.extension().to_uppercase())).collect()
    }

    pub fn is_image(&self, location: &QString) -> bool {
        StoragePath::decode(&location.to_string()).ok().and_then(|p| p.name().and_then(ImageFormat::of)).is_some()
    }

    pub fn convert_images(
        self: Pin<&mut Self>,
        locations: &QStringList,
        format: i32,
        mode: ResizeMode,
        size: i32,
        quality: i32,
    ) {
        let images = decode_all(locations);
        let Some(format) = usize::try_from(format).ok().and_then(|i| ImageFormat::ALL.get(i).copied()) else { return };
        let Some(services) = self.app_services() else { return self.report(NOT_READY_MESSAGE) };
        let quality = u8::try_from(quality).ok().filter(|q| (1..=100).contains(q)).unwrap_or(DEFAULT_JPEG_QUALITY);
        let conversion = ConversionRequest { format, resize: resize(mode, size), quality };
        let title = utility_title(UtilityJob::ConvertImages, &images);
        let affected = parents(&images);
        let request = ConvertRequest { images, conversion, destination: None };
        let use_case = services.utilities.convert_images;
        self.spawn_job(title, affected, move |ctx| async move { use_case.execute(&request, &ctx).await.result });
    }

    pub fn clean_metadata(self: Pin<&mut Self>, locations: &QStringList) {
        let images = decode_all(locations);
        let Some(services) = self.app_services() else { return self.report(NOT_READY_MESSAGE) };
        let title = utility_title(UtilityJob::CleanMetadata, &images);
        let affected = parents(&images);
        let use_case = services.utilities.clean_metadata;
        self.spawn_job(title, affected, move |ctx| async move { use_case.execute(&images, &ctx).await.result });
    }

    pub fn change_permissions(self: Pin<&mut Self>, locations: &QStringList, edit: &QString, scope: PermissionScope) {
        let items = decode_all(locations);
        let Some(edit) = parse_edit(&edit.to_string()) else { return };
        let Some(services) = self.app_services() else { return self.report(NOT_READY_MESSAGE) };
        let title = utility_title(UtilityJob::ChangePermissions, &items);
        let affected = parents(&items);
        let reach = reach(scope);
        let use_case = services.utilities.change_permissions;
        self.spawn_job(
            title,
            affected,
            move |ctx| async move { use_case.execute(&items, edit, reach, &ctx).await.result },
        );
    }

    pub fn apply_organization(self: Pin<&mut Self>, plan: &QString, folder: &QString) {
        let Some(resolution) = resolution_from_json(&plan.to_string()) else { return };
        let Ok(folder) = StoragePath::decode(&folder.to_string()) else { return };
        let Some(services) = self.app_services() else { return self.report(NOT_READY_MESSAGE) };
        let title = format!("Organizing “{}”", folder.name().map(|n| n.display_name()).unwrap_or_default());
        let mut affected: Vec<StoragePath> = resolution.folders_to_create.clone();
        affected.push(folder);
        let use_case = services.organize.apply;
        self.spawn_job(title, affected, move |ctx| async move { use_case.execute(&resolution, &ctx).await.result });
    }

    pub fn secure_delete(self: Pin<&mut Self>, locations: &QStringList) {
        let items = decode_all(locations);
        let Some(services) = self.app_services() else { return self.report(NOT_READY_MESSAGE) };
        let title = utility_title(UtilityJob::SecureDelete, &items);
        let affected = parents(&items);
        let use_case = services.utilities.secure_delete;
        self.spawn_job(title, affected, move |ctx| async move { use_case.execute(&items, &ctx).await });
    }
}

fn archive_label(format: ArchiveFormat) -> &'static str {
    match format {
        ArchiveFormat::Zip => "ZIP",
        ArchiveFormat::SevenZ => "7-Zip",
        ArchiveFormat::TarGz => "tar.gz",
        ArchiveFormat::TarXz => "tar.xz",
        ArchiveFormat::TarZst => "tar.zst",
        ArchiveFormat::TarBz2 => "tar.bz2",
        ArchiveFormat::Tar => "tar",
        ArchiveFormat::Rar => "RAR",
    }
}

fn resize(mode: ResizeMode, size: i32) -> Resize {
    let size = u32::try_from(size).unwrap_or(1).max(1);
    match mode {
        ResizeMode::ScalePercent => Resize::Percent(size.min(100)),
        ResizeMode::FitWithin => Resize::Fit { width: size, height: size },
        ResizeMode::KeepSize => Resize::Keep,
        // An unknown value from QML: change nothing.
        _ => Resize::Keep,
    }
}

fn reach(scope: PermissionScope) -> PermissionReach {
    match scope {
        PermissionScope::AllInside => PermissionReach::Everything,
        PermissionScope::FilesInside => PermissionReach::AlsoFilesInside,
        PermissionScope::FoldersInside => PermissionReach::AlsoFoldersInside,
        PermissionScope::SelectedOnly => PermissionReach::SelectedItems,
        // An unknown value from QML: the narrowest reach.
        _ => PermissionReach::SelectedItems,
    }
}

/// `"+-=…"` (nine characters) into an edit; anything else is refused.
fn parse_edit(text: &str) -> Option<PermissionEdit> {
    let changes: Vec<BitChange> = text
        .chars()
        .map(|c| match c {
            SET => BitChange::Set,
            CLEAR => BitChange::Clear,
            _ => BitChange::Keep,
        })
        .collect();
    if changes.len() != EDIT_LENGTH {
        return None;
    }
    let bits =
        PermissionClass::ALL.into_iter().flat_map(|class| Access::ALL.into_iter().map(move |access| (class, access)));
    Some(
        bits.zip(changes)
            .fold(PermissionEdit::default(), |edit, ((class, access), change)| edit.with(class, access, change)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use domain::UnixMode;

    #[test]
    fn edit_strings_map_to_bits_in_owner_group_others_order() {
        let edit = parse_edit("==+=-====").unwrap();
        assert_eq!(edit.apply(UnixMode::from_mode(0o650)), UnixMode::from_mode(0o750));
        assert!(parse_edit("+++").is_none());
    }

    #[test]
    fn resize_modes_are_bounded() {
        assert_eq!(resize(ResizeMode::ScalePercent, 250), Resize::Percent(100));
        assert_eq!(resize(ResizeMode::FitWithin, 0), Resize::Fit { width: 1, height: 1 });
        assert_eq!(resize(ResizeMode::KeepSize, 50), Resize::Keep);
    }
}
