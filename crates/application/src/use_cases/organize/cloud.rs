//! The user's own cloud model (ADR 0029): settings, the key in the
//! keychain, per-folder opt-in, the exact preview of what would be sent, and
//! turning the model's answer into checked suggestions.

use std::collections::HashSet;
use std::sync::Arc;

use domain::{Change, FileEntry, PathSegment, Plan, QueryClock, StoragePath, SuggestionSource, Tag};

use super::OrganizeError;
use super::gather::{Inventory, cloud_request, take_inventory};
use crate::cancel::CancelToken;
use crate::operations::NullObserver;
use crate::operations::progress::ProgressTracker;
use crate::ports::{
    CloudModelConfig, CloudModelSettings, CloudModelStore, ContentExtractor, CredentialKey, CredentialStore,
    ModelProposal, MountResolver, OrganizationModel, OrganizeRequest, Secret,
};
use crate::use_cases::utilities::opaque_debug;

/// Tags a cloud model may add to one item.
const MAX_TAGS_PER_ITEM: usize = 5;

#[derive(Clone)]
pub struct CloudParts {
    pub store: Arc<dyn CloudModelStore>,
    pub credentials: Arc<dyn CredentialStore>,
    pub model: Arc<dyn OrganizationModel>,
}

/// Whether `folder` is opted in: it, or a folder it's in, was opted in.
fn opted_in(settings: &CloudModelSettings, folder: &StoragePath) -> bool {
    settings.opted_in.iter().any(|allowed| folder.is_within(allowed))
}

/// Asks the cloud model about `inventory` if the folder is opted in and a key
/// is set; adds its checked suggestions to `plan`. `Ok(false)`: not asked.
pub(crate) async fn ask_cloud(
    cloud: &CloudParts,
    inventory: &Inventory,
    clock: QueryClock,
    plan: &mut Plan,
    can_tag: bool,
) -> Result<bool, OrganizeError> {
    let Some(settings) = cloud.store.load().await? else { return Ok(false) };
    if !opted_in(&settings, &inventory.folder) {
        return Ok(false);
    }
    let Some(key) = cloud.credentials.load(&CredentialKey::cloud_model_key()).await? else { return Ok(false) };
    let (request, items) = cloud_request(inventory, clock);
    if request.items.is_empty() {
        return Ok(false);
    }
    let proposal = cloud.model.propose(&settings.config, &key, &request).await?;
    add_proposal(plan, &proposal, &items, can_tag);
    Ok(true)
}

/// Adds what's valid in `proposal`: known item ids, names that are valid file
/// names, renames that keep the extension, valid tags. The rest is dropped.
pub(crate) fn add_proposal(plan: &mut Plan, proposal: &ModelProposal, items: &[FileEntry], can_tag: bool) {
    let item = |id: u32| items.get(usize::try_from(id).ok()?);
    let existing_files: HashSet<&PathSegment> =
        items.iter().filter(|e| e.kind() != domain::EntryKind::Directory).map(FileEntry::name).collect();
    for group in &proposal.groups {
        let Ok(folder) = PathSegment::new(group.folder.trim()) else { continue };
        if existing_files.contains(&folder) {
            continue;
        }
        for entry in group.item_ids.iter().filter_map(|id| item(*id)) {
            let change = Change::MoveInto(folder.clone());
            if entry.name() != &folder && !plan.has(entry.path(), &change) {
                plan.suggest(
                    entry.path().clone(),
                    entry.kind(),
                    change,
                    group.reason.clone(),
                    SuggestionSource::CloudModel,
                );
            }
        }
    }
    for rename in &proposal.renames {
        let Some(entry) = item(rename.item_id) else { continue };
        let Some(name) = keep_extension(entry, rename.name.trim()) else { continue };
        let change = Change::Rename(name);
        if !plan.has(entry.path(), &change) {
            plan.suggest(
                entry.path().clone(),
                entry.kind(),
                change,
                rename.reason.clone(),
                SuggestionSource::CloudModel,
            );
        }
    }
    if !can_tag {
        return;
    }
    for proposed in &proposal.tags {
        let Some(entry) = item(proposed.item_id) else { continue };
        let tags: Vec<Tag> = proposed.tags.iter().filter_map(|t| Tag::new(t).ok()).take(MAX_TAGS_PER_ITEM).collect();
        let change = Change::AddTags(tags);
        if matches!(&change, Change::AddTags(t) if !t.is_empty()) && !plan.has(entry.path(), &change) {
            plan.suggest(
                entry.path().clone(),
                entry.kind(),
                change,
                "Suggested tags".to_owned(),
                SuggestionSource::CloudModel,
            );
        }
    }
}

/// A suggested name, with the original extension put back if the model
/// dropped or changed it (a rename must never change what a file is).
fn keep_extension(entry: &FileEntry, suggested: &str) -> Option<PathSegment> {
    let suggested = PathSegment::new(suggested).ok()?;
    if entry.kind() == domain::EntryKind::Directory || &suggested == entry.name() {
        return (&suggested != entry.name()).then_some(suggested);
    }
    let original = entry.name().stem_and_extension().1.map(<[u8]>::to_vec);
    let (stem, extension) = suggested.stem_and_extension();
    match original {
        Some(original) if extension.map(<[u8]>::to_ascii_lowercase) != Some(original.to_ascii_lowercase()) => {
            let stem = if extension.is_some() && !stem.is_empty() { stem } else { suggested.as_bytes() };
            let mut name = stem.to_vec();
            name.push(b'.');
            name.extend(original);
            PathSegment::from_bytes(&name).ok()
        }
        _ => Some(suggested),
    }
}

/// The cloud-model settings, for the settings dialog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CloudModelStatus {
    pub settings: Option<CloudModelSettings>,
    pub has_key: bool,
}

pub struct CloudModelSetup {
    store: Arc<dyn CloudModelStore>,
    credentials: Arc<dyn CredentialStore>,
}

impl CloudModelSetup {
    #[must_use]
    pub fn new(store: Arc<dyn CloudModelStore>, credentials: Arc<dyn CredentialStore>) -> Self {
        Self { store, credentials }
    }

    /// # Errors
    /// The store or keychain failed.
    pub async fn status(&self) -> Result<CloudModelStatus, OrganizeError> {
        let settings = self.store.load().await?;
        let has_key = self.credentials.load(&CredentialKey::cloud_model_key()).await?.is_some();
        Ok(CloudModelStatus { settings, has_key })
    }

    /// Saves the model; `key` replaces the stored one when given. Folders
    /// opted in before stay opted in.
    ///
    /// # Errors
    /// The store or keychain failed.
    pub async fn configure(&self, config: CloudModelConfig, key: Option<Secret>) -> Result<(), OrganizeError> {
        if let Some(key) = key {
            self.credentials.store(&CredentialKey::cloud_model_key(), &key).await?;
        }
        let opted_in = self.store.load().await?.map(|s| s.opted_in).unwrap_or_default();
        self.store.save(CloudModelSettings { config, opted_in }).await?;
        Ok(())
    }

    /// Removes the model, its key and every opt-in.
    ///
    /// # Errors
    /// The store or keychain failed.
    pub async fn forget(&self) -> Result<(), OrganizeError> {
        self.credentials.delete(&CredentialKey::cloud_model_key()).await?;
        self.store.clear().await?;
        Ok(())
    }

    /// Opts `folder` in or out.
    ///
    /// # Errors
    /// No model is configured, or the store failed.
    pub async fn set_opt_in(&self, folder: &StoragePath, allowed: bool) -> Result<(), OrganizeError> {
        let mut settings = self.store.load().await?.ok_or(OrganizeError::NoCloudModel)?;
        settings.opted_in.retain(|f| f != folder);
        if allowed {
            settings.opted_in.push(folder.clone());
        }
        self.store.save(settings).await?;
        Ok(())
    }

    /// Whether the cloud model may be asked about `folder`.
    ///
    /// # Errors
    /// The store failed.
    pub async fn is_opted_in(&self, folder: &StoragePath) -> Result<bool, OrganizeError> {
        Ok(self.store.load().await?.is_some_and(|s| opted_in(&s, folder)))
    }
}

/// Exactly what would be sent for a folder, before the user opts it in.
pub struct PreviewCloudRequest {
    mounts: Arc<dyn MountResolver>,
    extractor: Arc<dyn ContentExtractor>,
}

impl PreviewCloudRequest {
    #[must_use]
    pub fn new(mounts: Arc<dyn MountResolver>, extractor: Arc<dyn ContentExtractor>) -> Self {
        Self { mounts, extractor }
    }

    /// # Errors
    /// The folder can't be listed.
    pub async fn execute(&self, folder: &StoragePath, clock: QueryClock) -> Result<OrganizeRequest, OrganizeError> {
        let tracker = ProgressTracker::new(Arc::new(NullObserver));
        let inventory =
            take_inventory(&*self.mounts, &*self.extractor, folder, &tracker, &CancelToken::never()).await?;
        Ok(cloud_request(&inventory, clock).0)
    }
}

opaque_debug!(CloudModelSetup, PreviewCloudRequest);

#[cfg(test)]
mod tests {
    use super::*;
    use domain::{EntryKind, EntryMetadata};

    fn entry(name: &str, kind: EntryKind) -> FileEntry {
        FileEntry::new(
            &StoragePath::parse("file:///d").unwrap(),
            PathSegment::new(name).unwrap(),
            kind,
            EntryMetadata::default(),
        )
    }

    #[test]
    fn renames_never_change_the_extension() {
        let pdf = entry("scan0001.pdf", EntryKind::File);
        let name = |s: &str| keep_extension(&pdf, s).map(|n| n.display_name().into_owned());
        assert_eq!(name("Lease agreement.pdf").as_deref(), Some("Lease agreement.pdf"));
        assert_eq!(name("Lease agreement").as_deref(), Some("Lease agreement.pdf"));
        assert_eq!(name("Lease agreement.docx").as_deref(), Some("Lease agreement.pdf"));
        assert_eq!(name("a/b.pdf"), None, "not a valid name");
        assert_eq!(name("scan0001.pdf"), None, "no change");
        let folder = entry("Stuff", EntryKind::Directory);
        assert_eq!(
            keep_extension(&folder, "Archive 2025").map(|n| n.display_name().into_owned()).as_deref(),
            Some("Archive 2025")
        );
    }
}
