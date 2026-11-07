//! Editing POSIX permissions on many items at once: per bit, set, clear or
//! leave as it is, so a selection with mixed modes keeps what the user didn't
//! touch.

use crate::units::UnixMode;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionClass {
    Owner,
    Group,
    Others,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Access {
    Read,
    Write,
    /// Run a file; enter (search) a folder.
    Execute,
}

impl PermissionClass {
    pub const ALL: [Self; 3] = [Self::Owner, Self::Group, Self::Others];
}

impl Access {
    pub const ALL: [Self; 3] = [Self::Read, Self::Write, Self::Execute];
}

/// The `rwx` bit for one class.
#[must_use]
pub const fn permission_bit(class: PermissionClass, access: Access) -> u32 {
    let shift = match class {
        PermissionClass::Owner => 6,
        PermissionClass::Group => 3,
        PermissionClass::Others => 0,
    };
    let bit = match access {
        Access::Read => 0o4,
        Access::Write => 0o2,
        Access::Execute => 0o1,
    };
    bit << shift
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BitChange {
    #[default]
    Keep,
    Set,
    Clear,
}

/// Which items below a folder an edit reaches.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionReach {
    /// The selected items only.
    SelectedItems,
    /// The selected items and everything inside the selected folders.
    Everything,
    /// The selected items and the files (not folders) inside them.
    AlsoFilesInside,
    /// The selected items and the folders (not files) inside them.
    AlsoFoldersInside,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PermissionEdit {
    changes: [[BitChange; 3]; 3],
}

impl PermissionEdit {
    /// An edit that results in exactly `mode` (for the octal field).
    #[must_use]
    pub fn exactly(mode: UnixMode) -> Self {
        let mut edit = Self::default();
        for class in PermissionClass::ALL {
            for access in Access::ALL {
                let wanted = mode.bits() & permission_bit(class, access) != 0;
                edit = edit.with(class, access, if wanted { BitChange::Set } else { BitChange::Clear });
            }
        }
        edit
    }

    #[must_use]
    pub fn with(mut self, class: PermissionClass, access: Access, change: BitChange) -> Self {
        self.changes[class as usize][access as usize] = change;
        self
    }

    #[must_use]
    pub fn change(&self, class: PermissionClass, access: Access) -> BitChange {
        self.changes[class as usize][access as usize]
    }

    #[must_use]
    pub fn is_noop(&self) -> bool {
        self.changes.iter().flatten().all(|c| *c == BitChange::Keep)
    }

    /// The mode after the edit. Special bits (setuid, setgid, sticky) stay.
    #[must_use]
    pub fn apply(&self, mode: UnixMode) -> UnixMode {
        let mut bits = mode.bits();
        for class in PermissionClass::ALL {
            for access in Access::ALL {
                let bit = permission_bit(class, access);
                match self.change(class, access) {
                    BitChange::Keep => {}
                    BitChange::Set => bits |= bit,
                    BitChange::Clear => bits &= !bit,
                }
            }
        }
        UnixMode::from_mode(bits)
    }
}

/// For each bit: `Some(state)` when all `modes` agree, `None` when mixed
/// (a tri-state checkbox). Empty input reads as all-clear.
#[must_use]
pub fn common_bits(modes: &[UnixMode]) -> [[Option<bool>; 3]; 3] {
    let mut common = [[Some(false); 3]; 3];
    for class in PermissionClass::ALL {
        for access in Access::ALL {
            let bit = permission_bit(class, access);
            let mut states = modes.iter().map(|m| m.bits() & bit != 0);
            common[class as usize][access as usize] = match states.next() {
                None => Some(false),
                Some(first) => states.all(|s| s == first).then_some(first),
            };
        }
    }
    common
}

/// `0o755` as `755` (with a leading special digit only when set).
#[must_use]
pub fn octal(mode: UnixMode) -> String {
    if mode.bits() > 0o777 { format!("{:04o}", mode.bits()) } else { format!("{:03o}", mode.bits()) }
}

/// `rwxr-xr-x`.
#[must_use]
pub fn symbolic(mode: UnixMode) -> String {
    let mut text = String::with_capacity(9);
    for class in PermissionClass::ALL {
        for (access, letter) in Access::ALL.into_iter().zip(['r', 'w', 'x']) {
            text.push(if mode.bits() & permission_bit(class, access) != 0 { letter } else { '-' });
        }
    }
    text
}

/// `755` or `0755`; `None` for anything else.
#[must_use]
pub fn parse_octal(text: &str) -> Option<UnixMode> {
    let text = text.trim();
    if !(3..=4).contains(&text.len()) {
        return None;
    }
    u32::from_str_radix(text, 8).ok().map(UnixMode::from_mode)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edits_touch_only_the_chosen_bits() {
        let edit = PermissionEdit::default().with(PermissionClass::Group, Access::Write, BitChange::Set).with(
            PermissionClass::Others,
            Access::Read,
            BitChange::Clear,
        );
        assert_eq!(edit.apply(UnixMode::from_mode(0o644)), UnixMode::from_mode(0o660));
        assert_eq!(edit.apply(UnixMode::from_mode(0o4755)), UnixMode::from_mode(0o4771), "setuid stays");
        assert!(PermissionEdit::default().is_noop());
    }

    #[test]
    fn exact_edits_reproduce_the_mode() {
        let edit = PermissionEdit::exactly(UnixMode::from_mode(0o750));
        assert_eq!(edit.apply(UnixMode::from_mode(0o007)), UnixMode::from_mode(0o750));
    }

    #[test]
    fn mixed_selections_show_mixed_bits() {
        let common = common_bits(&[UnixMode::from_mode(0o644), UnixMode::from_mode(0o600)]);
        assert_eq!(common[PermissionClass::Owner as usize][Access::Read as usize], Some(true));
        assert_eq!(common[PermissionClass::Group as usize][Access::Read as usize], None);
        assert_eq!(common[PermissionClass::Others as usize][Access::Write as usize], Some(false));
    }

    #[test]
    fn octal_and_symbolic_forms() {
        assert_eq!(octal(UnixMode::from_mode(0o755)), "755");
        assert_eq!(octal(UnixMode::from_mode(0o1777)), "1777");
        assert_eq!(symbolic(UnixMode::from_mode(0o754)), "rwxr-xr--");
        assert_eq!(parse_octal("0644"), Some(UnixMode::from_mode(0o644)));
        assert_eq!(parse_octal("888"), None);
        assert_eq!(parse_octal("7"), None);
    }
}
