//! Analyses one folder into a [`Plan`] of suggestions (ADR 0029). Never
//! changes anything. In order:
//!
//! 1. clutter (rules): old downloads, installers, large untouched files,
//!    exact and look-alike duplicates → the Trash;
//! 2. the user's cloud model, only for a folder they opted in: groups,
//!    renames, tags;
//! 3. local rules filling the gaps: screenshot piles and photo months →
//!    folders, camera names → dates, generic document names → their title,
//!    keywords → tags (local files only);
//! 4. the local language model, when installed and the cloud wasn't used:
//!    documents about the same thing → a folder named after their
//!    distinctive word.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

use domain::{
    ByteSize, Capability, Change, CivilTime, ClutterReason, DuplicateMatch, EntryKind, FileEntry, ImageFormat,
    MIN_PHOTOS_PER_MONTH, PathSegment, Plan, QueryClock, StoragePath, SuggestionSource, VfsError, cluster, clutter_of,
    dated_name, group_label, is_camera_name, is_downloads_folder, is_generic_name, is_screenshot, parse_exif_time,
    passages, photo_month_folder, tags_for_text, titled_name,
};

use super::cloud::{CloudParts, ask_cloud};
use super::gather::{Inventory, take_inventory};
use super::{OrganizeError, OrganizeParts};
use crate::cancel::CancelToken;
use crate::operations::progress::ProgressTracker;
use crate::ports::{JobObserver, ModelPack};
use crate::use_cases::DuplicateSearch;
use crate::use_cases::utilities::{local_copy, opaque_debug};

/// The folder screenshot piles are gathered in.
const SCREENSHOTS_FOLDER: &str = "Screenshots";
/// The EXIF field with the moment a photo was taken.
const EXIF_TAKEN: &str = "DateTimeOriginal";
/// Photos whose EXIF is read per analysis.
const MAX_PHOTOS: usize = 500;

/// Whether the cloud model may be asked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloudUse {
    Never,
    /// Only if the user opted this folder in and a key is set.
    IfOptedIn,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Analysis {
    pub plan: Plan,
    /// Names taken in the folder, for resolving the accepted suggestions.
    pub existing: HashSet<PathSegment>,
    /// Things the user should know ("turn on AI search for …", a cloud error).
    pub notes: Vec<String>,
    pub used_cloud: bool,
}

pub struct AnalyzeFolder {
    parts: OrganizeParts,
}

impl AnalyzeFolder {
    #[must_use]
    pub fn new(parts: OrganizeParts) -> Self {
        Self { parts }
    }

    /// # Errors
    /// The folder can't be listed, or `cancel` fired.
    pub async fn execute(
        &self,
        folder: &StoragePath,
        clock: QueryClock,
        cloud: CloudUse,
        observer: Arc<dyn JobObserver>,
        cancel: CancelToken,
    ) -> Result<Analysis, OrganizeError> {
        let parts = &self.parts;
        let tracker = ProgressTracker::new(observer);
        let inventory = take_inventory(&*parts.mounts, &*parts.extractor, folder, &tracker, &cancel).await?;
        let mut plan = Plan { folder: Some(folder.clone()), suggestions: Vec::new() };
        let mut notes = Vec::new();
        self.clutter(&inventory, clock, &mut plan, &cancel).await?;
        let used_cloud = match cloud {
            CloudUse::IfOptedIn => {
                match ask_cloud(&parts.cloud, &inventory, clock, &mut plan, self.can_tag(folder)).await {
                    Ok(used) => used,
                    Err(error) => {
                        notes.push(format!("The cloud model couldn't be used ({error}); these are local suggestions."));
                        false
                    }
                }
            }
            CloudUse::Never => false,
        };
        self.screenshots(&inventory, &mut plan)?;
        self.photos(&inventory, clock, &mut plan, &cancel).await?;
        self.documents(&inventory, &mut plan, folder)?;
        if !used_cloud {
            self.groups(&inventory, &mut plan, &mut notes).await?;
        }
        Ok(Analysis { plan, existing: inventory.existing, notes, used_cloud })
    }

    fn can_tag(&self, folder: &StoragePath) -> bool {
        self.parts.mounts.resolve(folder.mount()).is_ok_and(|m| m.capabilities().contains(Capability::EditTags))
    }

    async fn clutter(
        &self,
        inventory: &Inventory,
        clock: QueryClock,
        plan: &mut Plan,
        cancel: &CancelToken,
    ) -> Result<(), OrganizeError> {
        let in_downloads = inventory.folder.name().is_some_and(|n| is_downloads_folder(&n.display_name()));
        for entry in &inventory.entries {
            if let Some(reason) = clutter_of(entry, in_downloads, clock.now, &self.parts.policy) {
                let text = clutter_text(&reason);
                plan.suggest(entry.path().clone(), entry.kind(), Change::Trash(reason), text, SuggestionSource::Rules);
            }
        }
        let search = DuplicateSearch {
            folders: vec![inventory.folder.clone()],
            min_size: ByteSize(1),
            include_hidden: false,
            similar_images: true,
            top_level_only: true,
        };
        let groups = match self
            .parts
            .duplicates
            .execute(&search, Arc::new(crate::operations::NullObserver), cancel.clone())
            .await
        {
            Ok(groups) => groups,
            Err(VfsError::Cancelled) => return Err(VfsError::Cancelled.into()),
            Err(_) => Vec::new(),
        };
        for group in groups {
            let Some((keeper, copies)) = group.items.split_first() else { continue };
            let keeper_name = keeper.name().display_name().into_owned();
            for copy in copies {
                let reason = match group.matching {
                    DuplicateMatch::Identical => ClutterReason::DuplicateOf(keeper_name.clone()),
                    DuplicateMatch::Similar => ClutterReason::LooksLike(keeper_name.clone()),
                };
                let change = Change::Trash(reason.clone());
                if !plan.has(copy.path(), &change) {
                    plan.suggest(
                        copy.path().clone(),
                        copy.kind(),
                        change,
                        clutter_text(&reason),
                        SuggestionSource::Rules,
                    );
                }
            }
        }
        Ok(())
    }

    fn screenshots(&self, inventory: &Inventory, plan: &mut Plan) -> Result<(), OrganizeError> {
        let folder_is_screenshots =
            inventory.folder.name().is_some_and(|n| n.display_name().eq_ignore_ascii_case(SCREENSHOTS_FOLDER));
        let shots: Vec<&FileEntry> = inventory.entries.iter().filter(|e| is_screenshot(e)).collect();
        if folder_is_screenshots || shots.len() < self.parts.policy.screenshot_pile {
            return Ok(());
        }
        let folder = PathSegment::new(SCREENSHOTS_FOLDER).map_err(VfsError::from)?;
        let reason = format!("One of {} screenshots here", shots.len());
        for shot in shots {
            let change = Change::MoveInto(folder.clone());
            if !plan.has(shot.path(), &change) {
                plan.suggest(shot.path().clone(), shot.kind(), change, reason.clone(), SuggestionSource::Rules);
            }
        }
        Ok(())
    }

    /// When each photo was taken: EXIF first, the file's date otherwise.
    async fn taken_times(
        &self,
        photos: &[&FileEntry],
        clock: QueryClock,
        cancel: &CancelToken,
    ) -> Result<HashMap<StoragePath, CivilTime>, OrganizeError> {
        let mut taken = HashMap::new();
        for photo in photos.iter().take(MAX_PHOTOS) {
            if cancel.is_cancelled() {
                return Err(VfsError::Cancelled.into());
            }
            let from_exif = match local_copy(&*self.parts.mounts, photo.path()).await {
                Ok(local) => self.parts.image_metadata.read(local).await.ok().and_then(|fields| {
                    fields.iter().find(|f| f.tag == EXIF_TAKEN).and_then(|f| parse_exif_time(&f.value))
                }),
                Err(_) => None,
            };
            let time = from_exif.or_else(|| photo.metadata().modified.map(|m| m.civil(clock.utc_offset_minutes)));
            if let Some(time) = time {
                taken.insert(photo.path().clone(), time);
            }
        }
        Ok(taken)
    }

    async fn photos(
        &self,
        inventory: &Inventory,
        clock: QueryClock,
        plan: &mut Plan,
        cancel: &CancelToken,
    ) -> Result<(), OrganizeError> {
        let photos: Vec<&FileEntry> = inventory
            .entries
            .iter()
            .filter(|e| e.kind() == EntryKind::File && ImageFormat::of(e.name()).is_some() && !is_screenshot(e))
            .collect();
        let taken = self.taken_times(&photos, clock, cancel).await?;
        for photo in &photos {
            let Some(time) = taken.get(photo.path()) else { continue };
            if is_camera_name(photo.name()) {
                let change = Change::Rename(dated_name(photo.name(), *time).map_err(VfsError::from)?);
                if !plan.has(photo.path(), &change) {
                    let reason = "Named after when it was taken".to_owned();
                    plan.suggest(photo.path().clone(), photo.kind(), change, reason, SuggestionSource::Rules);
                }
            }
        }
        let mut months: BTreeMap<(i64, i64), Vec<&FileEntry>> = BTreeMap::new();
        for photo in &photos {
            if let Some(time) = taken.get(photo.path()) {
                months.entry((time.year, time.month)).or_default().push(photo);
            }
        }
        if months.len() < 2 {
            return Ok(());
        }
        for ((year, month), members) in months.into_iter().filter(|(_, m)| m.len() >= MIN_PHOTOS_PER_MONTH) {
            let folder = photo_month_folder(year, month).map_err(VfsError::from)?;
            let reason = format!("{} photos from {year:04}-{month:02}", members.len());
            for photo in members {
                let change = Change::MoveInto(folder.clone());
                if !plan.has(photo.path(), &change) {
                    plan.suggest(photo.path().clone(), photo.kind(), change, reason.clone(), SuggestionSource::Rules);
                }
            }
        }
        Ok(())
    }

    fn documents(&self, inventory: &Inventory, plan: &mut Plan, folder: &StoragePath) -> Result<(), OrganizeError> {
        let can_tag = self.can_tag(folder);
        for (document, text) in inventory.documents() {
            if is_generic_name(document.name())
                && let Some(title) = titled_name(document.name(), text)
            {
                let change = Change::Rename(title);
                if !plan.has(document.path(), &change) {
                    let reason = "Named after its first line".to_owned();
                    plan.suggest(document.path().clone(), document.kind(), change, reason, SuggestionSource::Rules);
                }
            }
            let tags = tags_for_text(text);
            if can_tag && !tags.is_empty() {
                let change = Change::AddTags(tags);
                if !plan.has(document.path(), &change) {
                    let reason = "Words in the document".to_owned();
                    plan.suggest(document.path().clone(), document.kind(), change, reason, SuggestionSource::Rules);
                }
            }
        }
        Ok(())
    }

    /// Documents about the same thing, by the local model's embeddings.
    async fn groups(
        &self,
        inventory: &Inventory,
        plan: &mut Plan,
        notes: &mut Vec<String>,
    ) -> Result<(), OrganizeError> {
        let documents: Vec<(&FileEntry, &str)> = inventory.documents().collect();
        if documents.len() < domain::MIN_GROUP_SIZE {
            return Ok(());
        }
        if !self.parts.installer.installed(ModelPack::Text) {
            notes.push("Turn on AI search to also group documents by what they're about.".to_owned());
            return Ok(());
        }
        let first_passages: Vec<String> =
            documents.iter().map(|(_, text)| passages(text).into_iter().next().unwrap_or_default()).collect();
        let vectors = match self.parts.text.embed_passages(first_passages).await {
            Ok(vectors) => vectors,
            Err(error) => {
                notes.push(format!("Documents couldn't be grouped by content ({error})."));
                return Ok(());
            }
        };
        let mut used: HashSet<PathSegment> = HashSet::new();
        for group in cluster(&vectors, self.parts.text.grouping_threshold()) {
            let inside: Vec<&str> = group.iter().map(|i| documents[*i].1).collect();
            let outside: Vec<&str> =
                (0..documents.len()).filter(|i| !group.contains(i)).map(|i| documents[i].1).collect();
            let Some(label) = group_label(&inside, &outside) else { continue };
            // An existing file of that name can't become a folder; a second group with the same label waits.
            let clashes_with_file =
                inventory.entries.iter().any(|e| e.name() == &label && e.kind() != EntryKind::Directory);
            // Only documents that mention the folder's word join it: meaning
            // alone pulls in short texts that merely look alike.
            let word = label.display_name().to_lowercase();
            let members: Vec<usize> =
                group.into_iter().filter(|i| documents[*i].1.to_lowercase().contains(&word)).collect();
            if clashes_with_file || members.len() < domain::MIN_GROUP_SIZE || !used.insert(label.clone()) {
                continue;
            }
            let reason = format!("{} documents about “{}”", members.len(), label.display_name());
            for index in members {
                let document = documents[index].0;
                let change = Change::MoveInto(label.clone());
                if !plan.has(document.path(), &change) {
                    plan.suggest(
                        document.path().clone(),
                        document.kind(),
                        change,
                        reason.clone(),
                        SuggestionSource::LocalModel,
                    );
                }
            }
        }
        Ok(())
    }
}

/// The reason line for a clutter suggestion.
#[must_use]
pub fn clutter_text(reason: &ClutterReason) -> String {
    match reason {
        ClutterReason::OldDownload { days } => format!("Downloaded and untouched for {days} days"),
        ClutterReason::Installer { days } => format!("An installer from {days} days ago, probably installed already"),
        ClutterReason::LargeAndUnchanged { size, days } => {
            format!("{} MB, unchanged for {days} days", size.0 / (1024 * 1024))
        }
        ClutterReason::DuplicateOf(name) => format!("An exact copy of “{name}”, which stays"),
        ClutterReason::LooksLike(name) => format!("Looks the same as “{name}”, which stays"),
    }
}

opaque_debug!(AnalyzeFolder);

impl std::fmt::Debug for CloudParts {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CloudParts").finish_non_exhaustive()
    }
}
