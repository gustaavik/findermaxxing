use std::collections::VecDeque;
use std::sync::Arc;

use domain::{EntryKind, FileEntry, SearchMatcher, SearchQuery, StoragePath, Timestamp, VfsError};
use futures::stream::{self, BoxStream};
use futures::{StreamExt, TryStreamExt};

use crate::cancel::CancelToken;
use crate::ports::{DynLister, MountResolver};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SearchEvent {
    /// Matches found in one folder (never empty).
    Matches(Vec<FileEntry>),
    /// How many folders have been searched so far.
    Progress { folders_searched: u64 },
    /// The scope itself could not be searched.
    Failed(VfsError),
    /// The walk is complete; `unreadable_folders` were skipped (e.g. no permission).
    Finished { folders_searched: u64, unreadable_folders: u64 },
}

/// Finds items below a folder by walking it (breadth first, so nearby
/// results come first). The M6 indexer will answer indexed scopes instantly
/// behind this same use case.
pub struct SearchFiles {
    mounts: Arc<dyn MountResolver>,
}

struct Walk {
    lister: Arc<dyn DynLister>,
    matcher: SearchMatcher,
    pending: VecDeque<StoragePath>,
    cancel: CancelToken,
    folders_searched: u64,
    unreadable_folders: u64,
    is_scope: bool,
    finished: bool,
}

impl SearchFiles {
    #[must_use]
    pub fn new(mounts: Arc<dyn MountResolver>) -> Self {
        Self { mounts }
    }

    /// `now` resolves relative date criteria ("past week").
    pub fn execute(
        &self,
        scope: &StoragePath,
        query: &SearchQuery,
        now: Timestamp,
        cancel: CancelToken,
    ) -> BoxStream<'static, SearchEvent> {
        let lister = match self.mounts.resolve(scope.mount()).and_then(|mount| mount.lister()) {
            Ok(lister) => lister,
            Err(error) => return stream::once(async move { SearchEvent::Failed(error) }).boxed(),
        };
        let walk = Walk {
            lister,
            matcher: query.matcher(now),
            pending: VecDeque::from([scope.clone()]),
            cancel,
            folders_searched: 0,
            unreadable_folders: 0,
            is_scope: true,
            finished: false,
        };
        stream::unfold(walk, |mut walk| async move {
            let event = walk.step().await?;
            Some((event, walk))
        })
        .boxed()
    }
}

impl Walk {
    async fn step(&mut self) -> Option<SearchEvent> {
        if self.finished || self.cancel.is_cancelled() {
            return None;
        }
        let Some(folder) = self.pending.pop_front() else {
            self.finished = true;
            return Some(SearchEvent::Finished {
                folders_searched: self.folders_searched,
                unreadable_folders: self.unreadable_folders,
            });
        };
        let listing: Result<Vec<Vec<FileEntry>>, VfsError> =
            self.lister.list_boxed(&folder, self.cancel.clone()).try_collect().await;
        let was_scope = std::mem::replace(&mut self.is_scope, false);
        match listing {
            Ok(batches) => Some(self.visit(batches.into_iter().flatten())),
            Err(VfsError::Cancelled) => None,
            Err(error) if was_scope => {
                self.finished = true;
                Some(SearchEvent::Failed(error))
            }
            Err(_) => {
                self.unreadable_folders += 1;
                Some(SearchEvent::Progress { folders_searched: self.folders_searched })
            }
        }
    }

    fn visit(&mut self, entries: impl Iterator<Item = FileEntry>) -> SearchEvent {
        self.folders_searched += 1;
        let mut matches = Vec::new();
        for entry in entries {
            // Symlinked folders are not followed: no cycles, no duplicate results.
            if entry.kind() == EntryKind::Directory && self.matcher.descends_into(&entry) {
                self.pending.push_back(entry.path().clone());
            }
            if self.matcher.matches(&entry) {
                matches.push(entry);
            }
        }
        if matches.is_empty() {
            SearchEvent::Progress { folders_searched: self.folders_searched }
        } else {
            SearchEvent::Matches(matches)
        }
    }
}

impl std::fmt::Debug for SearchFiles {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SearchFiles").finish_non_exhaustive()
    }
}
