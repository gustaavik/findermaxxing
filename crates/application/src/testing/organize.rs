//! Fakes for organization suggestions: `FakeVfs` tags, an in-memory cloud
//! model store, and a cloud model that answers from a script.

use std::sync::{Mutex, PoisonError};

use domain::{StoragePath, Tag, VfsError};
use futures::FutureExt;
use futures::future::{self, BoxFuture};

use super::FakeVfs;
use crate::ports::{
    CloudModelConfig, CloudModelSettings, CloudModelStore, ModelError, ModelProposal, OrganizationModel,
    OrganizeRequest, Secret, StoreError, TagEditor,
};

impl TagEditor for FakeVfs {
    fn tags<'a>(&'a self, path: &'a StoragePath) -> BoxFuture<'a, Result<Vec<Tag>, VfsError>> {
        future::ready(self.node(path).map(|node| node.tags)).boxed()
    }

    fn set_tags<'a>(&'a self, path: &'a StoragePath, tags: Vec<Tag>) -> BoxFuture<'a, Result<(), VfsError>> {
        let result = self.check_failure(path).and_then(|()| {
            let mut state = self.lock();
            state.nodes.get_mut(path).ok_or(VfsError::NotFound).map(|node| node.tags = tags)
        });
        future::ready(result).boxed()
    }
}

impl FakeVfs {
    /// The tags of the item at `absolute`, as names.
    #[must_use]
    pub fn tag_names(&self, absolute: &str) -> Vec<String> {
        self.lock()
            .nodes
            .get(&self.path(absolute))
            .map(|n| n.tags.iter().map(|t| t.as_str().to_owned()).collect())
            .unwrap_or_default()
    }
}

#[derive(Debug, Default)]
pub struct InMemoryCloudModels {
    settings: Mutex<Option<CloudModelSettings>>,
}

impl CloudModelStore for InMemoryCloudModels {
    fn load(&self) -> BoxFuture<'static, Result<Option<CloudModelSettings>, StoreError>> {
        future::ready(Ok(self.settings.lock().unwrap_or_else(PoisonError::into_inner).clone())).boxed()
    }

    fn save(&self, settings: CloudModelSettings) -> BoxFuture<'static, Result<(), StoreError>> {
        *self.settings.lock().unwrap_or_else(PoisonError::into_inner) = Some(settings);
        future::ready(Ok(())).boxed()
    }

    fn clear(&self) -> BoxFuture<'static, Result<(), StoreError>> {
        *self.settings.lock().unwrap_or_else(PoisonError::into_inner) = None;
        future::ready(Ok(())).boxed()
    }
}

/// Answers every request with the same proposal (or error) and keeps the
/// requests, so tests can check exactly what would have been sent.
#[derive(Debug)]
pub struct ScriptedModel {
    answer: Result<ModelProposal, ModelError>,
    pub requests: Mutex<Vec<(CloudModelConfig, String, OrganizeRequest)>>,
}

impl ScriptedModel {
    #[must_use]
    pub fn new(answer: Result<ModelProposal, ModelError>) -> Self {
        Self { answer, requests: Mutex::default() }
    }

    #[must_use]
    pub fn requests(&self) -> Vec<(CloudModelConfig, String, OrganizeRequest)> {
        self.requests.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }
}

impl OrganizationModel for ScriptedModel {
    fn propose(
        &self,
        config: &CloudModelConfig,
        key: &Secret,
        request: &OrganizeRequest,
    ) -> BoxFuture<'static, Result<ModelProposal, ModelError>> {
        self.requests.lock().unwrap_or_else(PoisonError::into_inner).push((
            config.clone(),
            key.expose().to_owned(),
            request.clone(),
        ));
        future::ready(self.answer.clone()).boxed()
    }
}
