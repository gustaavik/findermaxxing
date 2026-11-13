//! `CloudModelStore`: which cloud model to use and the folders opted in.
//! Never the API key; that lives in the OS keychain.

use application::ports::{CloudModelConfig, CloudModelSettings, CloudModelStore, CloudProvider, StoreError};
use futures::future::BoxFuture;
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};

use crate::location_codec::{corrupt, decode_locations, encode_locations};
use crate::{SqliteStore, unavailable};

const ANTHROPIC: &str = "anthropic";
const OPENAI_COMPATIBLE: &str = "openai-compatible";

#[derive(Serialize, Deserialize)]
struct Stored {
    provider: String,
    base_url: String,
    model: String,
    opted_in: String,
}

impl CloudModelStore for SqliteStore {
    fn load(&self) -> BoxFuture<'static, Result<Option<CloudModelSettings>, StoreError>> {
        self.run(|connection| {
            let json: Option<String> = connection
                .query_row("SELECT settings FROM cloud_model WHERE id = 1", [], |row| row.get(0))
                .optional()
                .map_err(|e| unavailable(&e))?;
            json.map(|json| {
                let stored: Stored = serde_json::from_str(&json).map_err(corrupt)?;
                let provider = match stored.provider.as_str() {
                    ANTHROPIC => CloudProvider::Anthropic,
                    OPENAI_COMPATIBLE => CloudProvider::OpenAiCompatible,
                    other => return Err(corrupt(format!("unknown provider {other:?}"))),
                };
                Ok(CloudModelSettings {
                    config: CloudModelConfig { provider, base_url: stored.base_url, model: stored.model },
                    opted_in: decode_locations(&stored.opted_in)?,
                })
            })
            .transpose()
        })
    }

    fn save(&self, settings: CloudModelSettings) -> BoxFuture<'static, Result<(), StoreError>> {
        self.run(move |connection| {
            let provider = match settings.config.provider {
                CloudProvider::Anthropic => ANTHROPIC,
                CloudProvider::OpenAiCompatible => OPENAI_COMPATIBLE,
            };
            let stored = Stored {
                provider: provider.to_owned(),
                base_url: settings.config.base_url,
                model: settings.config.model,
                opted_in: encode_locations(&settings.opted_in)?,
            };
            let json = serde_json::to_string(&stored).map_err(corrupt)?;
            connection
                .execute("INSERT OR REPLACE INTO cloud_model (id, settings) VALUES (1, ?1)", [json])
                .map_err(|e| unavailable(&e))
                .map(drop)
        })
    }

    fn clear(&self) -> BoxFuture<'static, Result<(), StoreError>> {
        self.run(|connection| connection.execute("DELETE FROM cloud_model", []).map_err(|e| unavailable(&e)).map(drop))
    }
}
