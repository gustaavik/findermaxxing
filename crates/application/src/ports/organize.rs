//! Ports of organization suggestions (ADR 0029): native tags as a provider
//! role, and the user's own cloud model with its settings.

use domain::{StoragePath, Tag, VfsError};
use futures::future::BoxFuture;

use super::accounts::Secret;
use super::saved_searches::StoreError;

/// Provider role: native tags (Finder tags, `user.xdg.tags`).
pub trait TagEditor: Send + Sync {
    fn tags<'a>(&'a self, path: &'a StoragePath) -> BoxFuture<'a, Result<Vec<Tag>, VfsError>>;
    /// Replaces the item's tags; an empty list removes them.
    fn set_tags<'a>(&'a self, path: &'a StoragePath, tags: Vec<Tag>) -> BoxFuture<'a, Result<(), VfsError>>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CloudProvider {
    /// The Anthropic Messages API.
    Anthropic,
    /// Any server speaking OpenAI's chat-completions protocol (OpenAI,
    /// Azure, Mistral, OpenRouter, Ollama, LM Studio, …).
    OpenAiCompatible,
}

/// Which model to ask; the key lives in the keychain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CloudModelConfig {
    pub provider: CloudProvider,
    /// The API's base URL, e.g. `https://api.anthropic.com`.
    pub base_url: String,
    pub model: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CloudModelSettings {
    pub config: CloudModelConfig,
    /// Folders the user agreed to send summaries of. Nothing else is ever sent.
    pub opted_in: Vec<StoragePath>,
}

pub trait CloudModelStore: Send + Sync {
    fn load(&self) -> BoxFuture<'static, Result<Option<CloudModelSettings>, StoreError>>;
    fn save(&self, settings: CloudModelSettings) -> BoxFuture<'static, Result<(), StoreError>>;
    fn clear(&self) -> BoxFuture<'static, Result<(), StoreError>>;
}

/// One item as the cloud model sees it: no path, no contents beyond a short
/// excerpt of a document's text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItemSummary {
    pub id: u32,
    pub name: String,
    /// `folder`, `pdf`, `document`, `image`, … (the item's category).
    pub kind: String,
    pub size: Option<u64>,
    /// `YYYY-MM-DD`.
    pub modified: Option<String>,
    pub excerpt: Option<String>,
}

/// Everything sent for one folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrganizeRequest {
    /// The folder's own name, not its path.
    pub folder_name: String,
    pub items: Vec<ItemSummary>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProposedGroup {
    pub folder: String,
    pub item_ids: Vec<u32>,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProposedRename {
    pub item_id: u32,
    pub name: String,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProposedTags {
    pub item_id: u32,
    pub tags: Vec<String>,
}

/// The model's answer, unchecked: names and ids are validated by the use case.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ModelProposal {
    pub groups: Vec<ProposedGroup>,
    pub renames: Vec<ProposedRename>,
    pub tags: Vec<ProposedTags>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ModelError {
    #[error("the API key was refused")]
    Unauthorized,
    #[error("the service is busy or the quota is used up; try again later")]
    RateLimited,
    #[error("the service couldn't be reached: {0}")]
    Network(String),
    #[error("the service answered with an error: {0}")]
    Service(String),
    #[error("the answer couldn't be understood: {0}")]
    BadAnswer(String),
}

/// The user's cloud model.
pub trait OrganizationModel: Send + Sync {
    fn propose(
        &self,
        config: &CloudModelConfig,
        key: &Secret,
        request: &OrganizeRequest,
    ) -> BoxFuture<'static, Result<ModelProposal, ModelError>>;
}
