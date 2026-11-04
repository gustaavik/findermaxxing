use std::sync::Arc;

use domain::{EntryBatch, StoragePath, VfsError};
use futures::stream::{self, BoxStream};
use futures::{StreamExt, future};

use crate::cancel::CancelToken;
use crate::ports::MountResolver;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum HiddenFiles {
    Show,
    #[default]
    Hide,
}

impl HiddenFiles {
    #[must_use]
    pub fn admits(self, is_hidden: bool) -> bool {
        self == Self::Show || !is_hidden
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListRequest {
    pub dir: StoragePath,
    pub hidden: HiddenFiles,
}

/// Streams the entries of one directory, in provider order, in batches.
/// Sorting is left to the caller because it changes without reloading.
pub struct ListDirectory {
    mounts: Arc<dyn MountResolver>,
}

impl ListDirectory {
    #[must_use]
    pub fn new(mounts: Arc<dyn MountResolver>) -> Self {
        Self { mounts }
    }

    pub fn execute(
        &self,
        request: &ListRequest,
        cancel: CancelToken,
    ) -> BoxStream<'static, Result<EntryBatch, VfsError>> {
        match self.mounts.resolve(request.dir.mount()).and_then(|mount| mount.lister()) {
            Ok(lister) => filter_hidden(lister.list_boxed(&request.dir, cancel), request.hidden),
            Err(error) => stream::once(future::ready(Err(error))).boxed(),
        }
    }
}

fn filter_hidden(
    batches: BoxStream<'static, Result<EntryBatch, VfsError>>,
    hidden: HiddenFiles,
) -> BoxStream<'static, Result<EntryBatch, VfsError>> {
    if hidden == HiddenFiles::Show {
        return batches;
    }
    batches
        .map(move |batch| batch.map(|entries| visible_only(entries, hidden)))
        .filter(|batch| future::ready(!matches!(batch, Ok(entries) if entries.is_empty())))
        .boxed()
}

fn visible_only(entries: EntryBatch, hidden: HiddenFiles) -> EntryBatch {
    entries.into_iter().filter(|entry| hidden.admits(entry.is_hidden())).collect()
}

impl std::fmt::Debug for ListDirectory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ListDirectory").finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::CancellationSource;
    use crate::ProviderRegistry;
    use crate::testing::FakeVfs;
    use futures::TryStreamExt;
    use futures_executor::block_on;

    fn use_case(fake: &FakeVfs) -> ListDirectory {
        let registry = ProviderRegistry::default().with_provider(fake.clone().into_factory()).unwrap();
        ListDirectory::new(Arc::new(registry))
    }

    fn collect_names(stream: BoxStream<'static, Result<EntryBatch, VfsError>>) -> Result<Vec<String>, VfsError> {
        let batches: Vec<EntryBatch> = block_on(stream.try_collect())?;
        let mut names: Vec<String> = batches.into_iter().flatten().map(|e| e.name().to_string()).collect();
        names.sort();
        Ok(names)
    }

    #[test]
    fn hides_hidden_entries_by_default() {
        let fake = FakeVfs::new();
        fake.add_dir("/docs");
        fake.add_file("/docs/a.txt", 3);
        fake.add_file("/docs/.secret", 3);
        let request = ListRequest { dir: fake.path("/docs"), hidden: HiddenFiles::Hide };
        assert_eq!(collect_names(use_case(&fake).execute(&request, CancelToken::never())).unwrap(), ["a.txt"]);
    }

    #[test]
    fn shows_hidden_entries_when_asked() {
        let fake = FakeVfs::new();
        fake.add_dir("/docs");
        fake.add_file("/docs/a.txt", 3);
        fake.add_file("/docs/.secret", 3);
        let request = ListRequest { dir: fake.path("/docs"), hidden: HiddenFiles::Show };
        assert_eq!(
            collect_names(use_case(&fake).execute(&request, CancelToken::never())).unwrap(),
            [".secret", "a.txt"]
        );
    }

    #[test]
    fn unknown_scheme_is_reported_as_error_item() {
        let fake = FakeVfs::new();
        let request = ListRequest { dir: StoragePath::parse("nope:///x").unwrap(), hidden: HiddenFiles::Show };
        let result = collect_names(use_case(&fake).execute(&request, CancelToken::never()));
        assert_eq!(result, Err(VfsError::UnknownScheme("nope".into())));
    }

    #[test]
    fn cancelled_listing_reports_cancelled() {
        let fake = FakeVfs::new();
        fake.add_dir("/big");
        for i in 0..2_000 {
            fake.add_file(&format!("/big/f{i}"), 1);
        }
        let source = CancellationSource::new();
        source.cancel();
        let request = ListRequest { dir: fake.path("/big"), hidden: HiddenFiles::Show };
        let result = collect_names(use_case(&fake).execute(&request, source.token()));
        assert_eq!(result, Err(VfsError::Cancelled));
    }
}
