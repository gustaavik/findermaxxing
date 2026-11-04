use std::fs::File;
use std::io::Read;
use std::path::Path;
use std::sync::{Arc, OnceLock};

use application::ports::{LocalFile, PreviewFailure, PreviewTheme, RenderedText, TextRenderer};
use futures::FutureExt;
use futures::future::BoxFuture;
use syntect::highlighting::{Theme, ThemeSet};
use syntect::html::highlighted_html_for_string;
use syntect::parsing::{SyntaxReference, SyntaxSet};
use tokio::runtime::Handle;

/// Bytes read from the start of a file. Previews are a glance, not an editor.
const READ_LIMIT: u64 = 256 * 1024;
/// Highlighting cost grows with lines; beyond this the rest is cut.
const LINE_LIMIT: usize = 2000;
/// A NUL byte in this prefix marks a file as binary.
const BINARY_SNIFF: usize = 8 * 1024;
const LIGHT_THEME: &str = "InspiredGitHub";
const DARK_THEME: &str = "base16-ocean.dark";

struct Assets {
    syntaxes: SyntaxSet,
    themes: ThemeSet,
}

/// Loads syntect's bundled grammars and themes on first use (tens of ms),
/// never at start-up.
#[derive(Clone)]
pub struct SyntectTextRenderer {
    runtime: Handle,
    assets: Arc<OnceLock<Assets>>,
}

impl SyntectTextRenderer {
    #[must_use]
    pub fn new(runtime: Handle) -> Self {
        Self { runtime, assets: Arc::default() }
    }
}

impl std::fmt::Debug for SyntectTextRenderer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SyntectTextRenderer").finish_non_exhaustive()
    }
}

impl TextRenderer for SyntectTextRenderer {
    fn render(
        &self,
        file: &LocalFile,
        theme: PreviewTheme,
    ) -> BoxFuture<'static, Result<Option<RenderedText>, PreviewFailure>> {
        let (path, assets) = (file.path().to_path_buf(), Arc::clone(&self.assets));
        let task = self.runtime.spawn_blocking(move || {
            let assets = assets.get_or_init(|| Assets {
                syntaxes: SyntaxSet::load_defaults_newlines(),
                themes: ThemeSet::load_defaults(),
            });
            render_blocking(&path, theme, assets)
        });
        async move { task.await.map_err(|e| failure(&e))? }.boxed()
    }
}

fn render_blocking(path: &Path, theme: PreviewTheme, assets: &Assets) -> Result<Option<RenderedText>, PreviewFailure> {
    let (bytes, mut truncated) = read_prefix(path)?;
    if bytes.iter().take(BINARY_SNIFF).any(|&b| b == 0) {
        return Ok(None);
    }
    let mut text = String::from_utf8_lossy(&bytes).into_owned();
    if let Some((cut, _)) = text.match_indices('\n').nth(LINE_LIMIT) {
        text.truncate(cut);
        truncated = true;
    }
    let syntax = syntax_for(path, &text, &assets.syntaxes);
    let html = highlighted_html_for_string(&text, &assets.syntaxes, syntax, pick_theme(theme, &assets.themes)?)
        .map_err(|e| failure(&e))?;
    Ok(Some(RenderedText { html, language: syntax.name.clone(), truncated }))
}

fn read_prefix(path: &Path) -> Result<(Vec<u8>, bool), PreviewFailure> {
    let file = File::open(path).map_err(|e| failure(&e))?;
    let size = file.metadata().map_err(|e| failure(&e))?.len();
    let mut bytes = Vec::new();
    file.take(READ_LIMIT).read_to_end(&mut bytes).map_err(|e| failure(&e))?;
    Ok((bytes, size > READ_LIMIT))
}

fn syntax_for<'a>(path: &Path, text: &str, syntaxes: &'a SyntaxSet) -> &'a SyntaxReference {
    let by_extension = path.extension().and_then(|e| e.to_str()).and_then(|e| syntaxes.find_syntax_by_extension(e));
    let by_name = path.file_name().and_then(|n| n.to_str()).and_then(|n| syntaxes.find_syntax_by_extension(n));
    by_extension
        .or(by_name)
        .or_else(|| syntaxes.find_syntax_by_first_line(text))
        .unwrap_or_else(|| syntaxes.find_syntax_plain_text())
}

fn pick_theme(theme: PreviewTheme, themes: &ThemeSet) -> Result<&Theme, PreviewFailure> {
    let name = match theme {
        PreviewTheme::Light => LIGHT_THEME,
        PreviewTheme::Dark => DARK_THEME,
    };
    themes.themes.get(name).ok_or_else(|| PreviewFailure { message: format!("missing theme {name}") })
}

fn failure(error: &dyn std::fmt::Display) -> PreviewFailure {
    PreviewFailure { message: error.to_string() }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn render(name: &str, contents: &[u8]) -> Option<RenderedText> {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(name);
        std::fs::write(&path, contents).unwrap();
        let assets = Assets { syntaxes: SyntaxSet::load_defaults_newlines(), themes: ThemeSet::load_defaults() };
        render_blocking(&path, PreviewTheme::Dark, &assets).unwrap()
    }

    #[test]
    fn highlights_known_languages() {
        let rendered = render("main.rs", b"fn main() { let x = 1; }\n").unwrap();
        assert_eq!(rendered.language, "Rust");
        assert!(rendered.html.contains("<span style=\"color:"));
        assert!(!rendered.truncated);
    }

    #[test]
    fn binary_files_are_not_text() {
        assert_eq!(render("blob", &[0x7f, b'E', b'L', b'F', 0, 0, 1]), None);
    }

    #[test]
    fn long_files_are_cut() {
        let long = "line\n".repeat(LINE_LIMIT + 10);
        let rendered = render("big.txt", long.as_bytes()).unwrap();
        assert!(rendered.truncated);
        assert_eq!(rendered.language, "Plain Text");
    }
}
