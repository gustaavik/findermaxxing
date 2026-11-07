use crate::entry::EntryKind;
use crate::error::PathError;
use crate::path::PathSegment;

/// First number used when a name is taken: `report.pdf` becomes `report 2.pdf`.
pub const FIRST_DUPLICATE_NUMBER: u32 = 2;
/// Extensions of two parts, kept together: `logs.tar.gz` → `logs 2.tar.gz`.
const COMPOUND_EXTENSIONS: &[&[u8]] = &[b".tar.gz", b".tar.bz2", b".tar.xz", b".tar.zst"];

/// Splits off a compound extension (case-insensitively), if there is one.
fn compound_split(name: &[u8]) -> Option<(&[u8], &[u8])> {
    COMPOUND_EXTENSIONS.iter().find_map(|extension| {
        let split = name.len().checked_sub(extension.len()).filter(|at| *at > 0)?;
        name[split..].eq_ignore_ascii_case(extension).then(|| (&name[..split], &name[split + 1..]))
    })
}

/// Finder/GNOME-style name for "keep both": inserts ` <n>` before the
/// extension of files; folders keep dots in their names (`v1.2 3`).
///
/// # Errors
/// Never in practice; the inputs are already valid segments.
pub fn numbered_name(original: &PathSegment, kind: EntryKind, number: u32) -> Result<PathSegment, PathError> {
    let (stem, extension) = if kind.is_container() {
        (original.as_bytes(), None)
    } else {
        compound_split(original.as_bytes()).map_or_else(|| original.stem_and_extension(), |(s, e)| (s, Some(e)))
    };
    let mut name = stem.to_vec();
    name.extend_from_slice(format!(" {number}").as_bytes());
    if let Some(extension) = extension {
        name.push(b'.');
        name.extend_from_slice(extension);
    }
    PathSegment::from_bytes(&name)
}

/// Picks the first numbered variant of `original` for which `is_taken` is false.
///
/// # Errors
/// Never in practice; see [`numbered_name`].
pub fn first_free_name(
    original: &PathSegment,
    kind: EntryKind,
    mut is_taken: impl FnMut(&PathSegment) -> bool,
) -> Result<PathSegment, PathError> {
    let mut number = FIRST_DUPLICATE_NUMBER;
    loop {
        let candidate = numbered_name(original, kind, number)?;
        if !is_taken(&candidate) {
            return Ok(candidate);
        }
        number = number.saturating_add(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn name(n: &str) -> PathSegment {
        PathSegment::new(n).unwrap()
    }

    #[test]
    fn files_number_before_the_extension() {
        assert_eq!(numbered_name(&name("report.pdf"), EntryKind::File, 2).unwrap(), name("report 2.pdf"));
        assert_eq!(numbered_name(&name("README"), EntryKind::File, 3).unwrap(), name("README 3"));
        assert_eq!(numbered_name(&name(".env"), EntryKind::File, 2).unwrap(), name(".env 2"));
        assert_eq!(numbered_name(&name("logs.TAR.GZ"), EntryKind::File, 2).unwrap(), name("logs 2.TAR.GZ"));
        assert_eq!(numbered_name(&name(".tar.gz"), EntryKind::File, 2).unwrap(), name(".tar 2.gz"));
    }

    #[test]
    fn folders_keep_dots() {
        assert_eq!(numbered_name(&name("v1.2"), EntryKind::Directory, 2).unwrap(), name("v1.2 2"));
    }

    #[test]
    fn first_free_skips_taken_names() {
        let taken = [name("a 2.txt"), name("a 3.txt")];
        let free = first_free_name(&name("a.txt"), EntryKind::File, |c| taken.contains(c)).unwrap();
        assert_eq!(free, name("a 4.txt"));
    }
}
