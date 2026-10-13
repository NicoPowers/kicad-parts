//! KiCad's two footprint filters for a symbol, as its footprint chooser
//! applies them (`footprint_filter.cpp` in KiCad):
//!
//! - the symbol's footprint filters (`ki_fp_filters`): space-separated
//!   patterns where `*` matches any run of characters and `?` any one
//!   character. A pattern matches the whole footprint name, or the whole
//!   `library:name` when the pattern has a colon, ignoring case. A footprint
//!   passes when any pattern matches; a symbol without filters lets every
//!   footprint pass.
//! - the symbol's pin count: the footprint passes when its number of
//!   distinct numbered pads equals the symbol's number of distinct pin
//!   numbers.

/// Whether the footprint `library:name` passes the symbol's footprint
/// `filters` (already split at white space).
pub fn matches_footprint_filters(filters: &[String], library: &str, name: &str) -> bool {
    if filters.is_empty() {
        return true;
    }
    let name = name.to_lowercase();
    let full = format!("{}:{name}", library.to_lowercase());
    filters.iter().any(|filter| {
        let pattern = filter.to_lowercase();
        let subject = if pattern.contains(':') { &full } else { &name };
        wildcard_match(pattern.as_bytes(), subject.as_bytes())
    })
}

/// Whether a footprint with `pad_count` distinct numbered pads fits a
/// symbol with `pin_count` distinct pin numbers.
pub fn matches_pin_count(pin_count: u32, pad_count: u32) -> bool {
    pin_count == pad_count
}

/// `*` and `?` wildcards anchored at both ends, over bytes (a `?` matches
/// one byte, so it matches ASCII characters exactly as KiCad does).
fn wildcard_match(pattern: &[u8], text: &[u8]) -> bool {
    let (mut p, mut t) = (0, 0);
    let mut star: Option<(usize, usize)> = None;
    while t < text.len() {
        match pattern.get(p) {
            Some(b'*') => {
                star = Some((p, t));
                p += 1;
            }
            Some(&c) if c == b'?' || c == text[t] => {
                p += 1;
                t += 1;
            }
            _ => match star {
                Some((star_p, star_t)) => {
                    p = star_p + 1;
                    t = star_t + 1;
                    star = Some((star_p, star_t + 1));
                }
                None => return false,
            },
        }
    }
    pattern[p..].iter().all(|&c| c == b'*')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn filters(text: &str) -> Vec<String> {
        text.split_whitespace().map(str::to_owned).collect()
    }

    #[test]
    fn matches_like_kicads_footprint_chooser() {
        // Device:R's filters.
        let r = filters("R_*");
        assert!(matches_footprint_filters(
            &r,
            "Resistor_SMD",
            "R_0603_1608Metric"
        ));
        assert!(!matches_footprint_filters(
            &r,
            "Capacitor_SMD",
            "C_0603_1608Metric"
        ));
        // Whole-name match: R_* does not match a name with R_ inside.
        assert!(!matches_footprint_filters(&r, "Resistor_SMD", "XR_0603"));
        // Case is ignored.
        assert!(matches_footprint_filters(&r, "x", "r_0402"));
        // A colon matches library:name.
        let lib = filters("Package_SO:SOIC-8*");
        assert!(matches_footprint_filters(
            &lib,
            "Package_SO",
            "SOIC-8_3.9x4.9mm_P1.27mm"
        ));
        assert!(!matches_footprint_filters(
            &lib,
            "Other",
            "SOIC-8_3.9x4.9mm_P1.27mm"
        ));
        // ? is one character; any of several patterns passes.
        let many = filters("SOT?23* TO?92*");
        assert!(matches_footprint_filters(
            &many,
            "Package_TO_SOT_SMD",
            "SOT-23"
        ));
        assert!(matches_footprint_filters(
            &many,
            "Package_TO_SOT_THT",
            "TO-92_Inline"
        ));
        assert!(!matches_footprint_filters(&many, "x", "SOT--23"));
        // No filters: everything passes.
        assert!(matches_footprint_filters(&[], "x", "anything"));
        assert!(wildcard_match(b"*", b""));
        assert!(wildcard_match(b"a*b*c", b"axxbyyc"));
        assert!(!wildcard_match(b"a*b*c", b"axxbyy"));
        assert!(!wildcard_match(b"abc", b"abcd"));
    }

    #[test]
    fn pin_count_must_equal_the_pad_count() {
        assert!(matches_pin_count(2, 2));
        assert!(!matches_pin_count(8, 9));
    }
}
