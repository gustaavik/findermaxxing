//! `SemanticIndex` and `AiSettingsStore` (ADR 0025).
//!
//! Vectors are stored int8-quantised with one scale per vector: a quarter of
//! the size of f32, and dot products stay within ~1 % of exact, far below the
//! differences that decide a ranking. The whole index is loaded into memory
//! on first use and searched by brute force: at ~100 k passages that is a few
//! milliseconds, with no approximate-search structure to keep in sync.
//! Writes go to SQLite and the in-memory copy together.

use std::collections::HashMap;
use std::sync::{Arc, PoisonError};

use application::ports::{
    AiSettings, AiSettingsStore, Embedding, IndexHit, IndexQuery, IndexedItem, IndexedPassage, ModelId, SemanticIndex,
    StoreError,
};
use domain::{ByteSize, ContentKind, FileCategory, Fingerprint, IndexScope, StoragePath, Timestamp};
use futures::future::BoxFuture;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

use crate::location_codec::{corrupt, decode_location, decode_locations, encode_locations};
use crate::{SqliteStore, unavailable};

const QUANT_MAX: f32 = 127.0;

struct Stored {
    snippet: String,
    scale: f32,
    vector: Vec<i8>,
}

struct Entry {
    item: IndexedItem,
    passages: Vec<Stored>,
}

/// The in-memory copy of the index.
#[derive(Default)]
pub(crate) struct Vectors {
    entries: HashMap<StoragePath, Entry>,
}

fn quantize(vector: &[f32]) -> (f32, Vec<i8>) {
    let max = vector.iter().fold(0.0_f32, |m, v| m.max(v.abs()));
    if max == 0.0 {
        return (0.0, vec![0; vector.len()]);
    }
    let scale = max / QUANT_MAX;
    #[allow(clippy::cast_possible_truncation, reason = "rounded and clamped to the i8 range")]
    let quantized = vector.iter().map(|v| (v / scale).round().clamp(-QUANT_MAX, QUANT_MAX) as i8).collect();
    (scale, quantized)
}

fn dot(query: &[f32], stored: &Stored) -> f32 {
    stored.scale * query.iter().zip(&stored.vector).map(|(q, v)| q * f32::from(*v)).sum::<f32>()
}

const TEXT: &str = "text";
const IMAGE: &str = "image";

fn kind_name(kind: ContentKind) -> &'static str {
    match kind {
        ContentKind::Text => TEXT,
        ContentKind::Image => IMAGE,
    }
}

fn kind_of(name: &str) -> Result<ContentKind, StoreError> {
    match name {
        TEXT => Ok(ContentKind::Text),
        IMAGE => Ok(ContentKind::Image),
        other => Err(corrupt(format!("unknown content kind {other:?}"))),
    }
}

const CATEGORIES: &[(&str, FileCategory)] = &[
    ("folder", FileCategory::Folder),
    ("image", FileCategory::Image),
    ("pdf", FileCategory::Pdf),
    ("video", FileCategory::Video),
    ("audio", FileCategory::Audio),
    ("text", FileCategory::Text),
    ("archive", FileCategory::Archive),
    ("document", FileCategory::Document),
    ("other", FileCategory::Other),
];

fn category_name(category: FileCategory) -> &'static str {
    CATEGORIES.iter().find(|(_, c)| *c == category).map_or("other", |(name, _)| name)
}

fn category_of(name: &str) -> Result<FileCategory, StoreError> {
    CATEGORIES
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, c)| *c)
        .ok_or_else(|| corrupt(format!("unknown category {name:?}")))
}

fn load(connection: &Connection) -> Result<Vectors, StoreError> {
    let mut entries = HashMap::new();
    let mut items = connection
        .prepare("SELECT location, kind, category, size, modified, model FROM semantic_items")
        .map_err(|e| unavailable(&e))?;
    let rows = items
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Option<i64>>(3)?,
                row.get::<_, Option<i64>>(4)?,
                row.get::<_, String>(5)?,
            ))
        })
        .map_err(|e| unavailable(&e))?;
    for row in rows {
        let (location, kind, category, size, modified, model) = row.map_err(|e| unavailable(&e))?;
        let location = decode_location(&location)?;
        let item = IndexedItem {
            location: location.clone(),
            kind: kind_of(&kind)?,
            category: category_of(&category)?,
            fingerprint: Fingerprint {
                size: size.map(|s| u64::try_from(s).map(ByteSize)).transpose().map_err(corrupt)?,
                modified: modified.map(Timestamp::from_unix_millis),
            },
            model: ModelId(model),
        };
        entries.insert(location, Entry { item, passages: Vec::new() });
    }
    let mut passages = connection
        .prepare("SELECT location, snippet, scale, vector FROM semantic_passages ORDER BY location, ordinal")
        .map_err(|e| unavailable(&e))?;
    let rows = passages
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, f64>(2)?, row.get::<_, Vec<u8>>(3)?))
        })
        .map_err(|e| unavailable(&e))?;
    for row in rows {
        let (location, snippet, scale, bytes) = row.map_err(|e| unavailable(&e))?;
        if let Some(entry) = entries.get_mut(&decode_location(&location)?) {
            #[allow(clippy::cast_possible_truncation, reason = "stored from an f32")]
            let scale = scale as f32;
            let vector = bytes.into_iter().map(|b| i8::from_ne_bytes([b])).collect();
            entry.passages.push(Stored { snippet, scale, vector });
        }
    }
    Ok(Vectors { entries })
}

impl SqliteStore {
    /// Runs `work` with the connection and the loaded in-memory index.
    fn with_vectors<T: Send + 'static>(
        &self,
        work: impl FnOnce(&Connection, &mut Vectors) -> Result<T, StoreError> + Send + 'static,
    ) -> BoxFuture<'static, Result<T, StoreError>> {
        let vectors = Arc::clone(&self.vectors);
        self.run(move |connection| {
            let mut vectors = vectors.lock().unwrap_or_else(PoisonError::into_inner);
            if vectors.is_none() {
                *vectors = Some(load(connection)?);
            }
            let loaded = vectors.as_mut().ok_or_else(|| unavailable(&"index not loaded"))?;
            work(connection, loaded)
        })
    }
}

fn delete(connection: &Connection, encoded: &str) -> Result<(), StoreError> {
    connection.execute("DELETE FROM semantic_passages WHERE location = ?1", [encoded]).map_err(|e| unavailable(&e))?;
    connection.execute("DELETE FROM semantic_items WHERE location = ?1", [encoded]).map_err(|e| unavailable(&e))?;
    Ok(())
}

fn matches(item: &IndexedItem, query: &IndexQuery) -> bool {
    item.kind == query.kind
        && item.model == query.model
        && query.category.is_none_or(|c| domain::category_matches(c, item.category))
        && query.modified.is_none_or(|range| item.fingerprint.modified.is_some_and(|m| range.contains(m)))
        && (query.within.is_empty() || query.within.iter().any(|folder| item.location.is_within(folder)))
}

fn best_passage<'a>(vector: &Embedding, passages: &'a [Stored]) -> Option<(f32, &'a Stored)> {
    passages.iter().map(|p| (dot(vector, p), p)).max_by(|a, b| a.0.total_cmp(&b.0))
}

impl SemanticIndex for SqliteStore {
    fn indexed(&self, location: &StoragePath) -> BoxFuture<'static, Result<Option<IndexedItem>, StoreError>> {
        let location = location.clone();
        self.with_vectors(move |_, vectors| Ok(vectors.entries.get(&location).map(|e| e.item.clone())))
    }

    fn upsert(&self, item: IndexedItem, passages: Vec<IndexedPassage>) -> BoxFuture<'static, Result<(), StoreError>> {
        self.with_vectors(move |connection, vectors| {
            let encoded = item.location.encode();
            let transaction = connection.unchecked_transaction().map_err(|e| unavailable(&e))?;
            delete(&transaction, &encoded)?;
            let size = item.fingerprint.size.map(|s| i64::try_from(s.0)).transpose().map_err(corrupt)?;
            transaction
                .execute(
                    "INSERT INTO semantic_items (location, kind, category, size, modified, model) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    params![
                        encoded,
                        kind_name(item.kind),
                        category_name(item.category),
                        size,
                        item.fingerprint.modified.map(Timestamp::unix_millis),
                        item.model.0
                    ],
                )
                .map_err(|e| unavailable(&e))?;
            let mut stored = Vec::with_capacity(passages.len());
            for (ordinal, passage) in passages.into_iter().enumerate() {
                let (scale, vector) = quantize(&passage.vector);
                let bytes: Vec<u8> = vector.iter().map(|v| v.to_ne_bytes()[0]).collect();
                transaction
                    .execute(
                        "INSERT INTO semantic_passages (location, ordinal, snippet, scale, vector) VALUES (?1, ?2, ?3, ?4, ?5)",
                        params![encoded, i64::try_from(ordinal).map_err(corrupt)?, passage.snippet, f64::from(scale), bytes],
                    )
                    .map_err(|e| unavailable(&e))?;
                stored.push(Stored { snippet: passage.snippet, scale, vector });
            }
            transaction.commit().map_err(|e| unavailable(&e))?;
            vectors.entries.insert(item.location.clone(), Entry { item, passages: stored });
            Ok(())
        })
    }

    fn remove(&self, locations: Vec<StoragePath>) -> BoxFuture<'static, Result<(), StoreError>> {
        self.with_vectors(move |connection, vectors| {
            let transaction = connection.unchecked_transaction().map_err(|e| unavailable(&e))?;
            for location in &locations {
                delete(&transaction, &location.encode())?;
            }
            transaction.commit().map_err(|e| unavailable(&e))?;
            for location in &locations {
                vectors.entries.remove(location);
            }
            Ok(())
        })
    }

    fn locations_within(&self, folder: &StoragePath) -> BoxFuture<'static, Result<Vec<StoragePath>, StoreError>> {
        let folder = folder.clone();
        self.with_vectors(move |_, vectors| {
            Ok(vectors.entries.keys().filter(|l| l.is_within(&folder)).cloned().collect())
        })
    }

    fn all_locations(&self) -> BoxFuture<'static, Result<Vec<StoragePath>, StoreError>> {
        self.with_vectors(|_, vectors| Ok(vectors.entries.keys().cloned().collect()))
    }

    fn search(&self, query: IndexQuery) -> BoxFuture<'static, Result<Vec<IndexHit>, StoreError>> {
        self.with_vectors(move |_, vectors| {
            let candidates = vectors.entries.values().filter(|e| !e.passages.is_empty() && matches(&e.item, &query));
            let mut hits: Vec<IndexHit> = match &query.vector {
                Some(vector) => candidates
                    .filter_map(|entry| {
                        let (similarity, passage) = best_passage(vector, &entry.passages)?;
                        Some(IndexHit { item: entry.item.clone(), similarity, snippet: Some(passage.snippet.clone()) })
                    })
                    .collect(),
                None => candidates.map(|e| IndexHit { item: e.item.clone(), similarity: 0.0, snippet: None }).collect(),
            };
            if query.vector.is_some() {
                hits.sort_by(|a, b| b.similarity.total_cmp(&a.similarity));
            } else {
                hits.sort_by_key(|hit| std::cmp::Reverse(hit.item.fingerprint.modified));
            }
            hits.truncate(query.limit);
            Ok(hits)
        })
    }

    fn count(&self) -> BoxFuture<'static, Result<u64, StoreError>> {
        self.with_vectors(|_, vectors| Ok(vectors.entries.values().filter(|e| !e.passages.is_empty()).count() as u64))
    }

    fn clear(&self) -> BoxFuture<'static, Result<(), StoreError>> {
        self.with_vectors(|connection, vectors| {
            connection
                .execute_batch("DELETE FROM semantic_passages; DELETE FROM semantic_items;")
                .map_err(|e| unavailable(&e))?;
            vectors.entries.clear();
            Ok(())
        })
    }
}

#[derive(Serialize, Deserialize)]
struct StoredSettings {
    enabled: bool,
    images: bool,
    paused: bool,
    include: String,
    exclude: String,
}

impl AiSettingsStore for SqliteStore {
    fn load(&self) -> BoxFuture<'static, Result<Option<AiSettings>, StoreError>> {
        self.run(|connection| {
            let json: Option<String> = connection
                .query_row("SELECT settings FROM ai_settings WHERE id = 1", [], |row| row.get(0))
                .optional()
                .map_err(|e| unavailable(&e))?;
            json.map(|json| {
                let stored: StoredSettings = serde_json::from_str(&json).map_err(corrupt)?;
                Ok(AiSettings {
                    enabled: stored.enabled,
                    images: stored.images,
                    paused: stored.paused,
                    scope: IndexScope {
                        include: decode_locations(&stored.include)?,
                        exclude: decode_locations(&stored.exclude)?,
                    },
                })
            })
            .transpose()
        })
    }

    fn save(&self, settings: AiSettings) -> BoxFuture<'static, Result<(), StoreError>> {
        self.run(move |connection| {
            let stored = StoredSettings {
                enabled: settings.enabled,
                images: settings.images,
                paused: settings.paused,
                include: encode_locations(&settings.scope.include)?,
                exclude: encode_locations(&settings.scope.exclude)?,
            };
            let json = serde_json::to_string(&stored).map_err(corrupt)?;
            connection
                .execute("INSERT OR REPLACE INTO ai_settings (id, settings) VALUES (1, ?1)", [json])
                .map_err(|e| unavailable(&e))
                .map(drop)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quantised_dot_products_stay_close() {
        let a: Vec<f32> = (0..384).map(|i| ((i * 37 % 101) as f32 - 50.0) / 50.0).collect();
        let norm = a.iter().map(|v| v * v).sum::<f32>().sqrt();
        let a: Vec<f32> = a.iter().map(|v| v / norm).collect();
        let (scale, vector) = quantize(&a);
        let exact: f32 = a.iter().map(|v| v * v).sum();
        let approximate = dot(&a, &Stored { snippet: String::new(), scale, vector });
        assert!((exact - approximate).abs() < 0.01, "{exact} vs {approximate}");
    }
}
