//! `s3://` provider. Buckets are mounts (`s3://<account>@<bucket>/…`);
//! "folders" are key prefixes, shown as directories.
//!
//! Implements `Lister`, `Copier` (server-side), `Mover` (copy + delete),
//! `Remover`, `FolderMaker` (zero-byte `prefix/` markers), `Reader`,
//! `Writer` (multipart, parallel parts), `Materializer` (download cache),
//! `MetadataEditor`, `Presigner` and `VersionLister`; plus the
//! `S3Connector` port. No `Watcher`, `Trasher` or `Launcher`: S3 has none.
//! Proven by `tests/s3_contract.rs` against a real S3-compatible server.

mod bucket;
mod errors;
mod keys;
mod service;
mod writer;

pub use bucket::BucketFs;
pub use service::S3Service;
