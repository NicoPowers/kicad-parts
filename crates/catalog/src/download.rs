//! A part's CAD files as an engineer downloads them from SnapEDA, Ultra
//! Librarian, SamacSys (Component Search Engine) or a manufacturer's site,
//! for Add part (kicad-parts-9693).
//!
//! A download is a ZIP file or loose files. [`read_download`] keeps the
//! KiCad symbol libraries (`.kicad_sym`), footprints (`.kicad_mod`) and 3D
//! models (`.step`, `.stp`, `.wrl`) and leaves out everything else, such as
//! files for other CAD tools and ZIP files inside the ZIP. ZIP files are
//! read in memory and never extracted: an entry's name is only a label, so
//! a name such as `../x.kicad_mod` or `C:\x.step` can never reach a folder.
//! A download over [`LIMITS`] is refused with a plain message.
//!
//! The rest of the module reads what the files say: the symbol's fields
//! ([`manufacturer_part`], [`fill_fields`]), the pin and pad counts
//! ([`pin_count`], [`pad_count`]), whether two footprints have the same pads
//! ([`pad_signature`]), and links a footprint to its 3D model
//! ([`link_model`]). [`next_free_ipn`] offers a new part its IPN.

use kicad_integration::library_file::{footprint_pads, symbol_pin_numbers, symbol_properties};
use std::collections::BTreeSet;
use std::io::{Cursor, Read};

/// How much a download may hold.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Limits {
    /// The most entries one ZIP file may have.
    pub entries: usize,
    /// The most bytes the download may hold in total, uncompressed.
    pub total_bytes: u64,
}

/// The limits the app uses: 200 entries and 100 MB uncompressed in total.
pub const LIMITS: Limits = Limits {
    entries: 200,
    total_bytes: 100 * 1024 * 1024,
};

/// What a kept file is.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DownloadKind {
    Symbol,
    Footprint,
    Model3d,
}

impl DownloadKind {
    /// The kind a file name says, by its extension in any case.
    pub fn of(name: &str) -> Option<Self> {
        let lower = name.to_lowercase();
        if lower.ends_with(".kicad_sym") {
            Some(Self::Symbol)
        } else if lower.ends_with(".kicad_mod") {
            Some(Self::Footprint)
        } else if [".step", ".stp", ".wrl"]
            .iter()
            .any(|extension| lower.ends_with(extension))
        {
            Some(Self::Model3d)
        } else {
            None
        }
    }
}

/// One kept file of a download.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DownloadFile {
    /// What to call it: the file's name, or for a ZIP entry its name in the
    /// ZIP and the ZIP's name, such as `KiCad/LM317.kicad_sym in LM317.zip`.
    pub label: String,
    /// The last part of its name, such as `LM317.kicad_sym`.
    pub file_name: String,
    pub kind: DownloadKind,
    pub bytes: Vec<u8>,
}

/// The site a ZIP file's layout says it came from.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Vendor {
    /// SamacSys, also known as Component Search Engine: a `KiCad/` folder.
    SamacSys,
    /// Ultra Librarian: a `KiCAD/` folder.
    UltraLibrarian,
    /// SnapEDA: its name in a file, or its how-to-import page.
    SnapEda,
}

impl Vendor {
    pub fn name(self) -> &'static str {
        match self {
            Self::SamacSys => "SamacSys (Component Search Engine)",
            Self::UltraLibrarian => "Ultra Librarian",
            Self::SnapEda => "SnapEDA",
        }
    }
}

/// What a download holds.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Download {
    /// The kept files, in the order given and, within a ZIP, in its order.
    pub files: Vec<DownloadFile>,
    /// Where the files came from, when a ZIP's layout says.
    pub vendor: Option<Vendor>,
    /// How many files were left out: other CAD tools' files, ZIP files
    /// inside a ZIP, and anything else.
    pub ignored: usize,
}

impl Download {
    /// The kept files of `kind`.
    pub fn of_kind(&self, kind: DownloadKind) -> impl Iterator<Item = &DownloadFile> {
        self.files.iter().filter(move |file| file.kind == kind)
    }
}

/// Reads a download: each input is a file's name and bytes, a ZIP file or
/// a loose file. See the module documentation.
pub fn read_download(inputs: &[(String, Vec<u8>)]) -> Result<Download, String> {
    read_download_with(inputs, &LIMITS)
}

/// [`read_download`] with other limits.
pub fn read_download_with(
    inputs: &[(String, Vec<u8>)],
    limits: &Limits,
) -> Result<Download, String> {
    let mut download = Download::default();
    let mut total: u64 = 0;
    let too_big = || {
        format!(
            "The download holds more than {} MB of files, which is too much for one part. Choose only the files for this part.",
            limits.total_bytes / (1024 * 1024)
        )
    };
    for (name, bytes) in inputs {
        if is_zip(name, bytes) {
            read_zip(name, bytes, limits, &mut total, &mut download)?;
            continue;
        }
        match DownloadKind::of(name) {
            Some(kind) => {
                total += bytes.len() as u64;
                if total > limits.total_bytes {
                    return Err(too_big());
                }
                download.files.push(DownloadFile {
                    label: last_part(name).to_owned(),
                    file_name: last_part(name).to_owned(),
                    kind,
                    bytes: bytes.clone(),
                });
            }
            None => download.ignored += 1,
        }
    }
    Ok(download)
}

/// A ZIP file by its name or its first bytes.
fn is_zip(name: &str, bytes: &[u8]) -> bool {
    name.to_lowercase().ends_with(".zip") || bytes.starts_with(b"PK\x03\x04")
}

/// The part of `name` after its last `/` or `\`.
fn last_part(name: &str) -> &str {
    name.rsplit(['/', '\\']).next().unwrap_or(name)
}

fn read_zip(
    zip_name: &str,
    bytes: &[u8],
    limits: &Limits,
    total: &mut u64,
    download: &mut Download,
) -> Result<(), String> {
    let shown = last_part(zip_name);
    let unreadable = |why: String| format!("{shown} could not be read as a ZIP file: {why}.");
    let mut archive =
        zip::ZipArchive::new(Cursor::new(bytes)).map_err(|error| unreadable(error.to_string()))?;
    if archive.len() > limits.entries {
        return Err(format!(
            "{shown} holds {} files; a download for one part can hold at most {}.",
            archive.len(),
            limits.entries
        ));
    }
    let too_big = || {
        format!(
            "{shown} holds more than {} MB of files once unpacked, which is too much for one part.",
            limits.total_bytes / (1024 * 1024)
        )
    };
    // What the entries say they hold, before reading any of them.
    let mut declared: u64 = 0;
    for index in 0..archive.len() {
        let entry = archive
            .by_index_raw(index)
            .map_err(|error| unreadable(error.to_string()))?;
        declared = declared.saturating_add(entry.size());
    }
    if total.saturating_add(declared) > limits.total_bytes {
        return Err(too_big());
    }
    for index in 0..archive.len() {
        // Name, kind and flags first, without unpacking: a file the app
        // leaves out may be protected or packed in a way it cannot read,
        // and that must not stop it.
        let (name, is_dir, encrypted) = {
            let raw = archive
                .by_index_raw(index)
                .map_err(|error| unreadable(error.to_string()))?;
            (raw.name().to_owned(), raw.is_dir(), raw.encrypted())
        };
        if is_dir {
            continue;
        }
        // Only a label: never joined to a folder.
        if download.vendor.is_none() {
            download.vendor = vendor_of(&name);
        }
        let file_name = last_part(&name).to_owned();
        let Some(kind) = DownloadKind::of(&file_name) else {
            download.ignored += 1;
            continue;
        };
        if encrypted {
            return Err(format!(
                "{name} in {shown} is protected by a password, so it cannot be read."
            ));
        }
        let mut entry = archive.by_index(index).map_err(|error| {
            format!("{name} in {shown} is packed in a way this app cannot read ({error}). Unpack the ZIP file and choose the KiCad files instead.")
        })?;
        // An entry may hold more than it says; read at most what is left.
        let left = limits.total_bytes - *total;
        let mut kept = Vec::new();
        (&mut entry)
            .take(left + 1)
            .read_to_end(&mut kept)
            .map_err(|error| unreadable(format!("{name}: {error}")))?;
        if kept.len() as u64 > left {
            return Err(too_big());
        }
        *total += kept.len() as u64;
        download.files.push(DownloadFile {
            label: format!("{name} in {shown}"),
            file_name,
            kind,
            bytes: kept,
        });
    }
    Ok(())
}

/// The site an entry's name says the ZIP came from.
fn vendor_of(name: &str) -> Option<Vendor> {
    let folders: Vec<&str> = name.split(['/', '\\']).collect();
    let folders = &folders[..folders.len().saturating_sub(1)];
    if folders.contains(&"KiCad") {
        return Some(Vendor::SamacSys);
    }
    if folders.contains(&"KiCAD") {
        return Some(Vendor::UltraLibrarian);
    }
    let lower = name.to_lowercase();
    (lower.contains("snapeda") || lower.contains("how-to-import")).then_some(Vendor::SnapEda)
}

// ---- What the symbol says ---------------------------------------------------

/// A symbol property name without case, spaces or punctuation, so
/// `Manufacturer_Part_Number` and `Mfr Part Number` compare simply.
fn plain(name: &str) -> String {
    name.chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

/// Property names vendors use for the MPN, as [`plain`] gives them.
const MPN_NAMES: [&str; 6] = [
    "mpn",
    "manufacturerpartnumber",
    "mfrpartnumber",
    "mfrpn",
    "mp",
    "partnumber",
];

/// Property names vendors use for the manufacturer.
const MANUFACTURER_NAMES: [&str; 5] = ["manufacturer", "manufacturername", "mfr", "mf", "mfg"];

/// Property names for the description, after the field's own name.
const DESCRIPTION_NAMES: [&str; 2] = ["description", "kidescription"];

/// Property names for keywords.
const KEYWORD_NAMES: [&str; 2] = ["kikeywords", "keywords"];

/// A symbol property or a field: its name and its value.
pub type Property = (String, String);

/// The properties of one symbol entry, with values trimmed; KiCad's `~`
/// (no value) is blank.
pub fn symbol_fields(entry: &[u8]) -> Vec<(String, String)> {
    symbol_properties(entry)
        .into_iter()
        .map(|(name, value)| {
            let value = value.trim();
            let value = if value == "~" { "" } else { value };
            (name, value.to_owned())
        })
        .collect()
}

/// The first non-blank value of a property whose plain name is in `names`,
/// in the order of `names`.
fn first_of(properties: &[(String, String)], names: &[&str]) -> String {
    names
        .iter()
        .find_map(|wanted| {
            properties
                .iter()
                .find(|(name, value)| plain(name) == *wanted && !value.is_empty())
                .map(|(_, value)| value.clone())
        })
        .unwrap_or_default()
}

/// The MPN and the manufacturer a symbol's properties name, blank when they
/// do not.
pub fn manufacturer_part(properties: &[(String, String)]) -> (String, String) {
    (
        first_of(properties, &MPN_NAMES),
        first_of(properties, &MANUFACTURER_NAMES),
    )
}

/// The description a symbol's properties give, from `Description` or
/// `ki_description`.
pub fn description(properties: &[(String, String)]) -> String {
    first_of(properties, &DESCRIPTION_NAMES)
}

/// The keywords a symbol's properties give, from `ki_keywords`.
pub fn keywords(properties: &[(String, String)]) -> String {
    first_of(properties, &KEYWORD_NAMES)
}

/// The fields of a new part in a category with `category_fields`, filled
/// from a symbol's properties: a field takes the value of the property with
/// the same name, without regard to case, spaces or punctuation (a
/// Description field also takes `ki_description`). Also gives the
/// properties with a value that filled nothing, so nothing is silently
/// lost; the MPN, the manufacturer and the Reference are left out of those,
/// as the manufacturer part and the symbol hold them.
pub fn fill_fields(
    category_fields: &[String],
    properties: &[(String, String)],
) -> (Vec<Property>, Vec<Property>) {
    let mut used: BTreeSet<String> = BTreeSet::new();
    let filled = category_fields
        .iter()
        .map(|field| {
            let wanted = plain(field);
            let names: Vec<&str> = if wanted == "description" {
                DESCRIPTION_NAMES.to_vec()
            } else {
                vec![wanted.as_str()]
            };
            let found = names.iter().find_map(|name| {
                properties
                    .iter()
                    .find(|(property, value)| plain(property) == *name && !value.is_empty())
            });
            match found {
                Some((property, value)) => {
                    used.insert(property.clone());
                    (field.clone(), value.clone())
                }
                None => (field.clone(), String::new()),
            }
        })
        .collect();
    let (mpn, manufacturer) = manufacturer_part(properties);
    let unused = properties
        .iter()
        .filter(|(name, value)| {
            let name_plain = plain(name);
            let holds_the_manufacturer_part = (MPN_NAMES.contains(&name_plain.as_str())
                && *value == mpn)
                || (MANUFACTURER_NAMES.contains(&name_plain.as_str()) && *value == manufacturer);
            let left_out = value.is_empty() || used.contains(name) || name_plain == "reference";
            !left_out && !holds_the_manufacturer_part
        })
        .cloned()
        .collect();
    (filled, unused)
}

// ---- Pins, pads and 3D models -----------------------------------------------

/// How many distinct pin numbers a symbol entry has across its units.
pub fn pin_count(entry: &[u8]) -> usize {
    symbol_pin_numbers(entry).len()
}

/// The pads of a footprint that count: numbered pads that are not
/// mechanical holes.
fn numbered_pads(footprint: &[u8]) -> Vec<kicad_integration::library_file::Pad> {
    footprint_pads(footprint)
        .into_iter()
        .filter(|pad| !pad.number.trim().is_empty() && pad.kind != "np_thru_hole")
        .collect()
}

/// How many distinct numbered pads a footprint has. A thermal pad shares
/// its number with the pin it belongs to; mounting pads and holes without a
/// number are left out.
pub fn pad_count(footprint: &[u8]) -> usize {
    numbered_pads(footprint)
        .into_iter()
        .map(|pad| pad.number)
        .collect::<BTreeSet<_>>()
        .len()
}

/// One pad as [`pad_signature`] compares it: number, type, shape, then
/// position, rotation, size and drill rounded to hundredths (of a
/// millimetre, or of a degree), written out as one text.
pub type PadKey = String;

/// What two footprints with "the same pads" share: the set of numbered pads,
/// each with its number, type (`smd`, `thru_hole`, ...), shape, position,
/// rotation, size and drill, rounded to 0.01. `None` for a footprint without
/// numbered pads, which matches nothing.
pub fn pad_signature(footprint: &[u8]) -> Option<BTreeSet<PadKey>> {
    let hundredths = |value: f64| (value * 100.0).round() as i64;
    let pads: BTreeSet<PadKey> = numbered_pads(footprint)
        .into_iter()
        .map(|pad| {
            let drill: Vec<String> = pad
                .drill
                .iter()
                .map(|word| match word.parse::<f64>() {
                    Ok(size) => hundredths(size).to_string(),
                    Err(_) => word.clone(),
                })
                .collect();
            format!(
                "{}|{}|{}|{}|{}|{}|{}|{}|{}",
                pad.number,
                pad.kind,
                pad.shape,
                hundredths(pad.at.0),
                hundredths(pad.at.1),
                hundredths(pad.rotation),
                hundredths(pad.size.0),
                hundredths(pad.size.1),
                drill.join(" ")
            )
        })
        .collect();
    (!pads.is_empty()).then_some(pads)
}

/// `footprint` (the text of a `.kicad_mod`) using the 3D model at `path`,
/// such as `${GITPLM_PARTS}/3d-models/g-Package_TO_SOT_SMD.3dshapes/X.step`:
/// the `(model "...")` whose file name is `model_file` (without regard to
/// case) is pointed there, otherwise the first one; a footprint without a
/// model gets one. Nothing else changes.
pub fn link_model(footprint: &str, path: &str, model_file: &str) -> String {
    let spans = model_path_spans(footprint);
    let quoted = format!("\"{}\"", path.replace('\\', "\\\\").replace('"', "\\\""));
    let wanted = model_file.to_lowercase();
    let chosen = spans
        .iter()
        .find(|span| {
            last_part(footprint[(*span).clone()].trim_matches('"')).to_lowercase() == wanted
        })
        .or_else(|| spans.first());
    if let Some(span) = chosen {
        return format!(
            "{}{quoted}{}",
            &footprint[..span.start],
            &footprint[span.end..]
        );
    }
    let Some(close) = footprint.rfind(')') else {
        return footprint.to_owned();
    };
    format!(
        "{}\t(model {quoted}\n\t\t(offset\n\t\t\t(xyz 0 0 0)\n\t\t)\n\t\t(scale\n\t\t\t(xyz 1 1 1)\n\t\t)\n\t\t(rotate\n\t\t\t(xyz 0 0 0)\n\t\t)\n\t)\n{}",
        &footprint[..close],
        &footprint[close..]
    )
}

/// The byte span of each `(model "...")` path, quotes included, found the
/// way [`kicad_integration::library_file::model_references`] finds them, and
/// of each KiCad 5 path written without quotes, which that leaves out (so
/// the app and the team server agree); linking quotes it.
fn model_path_spans(footprint: &str) -> Vec<std::ops::Range<usize>> {
    let mut spans = Vec::new();
    let mut from = 0;
    while let Some(found) = footprint[from..].find("(model") {
        let after = from + found + "(model".len();
        let rest = &footprint[after..];
        let start = after + (rest.len() - rest.trim_start().len());
        from = after;
        if !footprint[start..].starts_with('"') {
            // KiCad 5 writes the path without quotes, up to a space or `)`.
            let end = footprint[start..]
                .find(|ch: char| ch.is_whitespace() || ch == '(' || ch == ')')
                .map_or(footprint.len(), |at| start + at);
            if end > start {
                spans.push(start..end);
            }
            continue;
        }
        let mut escaped = false;
        for (offset, ch) in footprint[start + 1..].char_indices() {
            if !escaped && ch == '"' {
                spans.push(start..start + 1 + offset + 1);
                break;
            }
            escaped = !escaped && ch == '\\';
        }
    }
    spans
}

/// The next free IPN for `prefix` (three capital letters): the number after
/// the highest number any IPN with that prefix uses, with variant `0001`,
/// such as `ICS-0047-0001` when `ICS-0046-...` is the highest. Numbers are
/// never reused, so a number below the highest is never offered. `None`
/// when the prefix is not three capital letters or every number is used.
pub fn next_free_ipn<'a>(
    prefix: &str,
    existing: impl IntoIterator<Item = &'a str>,
) -> Option<String> {
    if prefix.len() != 3 || !prefix.bytes().all(|b| b.is_ascii_uppercase()) {
        return None;
    }
    let highest = existing
        .into_iter()
        .filter_map(|ipn| crate::Ipn::parse(ipn).ok())
        .filter(|ipn| ipn.prefix() == prefix)
        .filter_map(|ipn| ipn.as_str()[4..8].parse::<u32>().ok())
        .max();
    let next = highest.map_or(1, |highest| highest + 1);
    (next <= 9999).then(|| format!("{prefix}-{next:04}-0001"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn zip_of(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        for (name, bytes) in entries {
            writer.start_file(*name, options).unwrap();
            writer.write_all(bytes).unwrap();
        }
        writer.finish().unwrap().into_inner()
    }

    #[test]
    fn keeps_kicad_files_and_leaves_out_the_rest() {
        let zip = zip_of(&[
            ("LM317/KiCad/LM317.kicad_sym", b"(kicad_symbol_lib)"),
            ("LM317/KiCad/SOT230.kicad_mod", b"(footprint \"SOT230\")"),
            ("LM317/3D/LM317.STP", b"ISO-10303-21;"),
            ("LM317/Altium/LM317.SchLib", b"x"),
            ("LM317/Eagle/LM317.lbr", b"x"),
            (
                "LM317/inner.zip",
                &zip_of(&[("a.kicad_mod", b"(footprint \"A\")")]),
            ),
        ]);
        let download = read_download(&[("LM317.zip".into(), zip)]).unwrap();
        let labels: Vec<&str> = download.files.iter().map(|f| f.label.as_str()).collect();
        assert_eq!(
            labels,
            [
                "LM317/KiCad/LM317.kicad_sym in LM317.zip",
                "LM317/KiCad/SOT230.kicad_mod in LM317.zip",
                "LM317/3D/LM317.STP in LM317.zip"
            ]
        );
        assert_eq!(download.files[2].kind, DownloadKind::Model3d);
        assert_eq!(download.files[0].file_name, "LM317.kicad_sym");
        assert_eq!(download.vendor, Some(Vendor::SamacSys));
        // Two other CAD tools' files and the ZIP inside the ZIP.
        assert_eq!(download.ignored, 3);
    }

    /// A stored ZIP whose entry `name` has the general-purpose `flag` bits
    /// and compression `method` patched in its local and central headers,
    /// as a password-protected or oddly packed entry would.
    fn patched(entries: &[(&str, &[u8])], name: &str, flag: u16, method: Option<u16>) -> Vec<u8> {
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        for (entry, bytes) in entries {
            writer.start_file(*entry, options).unwrap();
            writer.write_all(bytes).unwrap();
        }
        let mut zip = writer.finish().unwrap().into_inner();
        let needle = name.as_bytes();
        let mut at = 0;
        while let Some(found) = zip[at..]
            .windows(needle.len())
            .position(|window| window == needle)
        {
            let start = at + found;
            // A local header puts the name 30 bytes in, a central one 46.
            for (signature, offset, flags, methods) in [
                (0x04034b50u32, 30usize, 6usize, 8usize),
                (0x02014b50u32, 46, 8, 10),
            ] {
                if start >= offset {
                    let header = start - offset;
                    if zip[header..header + 4] == signature.to_le_bytes() {
                        let old =
                            u16::from_le_bytes([zip[header + flags], zip[header + flags + 1]]);
                        zip[header + flags..header + flags + 2]
                            .copy_from_slice(&(old | flag).to_le_bytes());
                        if let Some(method) = method {
                            zip[header + methods..header + methods + 2]
                                .copy_from_slice(&method.to_le_bytes());
                        }
                    }
                }
            }
            at = start + needle.len();
        }
        zip
    }

    #[test]
    fn a_protected_or_oddly_packed_file_matters_only_if_it_is_needed() {
        let entries: [(&str, &[u8]); 3] = [
            ("x/KiCad/F.kicad_mod", b"(footprint \"F\")"),
            ("x/Altium/F.PcbLib", b"secret"),
            ("x/Eagle/F.lbr", b"packed"),
        ];
        // Password-protected and oddly packed files the app leaves out
        // anyway do not stop it.
        let zip = patched(&entries, "x/Altium/F.PcbLib", 1, None);
        let download = read_download(&[("x.zip".into(), zip)]).unwrap();
        assert_eq!(download.files.len(), 1);
        assert_eq!(download.ignored, 2);
        let zip = patched(&entries, "x/Eagle/F.lbr", 0, Some(12));
        assert_eq!(
            read_download(&[("x.zip".into(), zip)]).unwrap().files.len(),
            1
        );
        // A needed KiCad file that cannot be read says so plainly.
        let zip = patched(&entries, "x/KiCad/F.kicad_mod", 1, None);
        let error = read_download(&[("x.zip".into(), zip)]).unwrap_err();
        assert_eq!(
            error,
            "x/KiCad/F.kicad_mod in x.zip is protected by a password, so it cannot be read."
        );
        let zip = patched(&entries, "x/KiCad/F.kicad_mod", 0, Some(12));
        let error = read_download(&[("x.zip".into(), zip)]).unwrap_err();
        assert!(
            error.starts_with(
                "x/KiCad/F.kicad_mod in x.zip is packed in a way this app cannot read"
            ),
            "{error}"
        );
    }

    #[test]
    fn tells_vendors_apart_by_their_layout() {
        for (name, vendor) in [
            ("KiCad/x.kicad_mod", Some(Vendor::SamacSys)),
            ("KiCAD/x/x.kicad_mod", Some(Vendor::UltraLibrarian)),
            ("how-to-import.htm", Some(Vendor::SnapEda)),
            ("x.kicad_mod", None),
        ] {
            let zip = zip_of(&[(name, b"(footprint \"x\")")]);
            assert_eq!(
                read_download(&[("d.zip".into(), zip)]).unwrap().vendor,
                vendor,
                "{name}"
            );
        }
        // Loose files.
        let loose = read_download(&[
            (
                "C:\\Downloads\\R.kicad_mod".into(),
                b"(footprint \"R\")".to_vec(),
            ),
            ("notes.txt".into(), b"x".to_vec()),
        ])
        .unwrap();
        assert_eq!(loose.files[0].label, "R.kicad_mod");
        assert_eq!(loose.ignored, 1);
    }

    #[test]
    fn hostile_entry_names_are_only_labels() {
        let zip = zip_of(&[
            ("../../escape.kicad_mod", b"(footprint \"E\")"),
            ("/etc/absolute.kicad_sym", b"(kicad_symbol_lib)"),
            ("C:\\Windows\\drive.step", b"x"),
        ]);
        let download = read_download(&[("evil.zip".into(), zip)]).unwrap();
        let names: Vec<(&str, &str)> = download
            .files
            .iter()
            .map(|f| (f.file_name.as_str(), f.label.as_str()))
            .collect();
        assert_eq!(
            names,
            [
                ("escape.kicad_mod", "../../escape.kicad_mod in evil.zip"),
                ("absolute.kicad_sym", "/etc/absolute.kicad_sym in evil.zip"),
                ("drive.step", "C:\\Windows\\drive.step in evil.zip")
            ]
        );
    }

    #[test]
    fn refuses_a_download_over_the_limits() {
        let limits = Limits {
            entries: 3,
            total_bytes: 1024 * 1024,
        };
        // A ZIP bomb: a few kilobytes that unpack to 2 MB.
        let zeros = vec![0u8; 2 * 1024 * 1024];
        let bomb = zip_of(&[("bomb.step", &zeros)]);
        assert!(bomb.len() < 64 * 1024, "{}", bomb.len());
        let error = read_download_with(&[("bomb.zip".into(), bomb)], &limits).unwrap_err();
        assert!(error.contains("more than 1 MB"), "{error}");
        // Too many entries.
        let many = zip_of(&[("a", b""), ("b", b""), ("c", b""), ("d", b"")]);
        let error = read_download_with(&[("many.zip".into(), many)], &limits).unwrap_err();
        assert!(error.contains("holds 4 files"), "{error}");
        // Loose files count too.
        let error = read_download_with(&[("big.step".into(), zeros)], &limits).unwrap_err();
        assert!(error.contains("more than 1 MB"), "{error}");
        // Not a ZIP at all.
        let error =
            read_download(&[("broken.zip".into(), b"PK\x03\x04nonsense".to_vec())]).unwrap_err();
        assert!(error.contains("could not be read as a ZIP file"), "{error}");
        assert_eq!(LIMITS.entries, 200);
        assert_eq!(LIMITS.total_bytes, 100 * 1024 * 1024);
    }

    fn properties(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
            .collect()
    }

    #[test]
    fn maps_vendor_property_names_to_fields() {
        let found = properties(&[
            ("Reference", "U"),
            ("Value", "LM317"),
            ("Datasheet", "https://example.test/lm317.pdf"),
            ("ki_description", "Adjustable regulator"),
            ("Manufacturer_Name", "Texas Instruments"),
            ("Manufacturer_Part_Number", "LM317DCYR"),
            ("Mouser Part Number", "595-LM317DCYR"),
            ("Height", "1.8 mm"),
        ]);
        assert_eq!(
            manufacturer_part(&found),
            ("LM317DCYR".to_owned(), "Texas Instruments".to_owned())
        );
        let fields = [
            "Description".to_owned(),
            "value".to_owned(),
            "Datasheet".to_owned(),
            "Voltage".to_owned(),
        ];
        let (filled, unused) = fill_fields(&fields, &found);
        assert_eq!(
            filled,
            properties(&[
                ("Description", "Adjustable regulator"),
                ("value", "LM317"),
                ("Datasheet", "https://example.test/lm317.pdf"),
                ("Voltage", ""),
            ])
        );
        assert_eq!(
            unused,
            properties(&[
                ("Mouser Part Number", "595-LM317DCYR"),
                ("Height", "1.8 mm")
            ])
        );
        // Other spellings.
        for (mpn_name, manufacturer_name) in [
            ("MPN", "Manufacturer"),
            ("Mfr Part Number", "MFR"),
            ("MP", "MF"),
        ] {
            let found = properties(&[(mpn_name, "X1"), (manufacturer_name, "Acme")]);
            assert_eq!(
                manufacturer_part(&found),
                ("X1".to_owned(), "Acme".to_owned())
            );
        }
        assert_eq!(manufacturer_part(&[]), (String::new(), String::new()));
        let entry =
            b"(symbol \"X\" (property \"Datasheet\" \"~\") (property \"Description\" \" A \"))";
        assert_eq!(
            symbol_fields(entry),
            properties(&[("Datasheet", ""), ("Description", "A")])
        );
    }

    #[test]
    fn counts_pins_and_numbered_pads() {
        let symbol = b"(symbol \"U\" (symbol \"U_1_1\" (pin input line (number \"1\")) (pin input line (number \"2\")) (pin input line (number \"3\"))) (symbol \"U_2_1\" (pin power_in line (number \"3\"))))";
        assert_eq!(pin_count(symbol), 3);
        // Pad 2 twice (a thermal pad shares the pin's number), a mounting
        // hole and an unnumbered pad.
        let footprint = b"(footprint \"F\" (pad \"1\" smd rect (at 0 0) (size 1 1)) (pad \"2\" smd rect (at 1 0) (size 1 1)) (pad \"2\" smd rect (at 3 0) (size 2 2)) (pad \"\" np_thru_hole circle (at 0 3) (size 1 1)) (pad \"MP\" np_thru_hole circle (at 0 4) (size 1 1)) (pad \"\" smd rect (at 0 5) (size 1 1)))";
        assert_eq!(pad_count(footprint), 2);
    }

    #[test]
    fn same_pads_means_same_numbers_shapes_sizes_and_places() {
        let a = b"(footprint \"A\" (layer F.Cu) (pad \"1\" smd roundrect (at -0.775 0) (size 0.9 0.95)) (pad \"2\" smd roundrect (at 0.775 0) (size 0.9 0.95)))";
        // Another name, other drawing, pads within rounding, another order.
        let b = b"(footprint \"B\" (fp_line) (pad \"2\" smd roundrect (at 0.7751 0.001) (size 0.9 0.95)) (pad \"1\" smd roundrect (at -0.775 0) (size 0.9 0.95)))";
        let c = b"(footprint \"C\" (pad \"1\" smd rect (at -0.775 0) (size 0.9 0.95)) (pad \"2\" smd roundrect (at 0.775 0) (size 0.9 0.95)))";
        assert_eq!(pad_signature(a), pad_signature(b));
        assert_ne!(pad_signature(a), pad_signature(c));
        // The pad type, the rotation and the drill count too.
        let thru = b"(footprint \"T\" (pad \"1\" thru_hole roundrect (at -0.775 0) (size 0.9 0.95) (drill 0.4)) (pad \"2\" smd roundrect (at 0.775 0) (size 0.9 0.95)))";
        let turned = b"(footprint \"T\" (pad \"1\" smd roundrect (at -0.775 0 90) (size 0.9 0.95)) (pad \"2\" smd roundrect (at 0.775 0) (size 0.9 0.95)))";
        assert_ne!(pad_signature(a), pad_signature(thru));
        assert_ne!(pad_signature(a), pad_signature(turned));
        let drilled = |drill: &str| {
            format!(
                "(footprint \"D\" (pad \"1\" thru_hole circle (at 0 0) (size 1.6 1.6) (drill {drill})))"
            )
        };
        assert_eq!(
            pad_signature(drilled("0.8").as_bytes()),
            pad_signature(drilled("0.8001").as_bytes())
        );
        assert_ne!(
            pad_signature(drilled("0.8").as_bytes()),
            pad_signature(drilled("1.0").as_bytes())
        );
        assert_ne!(
            pad_signature(drilled("0.8").as_bytes()),
            pad_signature(drilled("oval 0.8 1.2").as_bytes())
        );
        assert_eq!(pad_signature(b"(footprint \"E\")"), None);
    }

    #[test]
    fn links_a_footprint_to_its_3d_model() {
        let path = "${GITPLM_PARTS}/3d-models/g-Package_TO_SOT_SMD.3dshapes/SOT230.step";
        let named = "(footprint \"F\"\n\t(model \"C:/Users/me/SOT230.STEP\"\n\t\t(offset (xyz 0 0 0)))\n\t(model \"${KICAD9_3DMODEL_DIR}/x.step\")\n)";
        let linked = link_model(named, path, "sot230.step");
        assert_eq!(
            kicad_integration::library_file::model_references(&linked),
            [path, "${KICAD9_3DMODEL_DIR}/x.step"]
        );
        // No matching name: the first model is pointed there.
        let other = link_model("(footprint \"F\" (model \"a.wrl\"))", path, "SOT230.step");
        assert_eq!(other, format!("(footprint \"F\" (model \"{path}\"))"));
        // No model: one is added.
        let added = link_model(
            "(footprint \"F\"\n\t(layer \"F.Cu\")\n)\n",
            path,
            "SOT230.step",
        );
        assert_eq!(
            kicad_integration::library_file::model_references(&added),
            [path]
        );
        assert!(kicad_integration::library_file::FootprintFile::parse(added.as_bytes()).is_ok());
        // A KiCad 5 footprint writes its model path without quotes: it is
        // rewritten in place, not given a second model.
        let old = "(module F (layer F.Cu)\n  (model ${KISYS3DMOD}/Package_SO.3dshapes/SOT230.step\n    (at (xyz 0 0 0))))";
        let linked = link_model(old, path, "SOT230.step");
        assert_eq!(
            kicad_integration::library_file::model_references(&linked),
            [path]
        );
        assert_eq!(linked.matches("(model").count(), 1, "{linked}");
    }

    #[test]
    fn offers_the_number_after_the_highest() {
        let existing = [
            "ICS-0046-0001",
            "ICS-0012-0002",
            "RES-0000-4702",
            "ics-0099-0001",
        ];
        assert_eq!(
            next_free_ipn("ICS", existing).as_deref(),
            Some("ICS-0047-0001")
        );
        assert_eq!(
            next_free_ipn("RES", existing).as_deref(),
            Some("RES-0001-0001")
        );
        assert_eq!(
            next_free_ipn("NEW", existing).as_deref(),
            Some("NEW-0001-0001")
        );
        assert_eq!(next_free_ipn("NEW", ["NEW-9999-0001"]), None);
        assert_eq!(next_free_ipn("ne", existing), None);
    }
}
