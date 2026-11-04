//! Implements the `TextRenderer` and `ArchiveInspector` ports. All parsing
//! runs on the runtime's blocking pool.

mod archive;
mod text;

pub use archive::LocalArchiveInspector;
pub use text::SyntectTextRenderer;
