//! Names in the KiCad libraries: each revision's entry name, and file
//! names Windows accepts.
//!
//! A revision's name is its part's IPN or its CAD item's name with `_v` and
//! the revision number, such as `RES-0000-0470_v2` or `R_0603_1608Metric_v2`.
//! The number is always added, even to the first revision, so one name
//! always means one exact revision.
//!
//! CAD item names come from files people choose in the app. The team
//! server refuses most unsafe names, but it accepts names Windows refuses
//! as file names, such as `CON`, `NUL`, `COM1` or a name ending in a dot or
//! a space. So a library's file name is encoded when Windows would refuse
//! it ([`file_stem`]); KiCad finds a library by its nickname, which keeps
//! the name. A footprint's file name is its name in KiCad, so a footprint
//! whose name Windows refuses is left out instead ([`windows_refuses`]).

/// The name of `number`, a revision of `name`: `name_vNUMBER`.
pub fn revision_name(name: &str, number: u32) -> String {
    format!("{name}_v{number}")
}

/// The names Windows keeps for devices. A file named one of them, with or
/// without an extension, cannot be made.
const DEVICE_NAMES: &[&str] = &[
    "CON", "PRN", "AUX", "NUL", "CONIN$", "CONOUT$", "COM0", "COM1", "COM2", "COM3", "COM4",
    "COM5", "COM6", "COM7", "COM8", "COM9", "COM¹", "COM²", "COM³", "LPT0", "LPT1", "LPT2", "LPT3",
    "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9", "LPT¹", "LPT²", "LPT³",
];

/// Characters Windows never accepts in a file name.
fn refused_character(ch: char) -> bool {
    ch.is_control() || matches!(ch, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*')
}

/// Whether the part of `name` before its first dot is a device name, such
/// as `CON` in `CON.kicad_sym` or `con` in `con`.
fn is_device_name(name: &str) -> bool {
    let stem = name
        .split('.')
        .next()
        .unwrap_or_default()
        .trim_end_matches(' ');
    let stem = stem.to_lowercase();
    DEVICE_NAMES
        .iter()
        .any(|device| device.to_lowercase() == stem)
}

/// Why Windows refuses `name` as a file name, or `None` if it accepts it.
/// Each reason finishes the sentence "Windows refuses this name because
/// ...".
pub fn windows_refuses(name: &str) -> Option<&'static str> {
    if name.is_empty() || name == "." || name == ".." {
        Some("it is empty or only dots")
    } else if name.chars().any(refused_character) {
        Some("it has a character Windows does not allow, such as : or ?")
    } else if name.ends_with('.') || name.ends_with(' ') {
        Some("it ends with a dot or a space")
    } else if is_device_name(name) {
        Some("Windows keeps it for a device, like CON or COM1")
    } else {
        None
    }
}

/// `name` made into a file name Windows accepts, to go before an
/// extension: unchanged when it is safe, and otherwise with each refused
/// character, the last character when it is a dot or a space, and the
/// first character of a device name written as `%` and its hex code. `%`
/// itself is always written `%25`, so two names never give the same file
/// name.
pub fn file_stem(name: &str) -> String {
    let encode = |ch: char, into: &mut String| {
        let mut buffer = [0; 4];
        for byte in ch.encode_utf8(&mut buffer).bytes() {
            into.push_str(&format!("%{byte:02X}"));
        }
    };
    let mut stem = String::with_capacity(name.len());
    let last = name.chars().count().saturating_sub(1);
    let device = is_device_name(name);
    for (index, ch) in name.chars().enumerate() {
        let trailing = index == last && (ch == '.' || ch == ' ');
        if ch == '%' || refused_character(ch) || trailing || (device && index == 0) {
            encode(ch, &mut stem);
        } else {
            stem.push(ch);
        }
    }
    if stem.is_empty() {
        stem.push_str("%00");
    }
    stem
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_revision_gets_its_number() {
        assert_eq!(revision_name("RES-0000-0470", 1), "RES-0000-0470_v1");
        assert_eq!(
            revision_name("R_0603_1608Metric", 12),
            "R_0603_1608Metric_v12"
        );
        // A name that already ends like a revision still gets one.
        assert_eq!(revision_name("Foo_v2", 1), "Foo_v2_v1");
    }

    #[test]
    fn names_windows_refuses_are_found() {
        for refused in [
            "CON",
            "con",
            "NUL",
            "nul.kicad_mod",
            "COM1",
            "com1.x_v1.kicad_mod",
            "LPT9",
            "AUX ",
            "CONIN$",
            "COM¹",
            "R_0603.",
            "R_0603 ",
            "a:b",
            "a?b",
            "a\u{1}b",
            "",
            "..",
        ] {
            assert!(windows_refuses(refused).is_some(), "{refused:?}");
        }
        for accepted in [
            "R_0603_1608Metric_v1.kicad_mod",
            "CON_v1.kicad_mod",
            "CONSOLE",
            "COM10",
            "g-res.kicad_sym",
            "R_0603 _v1",
            "x.y",
        ] {
            assert_eq!(windows_refuses(accepted), None, "{accepted:?}");
        }
    }

    #[test]
    fn library_file_names_are_encoded_only_when_needed() {
        assert_eq!(file_stem("g-res"), "g-res");
        assert_eq!(file_stem("g-Resistor_SMD"), "g-Resistor_SMD");
        assert_eq!(file_stem("CON"), "%43ON");
        assert_eq!(file_stem("nul.x"), "%6Eul.x");
        assert_eq!(file_stem("lib."), "lib%2E");
        assert_eq!(file_stem("lib "), "lib%20");
        assert_eq!(file_stem("a:b?"), "a%3Ab%3F");
        assert_eq!(file_stem("50%"), "50%25");
        assert_eq!(file_stem(""), "%00");
        for name in ["CON", "nul.x", "lib.", "lib ", "a:b?", "COM¹", "50%"] {
            let stem = file_stem(name);
            assert_eq!(windows_refuses(&format!("{stem}.pretty")), None, "{name}");
            assert_eq!(windows_refuses(&stem), None, "{name}");
        }
        // Two names never share a file name.
        assert_ne!(file_stem("%43ON"), file_stem("CON"));
    }
}
