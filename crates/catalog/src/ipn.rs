//! The IPN and the parts of it the glossary names.

use std::fmt;

/// An Internal Part Number, `CCC-NNNN-VVVV`: a three-letter IPN Family, a
/// four-digit sequence, and a four-character Variation. Variations may hold
/// letters (`REG-0006-03V3`), so they are uppercase letters or digits.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Ipn(String);

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum IpnError {
    Blank,
    Malformed(String),
}

impl fmt::Display for IpnError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Blank => formatter.write_str("the IPN is blank"),
            Self::Malformed(text) => write!(formatter, "`{text}` is not a CCC-NNNN-VVVV IPN"),
        }
    }
}

impl std::error::Error for IpnError {}

impl Ipn {
    /// Parses an IPN exactly as written: no trimming and no case folding,
    /// because an IPN is never rewritten.
    pub fn parse(text: &str) -> Result<Self, IpnError> {
        if text.is_empty() {
            return Err(IpnError::Blank);
        }
        let bytes = text.as_bytes();
        let valid = bytes.len() == 13
            && bytes[3] == b'-'
            && bytes[8] == b'-'
            && bytes[..3].iter().all(u8::is_ascii_uppercase)
            && bytes[4..8].iter().all(u8::is_ascii_digit)
            && bytes[9..]
                .iter()
                .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit());
        if valid {
            Ok(Self(text.to_owned()))
        } else {
            Err(IpnError::Malformed(text.to_owned()))
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The IPN Family, such as `RES`.
    pub fn family(&self) -> &str {
        &self.0[..3]
    }

    /// The IPN Base, such as `RES-0000`.
    pub fn base(&self) -> &str {
        &self.0[..8]
    }

    /// The Variation, such as `0470`.
    pub fn variation(&self) -> &str {
        &self.0[9..]
    }
}

impl fmt::Display for Ipn {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_family_base_and_variation() {
        let ipn = Ipn::parse("RES-0000-0470").unwrap();
        assert_eq!(
            (ipn.family(), ipn.base(), ipn.variation()),
            ("RES", "RES-0000", "0470")
        );
        assert_eq!(Ipn::parse("REG-0006-03V3").unwrap().variation(), "03V3");
    }

    #[test]
    fn rejects_anything_else() {
        assert_eq!(Ipn::parse(""), Err(IpnError::Blank));
        for bad in [
            " RES-0000-0470",
            "res-0000-0470",
            "RES-000-0470",
            "RES-0000-047",
            "RES_0000_0470",
            "RES-00A0-0470",
            "RES-0000-04v0",
        ] {
            assert!(
                matches!(Ipn::parse(bad), Err(IpnError::Malformed(_))),
                "{bad}"
            );
        }
    }
}
