//! Tags (Finder tags on macOS, `user.xdg.tags` on Linux) and the keyword
//! rules that suggest them from a document's text.

use std::fmt;

/// Longest tag accepted; Finder truncates long tags in its sidebar anyway.
const MAX_TAG_CHARS: usize = 64;
/// Separator of `user.xdg.tags`, so it can't appear in a tag.
const XDG_SEPARATOR: char = ',';

/// A tag name: trimmed, non-empty, one line, no commas.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Tag(String);

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum TagError {
    #[error("a tag can't be empty")]
    Empty,
    #[error("a tag can't be longer than {MAX_TAG_CHARS} characters")]
    TooLong,
    #[error("a tag can't contain line breaks or commas")]
    InvalidCharacter,
}

impl Tag {
    /// # Errors
    /// [`TagError`] when the name isn't a valid tag.
    pub fn new(name: &str) -> Result<Self, TagError> {
        let name = name.trim();
        if name.is_empty() {
            return Err(TagError::Empty);
        }
        if name.chars().count() > MAX_TAG_CHARS {
            return Err(TagError::TooLong);
        }
        if name.chars().any(|c| c == XDG_SEPARATOR || c.is_control()) {
            return Err(TagError::InvalidCharacter);
        }
        Ok(Self(name.to_owned()))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Tag {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// `existing` plus `added`, without duplicates (case-insensitive), in order.
#[must_use]
pub fn merged_tags(existing: &[Tag], added: &[Tag]) -> Vec<Tag> {
    let mut merged = existing.to_vec();
    for tag in added {
        if !merged.iter().any(|t| t.0.eq_ignore_ascii_case(&tag.0)) {
            merged.push(tag.clone());
        }
    }
    merged
}

/// Tags suggested by words in a document, in English, Danish, German,
/// French and Spanish. Whole words only; phrases match as written.
const KEYWORD_TAGS: &[(&str, &[&str])] = &[
    ("Invoice", &["invoice", "faktura", "rechnung", "facture", "factura"]),
    ("Receipt", &["receipt", "kvittering", "quittung", "reçu", "recibo"]),
    ("Contract", &["contract", "agreement", "kontrakt", "aftale", "vertrag", "contrat", "contrato"]),
    ("Tax", &["tax return", "årsopgørelse", "selvangivelse", "steuererklärung", "impôt", "impuesto"]),
    ("Payslip", &["payslip", "pay slip", "lønseddel", "gehaltsabrechnung", "bulletin de paie", "nómina"]),
    ("CV", &["curriculum vitae", "résumé", "resume", "lebenslauf"]),
    ("Ticket", &["boarding pass", "e-ticket", "boardingkort", "bordkarte", "carte d'embarquement"]),
];

/// The tags whose keywords appear in `text`.
#[must_use]
pub fn tags_for_text(text: &str) -> Vec<Tag> {
    let lower = text.to_lowercase();
    let words: Vec<&str> = lower.split(|c: char| !c.is_alphanumeric() && c != '-' && c != '\'').collect();
    KEYWORD_TAGS
        .iter()
        .filter(|(_, keywords)| {
            keywords.iter().any(
                |keyword| {
                    if keyword.contains(' ') { lower.contains(keyword) } else { words.contains(keyword) }
                },
            )
        })
        .filter_map(|(tag, _)| Tag::new(tag).ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tags_are_validated() {
        assert_eq!(Tag::new("  Work ").unwrap().as_str(), "Work");
        assert_eq!(Tag::new(""), Err(TagError::Empty));
        assert_eq!(Tag::new("a,b"), Err(TagError::InvalidCharacter));
        assert_eq!(Tag::new(&"x".repeat(65)), Err(TagError::TooLong));
    }

    #[test]
    fn keywords_suggest_tags_in_several_languages() {
        let tags = |text: &str| tags_for_text(text).iter().map(|t| t.as_str().to_owned()).collect::<Vec<_>>();
        assert_eq!(tags("FAKTURA nr. 1234 — beløb 1200 kr"), ["Invoice"]);
        assert_eq!(tags("Ihre Rechnung und Quittung"), ["Invoice", "Receipt"]);
        assert_eq!(tags("Boarding pass for SK 123"), ["Ticket"]);
        assert!(tags("invoiced customers").is_empty(), "whole words only");
    }

    #[test]
    fn merging_skips_duplicates() {
        let t = |s: &str| Tag::new(s).unwrap();
        assert_eq!(merged_tags(&[t("Work")], &[t("work"), t("Tax")]), [t("Work"), t("Tax")]);
    }
}
