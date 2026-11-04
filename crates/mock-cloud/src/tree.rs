//! An id-based file tree, the common model behind all three APIs.

use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};

pub const ROOT: &str = "root";
pub const FOLDER_MIME: &str = "application/vnd.google-apps.folder";

#[derive(Debug, Clone)]
pub struct Item {
    pub id: String,
    pub parent: String,
    pub name: String,
    pub folder: bool,
    pub content: Vec<u8>,
    pub mime: String,
    pub modified_ms: i64,
    pub trashed: bool,
    /// Creation order, for stable listings.
    pub created: u64,
}

#[derive(Debug)]
pub struct Tree {
    pub items: BTreeMap<String, Item>,
    next: u64,
}

impl Default for Tree {
    fn default() -> Self {
        Self::new()
    }
}

pub fn now_ms() -> i64 {
    let millis = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis());
    i64::try_from(millis).unwrap_or_default()
}

impl Tree {
    #[must_use]
    pub fn new() -> Self {
        let root = Item {
            id: ROOT.into(),
            parent: String::new(),
            name: String::new(),
            folder: true,
            content: Vec::new(),
            mime: FOLDER_MIME.into(),
            modified_ms: now_ms(),
            trashed: false,
            created: 0,
        };
        Self { items: BTreeMap::from([(ROOT.to_owned(), root)]), next: 1 }
    }

    pub fn get(&self, id: &str) -> Option<&Item> {
        self.items.get(id).filter(|item| !item.trashed)
    }

    pub fn get_any(&self, id: &str) -> Option<&Item> {
        self.items.get(id)
    }

    pub fn get_mut(&mut self, id: &str) -> Option<&mut Item> {
        self.items.get_mut(id)
    }

    /// Children that aren't trashed, oldest first.
    pub fn children(&self, parent: &str) -> Vec<Item> {
        let mut children: Vec<Item> =
            self.items.values().filter(|i| i.parent == parent && !i.trashed && i.id != ROOT).cloned().collect();
        children.sort_by_key(|i| i.created);
        children
    }

    pub fn child_named(&self, parent: &str, name: &str, ignore_case: bool) -> Option<Item> {
        self.children(parent)
            .into_iter()
            .find(|i| if ignore_case { i.name.eq_ignore_ascii_case(name) } else { i.name == name })
    }

    /// `/a/b` (or empty for the root) → id.
    pub fn by_path(&self, path: &str, ignore_case: bool) -> Option<String> {
        let mut id = ROOT.to_owned();
        for part in path.split('/').filter(|p| !p.is_empty()) {
            id = self.child_named(&id, part, ignore_case)?.id;
        }
        Some(id)
    }

    pub fn path_of(&self, id: &str) -> String {
        let mut parts = Vec::new();
        let mut current = id.to_owned();
        while current != ROOT {
            let Some(item) = self.items.get(&current) else { break };
            parts.push(item.name.clone());
            current = item.parent.clone();
        }
        parts.reverse();
        format!("/{}", parts.join("/"))
    }

    pub fn add(&mut self, parent: &str, name: &str, folder: bool, content: Vec<u8>, mime: &str) -> String {
        let id = format!("id{}", self.next);
        self.next += 1;
        let created = self.next;
        self.items.insert(
            id.clone(),
            Item {
                id: id.clone(),
                parent: parent.to_owned(),
                name: name.to_owned(),
                folder,
                content,
                mime: mime.to_owned(),
                modified_ms: now_ms(),
                trashed: false,
                created,
            },
        );
        id
    }

    /// Creates `/a/b/c` folders as needed (Dropbox and Graph do).
    pub fn ensure_folders(&mut self, path: &str, ignore_case: bool) -> String {
        let mut id = ROOT.to_owned();
        for part in path.split('/').filter(|p| !p.is_empty()) {
            id = match self.child_named(&id, part, ignore_case) {
                Some(item) => item.id,
                None => self.add(&id, part, true, Vec::new(), FOLDER_MIME),
            };
        }
        id
    }

    pub fn remove_tree(&mut self, id: &str) {
        let children: Vec<String> = self.items.values().filter(|i| i.parent == id).map(|i| i.id.clone()).collect();
        for child in children {
            self.remove_tree(&child);
        }
        self.items.remove(id);
    }

    pub fn copy_tree(&mut self, id: &str, parent: &str, name: &str) -> Option<String> {
        let item = self.items.get(id)?.clone();
        let copy = self.add(parent, name, item.folder, item.content.clone(), &item.mime);
        for child in self.children(id) {
            self.copy_tree(&child.id, &copy, &child.name);
        }
        Some(copy)
    }

    pub fn is_within(&self, id: &str, ancestor: &str) -> bool {
        let mut current = id.to_owned();
        loop {
            if current == ancestor {
                return true;
            }
            match self.items.get(&current) {
                Some(item) if !item.parent.is_empty() => current = item.parent.clone(),
                _ => return false,
            }
        }
    }
}

/// `2026-11-04T16:39:18.123Z`.
pub fn rfc3339(ms: i64) -> String {
    let (days, rest) = (ms.div_euclid(86_400_000), ms.rem_euclid(86_400_000));
    let (year, month, day) = civil_from_days(days);
    let (hours, minutes, seconds, millis) = (rest / 3_600_000, rest / 60_000 % 60, rest / 1000 % 60, rest % 1000);
    format!("{year:04}-{month:02}-{day:02}T{hours:02}:{minutes:02}:{seconds:02}.{millis:03}Z")
}

/// Howard Hinnant's algorithm: days since 1970-01-01 → (y, m, d).
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + i64::from(month <= 2), month, day)
}
