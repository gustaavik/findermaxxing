//! Keyboard modifiers held right now (C++ shim over `QGuiApplication`).

#[cxx::bridge]
mod ffi {
    unsafe extern "C++" {
        include!("presentation/cpp/input.h");

        #[cxx_name = "keyboardModifiers"]
        fn keyboard_modifiers() -> u32;
    }
}

/// `Qt::KeyboardModifier` bits. On macOS Qt reports ⌘ as Control and ⌥ as Alt.
const SHIFT: u32 = 0x0200_0000;
const CONTROL: u32 = 0x0400_0000;
const ALT: u32 = 0x0800_0000;

/// What the held keys ask a drop to do, by each platform's convention:
/// macOS ⌥ copies and ⌘ moves; Linux desktops copy with Ctrl and move with Shift.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DropModifier {
    None,
    ForceCopy,
    ForceMove,
}

pub(crate) fn drop_modifier() -> DropModifier {
    modifier_from_bits(ffi::keyboard_modifiers())
}

fn modifier_from_bits(bits: u32) -> DropModifier {
    let (copy, moving) = if cfg!(target_os = "macos") { (ALT, CONTROL) } else { (CONTROL, SHIFT) };
    if bits & copy != 0 {
        DropModifier::ForceCopy
    } else if bits & moving != 0 {
        DropModifier::ForceMove
    } else {
        DropModifier::None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn platform_keys_choose_the_operation() {
        assert_eq!(modifier_from_bits(0), DropModifier::None);
        let (copy, moving) = if cfg!(target_os = "macos") { (ALT, CONTROL) } else { (CONTROL, SHIFT) };
        assert_eq!(modifier_from_bits(copy), DropModifier::ForceCopy);
        assert_eq!(modifier_from_bits(moving), DropModifier::ForceMove);
    }
}
