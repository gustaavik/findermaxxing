//! The local, rule-based half of organizing (ADR 0029): better names for
//! camera files and generically named documents, photo months, groups of
//! documents that are about the same thing, and names for those groups.

use std::collections::{HashMap, HashSet};

use crate::error::PathError;
use crate::path::PathSegment;
use crate::units::CivilTime;

/// Camera and phone file names: a prefix, then digits (and `_`/`-`).
const CAMERA_PREFIXES: &[&str] =
    &["img", "dsc", "dscn", "dscf", "pxl", "mvimg", "gopr", "vid", "mov", "p10", "dji", "sam", "wp"];
/// Stems that say nothing about the content.
const GENERIC_STEMS: &[&str] = &[
    "untitled",
    "document",
    "scan",
    "scanned",
    "file",
    "image",
    "download",
    "unnamed",
    "noname",
    "new document",
    "doc",
    "page",
    "unbenannt",
    "sans titre",
    "unavngivet",
    "dokument",
];
/// Titles longer than this are cut at a word boundary.
const MAX_TITLE_CHARS: usize = 60;
const MIN_TITLE_CHARS: usize = 4;
/// Characters that can't appear in file names on any of our platforms.
const UNSAFE_NAME_CHARS: &[char] = &['/', '\\', ':', '*', '?', '"', '<', '>', '|'];
/// Documents that must share a group before it's worth a folder.
pub const MIN_GROUP_SIZE: usize = 3;
/// Photos in one month before they get their own folder.
pub const MIN_PHOTOS_PER_MONTH: usize = 8;
/// Words too common to name a group.
const STOP_WORDS: &[&str] = &[
    "the", "and", "for", "with", "from", "this", "that", "are", "was", "you", "your", "our", "not", "but", "have",
    "has", "will", "page", "date", "total", "og", "det", "der", "til", "for", "med", "som", "på", "af", "den", "er",
    "ikke", "und", "die", "das", "mit", "von", "les", "des", "pour", "una", "los", "las", "del", "pdf", "docx", "txt",
];
const MIN_TERM_CHARS: usize = 4;

fn stem_and_extension(name: &PathSegment) -> (String, Option<String>) {
    let (stem, extension) = name.stem_and_extension();
    (String::from_utf8_lossy(stem).into_owned(), extension.map(|e| String::from_utf8_lossy(e).to_lowercase()))
}

/// `IMG_1234.JPG`, `PXL_20260314_150926.jpg`, `DSC-0042.png`.
#[must_use]
pub fn is_camera_name(name: &PathSegment) -> bool {
    let (stem, _) = stem_and_extension(name);
    let lower = stem.to_lowercase();
    let Some(prefix) = CAMERA_PREFIXES.iter().find(|p| lower.starts_with(*p)) else { return false };
    let rest = &lower[prefix.len()..];
    !rest.is_empty()
        && rest.chars().all(|c| c.is_ascii_digit() || c == '_' || c == '-')
        && rest.chars().any(|c| c.is_ascii_digit())
}

/// `Untitled.pdf`, `document (3).docx`, `scan0001.pdf`.
#[must_use]
pub fn is_generic_name(name: &PathSegment) -> bool {
    let (stem, _) = stem_and_extension(name);
    let lower = stem.to_lowercase();
    let core = lower.trim_end_matches(|c: char| c.is_ascii_digit() || " _-()".contains(c));
    GENERIC_STEMS.contains(&core.trim())
}

/// `2026-03-14 15.09.26.jpg`: sorts by time, readable, valid everywhere.
///
/// # Errors
/// Never in practice; the name is built from digits and the extension.
pub fn dated_name(original: &PathSegment, taken: CivilTime) -> Result<PathSegment, PathError> {
    let (_, extension) = stem_and_extension(original);
    let base = format!(
        "{:04}-{:02}-{:02} {:02}.{:02}.{:02}",
        taken.year, taken.month, taken.day, taken.hour, taken.minute, taken.second
    );
    PathSegment::new(&extension.map_or_else(|| base.clone(), |e| format!("{base}.{e}")))
}

/// EXIF's `2026:03:14 15:09:26` (also with `-` in the date, as readers print it).
#[must_use]
pub fn parse_exif_time(value: &str) -> Option<CivilTime> {
    let value = value.trim();
    let (date, time) = value.split_once(' ')?;
    let date: Vec<i64> = date.split([':', '-']).map(|p| p.parse().ok()).collect::<Option<_>>()?;
    let time: Vec<i64> = time.split(':').take(3).map(|p| p.trim().parse().ok()).collect::<Option<_>>()?;
    match (date.as_slice(), time.as_slice()) {
        ([year, month, day], [hour, minute, second])
            if (1..=12).contains(month) && (1..=31).contains(day) && *year > 1900 =>
        {
            Some(CivilTime { year: *year, month: *month, day: *day, hour: *hour, minute: *minute, second: *second })
        }
        _ => None,
    }
}

/// A name from a document's first meaningful line, keeping the extension.
/// `None` when the text has no usable line.
#[must_use]
pub fn titled_name(original: &PathSegment, text: &str) -> Option<PathSegment> {
    let (_, extension) = stem_and_extension(original);
    let line =
        text.lines().map(str::trim).find(|l| l.chars().filter(|c| c.is_alphabetic()).count() >= MIN_TITLE_CHARS)?;
    let clean: String =
        line.chars().map(|c| if UNSAFE_NAME_CHARS.contains(&c) || c.is_control() { ' ' } else { c }).collect();
    let words: Vec<&str> = clean.split_whitespace().collect();
    let mut title = String::new();
    for word in words {
        if title.chars().count() + word.chars().count() + 1 > MAX_TITLE_CHARS {
            break;
        }
        if !title.is_empty() {
            title.push(' ');
        }
        title.push_str(word);
    }
    let title = title.trim_matches(|c: char| c == '.' || c.is_whitespace());
    if title.chars().count() < MIN_TITLE_CHARS {
        return None;
    }
    PathSegment::new(&extension.map_or_else(|| title.to_owned(), |e| format!("{title}.{e}"))).ok()
}

/// `Photos 2026-03`.
///
/// # Errors
/// Never in practice.
pub fn photo_month_folder(year: i64, month: i64) -> Result<PathSegment, PathError> {
    PathSegment::new(&format!("Photos {year:04}-{month:02}"))
}

fn cosine(a: &[f32], b: &[f32]) -> f32 {
    let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let norm = |v: &[f32]| v.iter().map(|x| x * x).sum::<f32>().sqrt();
    let denominator = norm(a) * norm(b);
    if denominator > 0.0 { dot / denominator } else { 0.0 }
}

/// Groups vectors by average-linkage agglomerative clustering: the two
/// groups most alike merge while their average similarity is at least
/// `threshold`. Returns groups of at least [`MIN_GROUP_SIZE`] indices.
#[must_use]
pub fn cluster(vectors: &[Vec<f32>], threshold: f32) -> Vec<Vec<usize>> {
    let n = vectors.len();
    let similarity: Vec<Vec<f32>> =
        (0..n).map(|i| (0..n).map(|j| cosine(&vectors[i], &vectors[j])).collect()).collect();
    let mut groups: Vec<Vec<usize>> = (0..n).map(|i| vec![i]).collect();
    loop {
        let mut best: Option<(usize, usize, f32)> = None;
        for a in 0..groups.len() {
            for b in (a + 1)..groups.len() {
                let pairs = groups[a].len() * groups[b].len();
                #[allow(clippy::cast_precision_loss, reason = "group sizes are small")]
                let total: f32 =
                    groups[a].iter().map(|i| groups[b].iter().map(|j| similarity[*i][*j]).sum::<f32>()).sum();
                let average = total / pairs as f32;
                if average >= threshold && best.is_none_or(|(_, _, s)| average > s) {
                    best = Some((a, b, average));
                }
            }
        }
        let Some((a, b, _)) = best else { break };
        let merged = groups.remove(b);
        groups[a].extend(merged);
    }
    groups
        .into_iter()
        .filter(|g| g.len() >= MIN_GROUP_SIZE)
        .map(|mut g| {
            g.sort_unstable();
            g
        })
        .collect()
}

fn terms(text: &str) -> HashSet<String> {
    let stop: HashSet<&str> = STOP_WORDS.iter().copied().collect();
    text.to_lowercase()
        .split(|c: char| !c.is_alphabetic())
        .filter(|w| w.chars().count() >= MIN_TERM_CHARS && !stop.contains(w))
        .map(str::to_owned)
        .collect()
}

/// A folder name for a group: the word found in most of the group's texts
/// and fewest of the others' (document frequency in, minus out), capitalised.
#[must_use]
pub fn group_label(group_texts: &[&str], other_texts: &[&str]) -> Option<PathSegment> {
    // Per term: documents containing it, and occurrences in all of them.
    let mut inside: HashMap<String, (usize, usize)> = HashMap::new();
    for text in group_texts {
        for term in terms(text) {
            inside.entry(term).or_default().0 += 1;
        }
        let lower = text.to_lowercase();
        for word in lower.split(|c: char| !c.is_alphabetic()) {
            if let Some(counts) = inside.get_mut(word) {
                counts.1 += 1;
            }
        }
    }
    let outside: Vec<HashSet<String>> = other_texts.iter().map(|t| terms(t)).collect();
    #[allow(clippy::cast_precision_loss, reason = "counts of a few hundred documents")]
    let score = |term: &str, count: usize| {
        let out = outside.iter().filter(|o| o.contains(term)).count();
        count as f32 / group_texts.len().max(1) as f32 - out as f32 / outside.len().max(1) as f32
    };
    let (term, _, _) = inside
        .iter()
        .filter(|(_, (documents, _))| *documents * 2 >= group_texts.len())
        .map(|(term, (documents, occurrences))| (term, score(term, *documents), *occurrences))
        // Most distinctive; then the most used; then alphabetical, so the result is stable.
        .max_by(|a, b| a.1.total_cmp(&b.1).then_with(|| a.2.cmp(&b.2)).then_with(|| b.0.cmp(a.0)))?;
    let mut chars = term.chars();
    let label: String = chars.next().map(|c| c.to_uppercase().chain(chars).collect()).unwrap_or_default();
    PathSegment::new(&label).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seg(name: &str) -> PathSegment {
        PathSegment::new(name).unwrap()
    }

    #[test]
    fn camera_and_generic_names() {
        assert!(is_camera_name(&seg("IMG_1234.JPG")));
        assert!(is_camera_name(&seg("PXL_20260314_150926123.jpg")));
        assert!(!is_camera_name(&seg("Imagine.jpg")));
        assert!(!is_camera_name(&seg("IMG_beach.jpg")));
        assert!(is_generic_name(&seg("Untitled.pdf")));
        assert!(is_generic_name(&seg("document (3).docx")));
        assert!(is_generic_name(&seg("scan0001.pdf")));
        assert!(!is_generic_name(&seg("Lease agreement.pdf")));
    }

    #[test]
    fn dated_names_come_from_exif_times() {
        let taken = parse_exif_time("2026:03:14 15:09:26").unwrap();
        assert_eq!(dated_name(&seg("IMG_1234.JPG"), taken).unwrap(), seg("2026-03-14 15.09.26.jpg"));
        assert_eq!(parse_exif_time("2026-03-14 15:09:26"), Some(taken));
        assert_eq!(parse_exif_time("0000:00:00 00:00:00"), None);
    }

    #[test]
    fn titles_come_from_the_first_meaningful_line() {
        let text = "\n  12/04\nLease agreement: Nørrebrogade 12, 2200 København N\nParties…";
        assert_eq!(
            titled_name(&seg("scan0001.pdf"), text).unwrap(),
            seg("Lease agreement Nørrebrogade 12, 2200 København N.pdf")
        );
        assert_eq!(titled_name(&seg("scan.pdf"), "  \n 123 \n"), None);
        let long = "word ".repeat(40);
        assert!(titled_name(&seg("a.txt"), &long).unwrap().display_name().chars().count() <= MAX_TITLE_CHARS + 4);
    }

    #[test]
    fn clustering_groups_close_vectors_only() {
        let v = |x: f32, y: f32| vec![x, y, 0.0];
        let vectors = vec![v(1.0, 0.0), v(0.98, 0.05), v(0.97, 0.1), v(0.0, 1.0), v(0.05, 0.99), v(0.6, 0.6)];
        assert_eq!(cluster(&vectors, 0.95), [vec![0, 1, 2]], "the second pair is too small to be a group");
    }

    #[test]
    fn groups_are_named_by_their_distinctive_word() {
        let group = ["Electricity invoice March", "Invoice for electricity, April", "Your electricity invoice"];
        let others = ["Holiday photos invoice", "Recipe for lasagne"];
        assert_eq!(group_label(&group, &others), Some(seg("Electricity")));
    }
}
