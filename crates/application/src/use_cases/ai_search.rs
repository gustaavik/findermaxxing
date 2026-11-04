//! Turning AI search on and off, choosing what it covers, and installing
//! its models. Every change is saved, then the indexer is told.

use std::sync::Arc;

use domain::{IndexScope, StoragePath, VfsError};

use crate::cancel::CancelToken;
use crate::ports::{
    AiSettings, AiSettingsStore, ModelInstaller, ModelPack, ProgressCallback, SemanticIndex, StoreError,
};
use crate::use_cases::indexer::IndexerControl;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AiSearchError {
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error("the model couldn't be installed: {0}")]
    Install(VfsError),
}

#[derive(Clone)]
pub struct AiSearchServices {
    pub settings: Arc<dyn AiSettingsStore>,
    pub installer: Arc<dyn ModelInstaller>,
    pub index: Arc<dyn SemanticIndex>,
    pub control: IndexerControl,
    /// Documents, Desktop, Downloads and Pictures; used the first time.
    pub default_scope: Vec<StoragePath>,
}

impl AiSearchServices {
    async fn current(&self) -> Result<AiSettings, AiSearchError> {
        Ok(self.settings.load().await?.unwrap_or_else(|| AiSettings {
            scope: IndexScope { include: self.default_scope.clone(), exclude: Vec::new() },
            ..AiSettings::default()
        }))
    }

    async fn change(&self, edit: impl FnOnce(&mut AiSettings)) -> Result<AiSettings, AiSearchError> {
        let _changing = self.control.lock_settings().await;
        let mut settings = self.current().await?;
        edit(&mut settings);
        self.settings.save(settings.clone()).await?;
        self.control.settings_changed();
        Ok(settings)
    }
}

/// Reads the settings (with the default scope before the first choice).
pub struct GetAiSettings {
    services: AiSearchServices,
}

impl GetAiSettings {
    #[must_use]
    pub fn new(services: AiSearchServices) -> Self {
        Self { services }
    }

    /// # Errors
    /// The store's error.
    pub async fn execute(&self) -> Result<AiSettings, AiSearchError> {
        self.services.current().await
    }
}

/// Installs a model pack (if needed) after the user agreed, then turns the
/// matching search on.
pub struct EnableAiSearch {
    services: AiSearchServices,
}

impl EnableAiSearch {
    #[must_use]
    pub fn new(services: AiSearchServices) -> Self {
        Self { services }
    }

    /// # Errors
    /// The download failed or was cancelled (nothing is turned on then).
    pub async fn execute(
        &self,
        pack: ModelPack,
        progress: ProgressCallback,
        cancel: CancelToken,
    ) -> Result<AiSettings, AiSearchError> {
        let s = &self.services;
        if !s.installer.installed(pack) {
            s.installer.install(pack, progress, cancel).await.map_err(AiSearchError::Install)?;
        }
        s.change(|settings| match pack {
            ModelPack::Text => settings.enabled = true,
            ModelPack::Images => settings.images = true,
        })
        .await
    }
}

/// Turns AI search (or just image search) off. With `forget`, the index is
/// deleted and the models removed from disk too.
pub struct DisableAiSearch {
    services: AiSearchServices,
}

impl DisableAiSearch {
    #[must_use]
    pub fn new(services: AiSearchServices) -> Self {
        Self { services }
    }

    /// # Errors
    /// The store's error, or the models couldn't be removed.
    pub async fn execute(&self, pack: ModelPack, forget: bool) -> Result<AiSettings, AiSearchError> {
        let s = &self.services;
        let settings = s
            .change(|settings| match pack {
                ModelPack::Text => {
                    settings.enabled = false;
                    settings.images = false;
                }
                ModelPack::Images => settings.images = false,
            })
            .await?;
        if forget {
            if pack == ModelPack::Text {
                // The indexer was told above; this waits out a write in flight.
                let _writes = s.control.lock_writes().await;
                s.index.clear().await?;
                s.installer.uninstall(ModelPack::Text).await.map_err(AiSearchError::Install)?;
            }
            s.installer.uninstall(ModelPack::Images).await.map_err(AiSearchError::Install)?;
        }
        Ok(settings)
    }
}

/// Pauses or resumes indexing (search keeps working on what's indexed).
pub struct PauseIndexing {
    services: AiSearchServices,
}

impl PauseIndexing {
    #[must_use]
    pub fn new(services: AiSearchServices) -> Self {
        Self { services }
    }

    /// # Errors
    /// The store's error.
    pub async fn execute(&self, paused: bool) -> Result<AiSettings, AiSearchError> {
        self.services.change(|settings| settings.paused = paused).await
    }
}

/// How the scope changes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScopeChange {
    Include(StoragePath),
    Exclude(StoragePath),
    /// Takes a folder off either list.
    Forget(StoragePath),
}

pub struct ChangeIndexScope {
    services: AiSearchServices,
}

impl ChangeIndexScope {
    #[must_use]
    pub fn new(services: AiSearchServices) -> Self {
        Self { services }
    }

    /// # Errors
    /// The store's error.
    pub async fn execute(&self, change: ScopeChange) -> Result<AiSettings, AiSearchError> {
        self.services
            .change(|settings| {
                let scope = &mut settings.scope;
                match change {
                    ScopeChange::Include(folder) => {
                        scope.exclude.retain(|f| *f != folder);
                        if !scope.include.contains(&folder) {
                            scope.include.push(folder);
                        }
                    }
                    ScopeChange::Exclude(folder) => {
                        scope.include.retain(|f| *f != folder);
                        if !scope.exclude.contains(&folder) {
                            scope.exclude.push(folder);
                        }
                    }
                    ScopeChange::Forget(folder) => {
                        scope.include.retain(|f| *f != folder);
                        scope.exclude.retain(|f| *f != folder);
                    }
                }
            })
            .await
    }
}

macro_rules! opaque_debug {
    ($($name:ident),+) => {$(
        impl std::fmt::Debug for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.debug_struct(stringify!($name)).finish_non_exhaustive()
            }
        }
    )+};
}

opaque_debug!(GetAiSettings, EnableAiSearch, DisableAiSearch, PauseIndexing, ChangeIndexScope, AiSearchServices);
