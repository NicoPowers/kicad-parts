//! Files someone chooses in the app to use as a part's symbol, footprint or
//! 3D model, read into the uploads an edit sends (kicad-parts-3b8b).
//!
//! They are read the way the CSV import reads the CSV catalog's files
//! ([`crate::csv_import`]): a symbol is the exact bytes of one top-level
//! `(symbol ...)` entry of a `.kicad_sym`, with the library's
//! `(version ...)`; a footprint is a whole `.kicad_mod`; a 3D model is a
//! whole file. The team server reads and checks every file again and never
//! trusts this reading.

use crate::csv_import::classify_model_reference;
use crate::pocketbase::{CadUpload, UploadRole};
use kicad_integration::LibRef;
use kicad_integration::library_file::{FootprintFile, SymbolLibrary, SymbolLookup};

/// The symbols of a `.kicad_sym` file.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SymbolFile {
    /// The library's `(version ...)`.
    pub version: Option<String>,
    /// Every top-level symbol's name, in file order.
    pub names: Vec<String>,
}

/// What a `.kicad_mod` file holds.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FootprintFileInfo {
    /// The footprint's own name.
    pub name: String,
    /// Each 3D model reference, in file order.
    pub models: Vec<ModelFileReference>,
}

/// One `(model "...")` of a footprint file.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ModelFileReference {
    /// Exactly as written.
    pub reference: String,
    /// For a reference into the Catalog's 3D model folder
    /// (`${GITPLM_PARTS}/3d-models/...`), the repository path the 3D model's
    /// CAD item has as its key, such as
    /// `3d-models/g-Diode_SMD.3dshapes/D_SOD-923.step`. `None` for anything
    /// else, such as KiCad's own models, which stay external.
    pub key: Option<String>,
}

/// The text of a symbol or footprint file, or why it is not one KiCad saved.
fn kicad_text(bytes: &[u8]) -> Result<&str, String> {
    std::str::from_utf8(bytes)
        .map_err(|_| "The file is not text, so it is not a file KiCad saved.".to_owned())
}

/// Reads a `.kicad_sym` file: its version and the names of its symbols.
pub fn read_symbol_file(bytes: &[u8]) -> Result<SymbolFile, String> {
    kicad_text(bytes)?;
    let library = SymbolLibrary::parse(bytes)
        .map_err(|error| format!("The file is not a KiCad symbol library: {error}."))?;
    if library.entries.is_empty() {
        return Err("The file holds no symbol.".into());
    }
    Ok(SymbolFile {
        version: library.version,
        names: library
            .entries
            .into_iter()
            .map(|entry| entry.name)
            .collect(),
    })
}

/// The upload of the symbol named `name` in a `.kicad_sym` file, for
/// `reference` (`LIB:NAME`, whose name must be `name`): the exact bytes of
/// its entry and the library's version, as the import stores a symbol.
pub fn symbol_upload(
    bytes: &[u8],
    name: &str,
    reference: &str,
    file_name: &str,
) -> Result<CadUpload, String> {
    kicad_text(bytes)?;
    check_name(reference, name, "symbol")?;
    let library = SymbolLibrary::parse(bytes)
        .map_err(|error| format!("The file is not a KiCad symbol library: {error}."))?;
    let entry = match library.find(name) {
        SymbolLookup::Found(entry) => entry,
        SymbolLookup::Missing => return Err(format!("The file has no symbol named {name}.")),
        SymbolLookup::Ambiguous => {
            return Err(format!("The file has more than one symbol named {name}."));
        }
    };
    if let Some(parent) = &entry.extends {
        return Err(format!(
            "The symbol {name} is drawn from another symbol ({parent}), so it cannot be used on its own. Choose another symbol."
        ));
    }
    let Some(version) = library.version.clone() else {
        return Err("The file does not say which KiCad version saved it.".into());
    };
    // KiCad file versions are dates, such as 20241209.
    if version.len() != 8
        || !version.starts_with("20")
        || !version.bytes().all(|b| b.is_ascii_digit())
    {
        return Err(format!(
            "The file says it was saved in KiCad file version {version}, which is not one KiCad writes."
        ));
    }
    Ok(CadUpload {
        role: UploadRole::Symbol,
        reference: reference.to_owned(),
        file_name: file_name.to_owned(),
        format_version: Some(version),
        bytes: bytes[entry.span.clone()].to_vec(),
    })
}

/// Reads a `.kicad_mod` file: its name and 3D model references.
pub fn read_footprint_file(bytes: &[u8]) -> Result<FootprintFileInfo, String> {
    let text = kicad_text(bytes)?;
    let footprint = FootprintFile::parse(bytes)
        .map_err(|error| format!("The file is not a KiCad footprint: {error}."))?;
    let name = footprint
        .name
        .filter(|name| !name.is_empty())
        .ok_or_else(|| "The footprint in the file has no name.".to_owned())?;
    // The same reading the server and the CSV catalog fixture use.
    let models = kicad_integration::library_file::model_references(text)
        .into_iter()
        .map(|reference| ModelFileReference {
            key: classify_model_reference(&reference)
                .candidate
                .filter(|key| is_catalog_model_key(key)),
            reference,
        })
        .collect();
    Ok(FootprintFileInfo { name, models })
}

/// The upload of a whole `.kicad_mod` file for `reference` (`LIB:NAME`,
/// whose name must be the footprint's own name).
pub fn footprint_upload(
    bytes: &[u8],
    reference: &str,
    file_name: &str,
) -> Result<CadUpload, String> {
    let file = read_footprint_file(bytes)?;
    check_name(reference, &file.name, "footprint")?;
    Ok(CadUpload {
        role: UploadRole::Footprint,
        reference: reference.to_owned(),
        file_name: file_name.to_owned(),
        format_version: None,
        bytes: bytes.to_vec(),
    })
}

/// The upload of a whole 3D model file for `key`, the `3d-models/...` path
/// a footprint sent with it names.
pub fn model_upload(bytes: &[u8], key: &str, file_name: &str) -> Result<CadUpload, String> {
    if !is_catalog_model_key(key) {
        return Err(format!(
            "{key} is not where the Catalog keeps 3D models, such as 3d-models/g-Resistor_SMD.3dshapes/R_0603_1608Metric.step."
        ));
    }
    if bytes.is_empty() {
        return Err(format!("The 3D model file {file_name} is empty."));
    }
    Ok(CadUpload {
        role: UploadRole::Model3d,
        reference: key.to_owned(),
        file_name: file_name.to_owned(),
        format_version: None,
        bytes: bytes.to_vec(),
    })
}

/// Whether `name` can be a library or item name: what [`LibRef::parse`]
/// allows (not empty, `.` or `..`, and no `/`, `\\`, `:` or NUL), with no
/// other control character and no leading `-`. Both halves of a reference
/// become file names, in KiCad's libraries and in the preview's staging
/// folder, and a name can come from a downloaded file.
pub fn is_plain_name(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && !name.starts_with('-')
        && !name.contains(['/', '\\', ':'])
        && !name.chars().any(char::is_control)
}

/// Whether `key` is where the Catalog keeps a 3D model: its footprint
/// library's folder, `3d-models/g-<library>.3dshapes/<file>`, with plain
/// names (kicad-parts-82ea), so it can never leave that folder. The team
/// server's catalog_lib.js isCatalogModelKey applies the same rule.
pub fn is_catalog_model_key(key: &str) -> bool {
    let Some(rest) = key.strip_prefix("3d-models/") else {
        return false;
    };
    let Some((folder, file)) = rest.split_once('/') else {
        return false;
    };
    let Some(library) = folder.strip_suffix(".3dshapes") else {
        return false;
    };
    library.starts_with("g-")
        && library.len() > 2
        && is_plain_name(library)
        && is_plain_name(file)
        && !file.contains('/')
}

/// Checks that `reference` is `LIB:NAME` in one of the Catalog's own (`g-`)
/// libraries, with `name` after the colon, and that both are plain names
/// ([`is_plain_name`]), exactly as written.
fn check_name(reference: &str, name: &str, what: &str) -> Result<(), String> {
    if !is_plain_name(name) {
        return Err(format!(
            "The {what}'s name in the file, {name:?}, cannot be used: a name cannot be . or .., start with -, or hold / \\ : or control characters."
        ));
    }
    let Some((library, item)) = reference.split_once(':') else {
        return Err(format!(
            "Give the {what} a library, such as g-res:{name}, before you save."
        ));
    };
    let parsed = LibRef::parse(reference).ok();
    if !is_plain_name(library)
        || !is_plain_name(item)
        || parsed.is_none_or(|parsed| parsed.to_string() != reference)
    {
        return Err(format!(
            "{reference:?} cannot be used as a reference: write LIBRARY:NAME, with no spaces around it, and a library and name that are not . or .., do not start with -, and hold no / \\ : or control characters."
        ));
    }
    if !library.starts_with("g-") || library.len() < 3 {
        return Err(format!(
            "A file can only be added to the Catalog's own libraries, whose names start with g-, not {library}."
        ));
    }
    if item != name {
        return Err(format!(
            "The name after the colon must be {name}, the {what}'s name in the file."
        ));
    }
    Ok(())
}

/// The Catalog library a footprint named `name` belongs in, from its
/// package (kicad-parts-f397: footprints are grouped like KiCad's own
/// libraries), such as `g-Resistor_SMD` for `R_0603_1608Metric`; `None`
/// when the name does not say, so the person chooses.
pub fn suggested_footprint_library(name: &str) -> Option<&'static str> {
    let starts = |prefixes: &[&str]| prefixes.iter().any(|prefix| name.starts_with(prefix));
    let library = if starts(&["R_"]) {
        "g-Resistor_SMD"
    } else if starts(&["C_"]) {
        "g-Capacitor_SMD"
    } else if starts(&["LED_"]) {
        "g-LED_SMD"
    } else if starts(&["L_"]) {
        "g-Inductor_SMD"
    } else if starts(&["D_"]) {
        "g-Diode_SMD"
    } else if starts(&["SOT-", "SC-70", "TSOT"]) {
        "g-Package_TO_SOT_SMD"
    } else if starts(&["SOIC", "SSOP", "TSSOP", "SO-", "SO_"]) {
        "g-Package_SO"
    } else if starts(&["QFN", "DFN"]) {
        "g-Package_DFN_QFN"
    } else if starts(&["BGA"]) {
        "g-Package_BGA"
    } else {
        return None;
    };
    Some(library)
}

#[cfg(test)]
mod tests {
    use super::*;

    const TWO: &str = "(kicad_symbol_lib\n\t(version 20241209)\n\t(symbol \"A\"\n\t\t(in_bom yes)\n\t)\n\t(symbol \"B\"\n\t\t(symbol \"B_0_1\")\n\t)\n\t(symbol \"C\"\n\t\t(extends \"A\")\n\t)\n)\n";

    #[test]
    fn a_symbol_upload_is_one_entry_with_its_library_version() {
        let file = read_symbol_file(TWO.as_bytes()).unwrap();
        assert_eq!(file.names, ["A", "B", "C"]);
        assert_eq!(file.version.as_deref(), Some("20241209"));
        let upload = symbol_upload(TWO.as_bytes(), "B", "g-res:B", "two.kicad_sym").unwrap();
        assert_eq!(upload.role, UploadRole::Symbol);
        assert_eq!(upload.format_version.as_deref(), Some("20241209"));
        assert_eq!(
            String::from_utf8(upload.bytes).unwrap(),
            "(symbol \"B\"\n\t\t(symbol \"B_0_1\")\n\t)"
        );
        // Refused, plainly: a symbol drawn from another, a missing one, a
        // name that is not the symbol's, and a library that is not the
        // Catalog's.
        for (name, reference, says) in [
            ("C", "g-res:C", "drawn from another symbol"),
            ("Z", "g-res:Z", "no symbol named Z"),
            ("B", "g-res:A", "must be B"),
            ("B", "Device:B", "start with g-"),
            ("B", "B", "Give the symbol a library"),
        ] {
            let error = symbol_upload(TWO.as_bytes(), name, reference, "f").unwrap_err();
            assert!(error.contains(says), "{error}");
        }
        assert!(read_symbol_file(b"(kicad_symbol_lib (version 1))").is_err());
        let old = TWO.replace("20241209", "1");
        assert!(
            symbol_upload(old.as_bytes(), "B", "g-res:B", "f")
                .unwrap_err()
                .contains("not one KiCad writes")
        );
        assert!(
            read_symbol_file(&[0xff, 0xfe])
                .unwrap_err()
                .contains("not text")
        );
    }

    #[test]
    fn a_footprint_upload_names_its_catalog_3d_models() {
        let footprint = "(footprint \"R_0603_1608Metric\"\n\t(version 20241229)\n\t(model \"${GITPLM_PARTS}/3d-models/g-Resistor_SMD.3dshapes/R_0603_1608Metric.step\")\n\t(model \"${KICAD9_3DMODEL_DIR}/Resistor_SMD.3dshapes/R_0603_1608Metric.step\")\n)\n";
        let file = read_footprint_file(footprint.as_bytes()).unwrap();
        assert_eq!(file.name, "R_0603_1608Metric");
        let keys: Vec<Option<&str>> = file.models.iter().map(|m| m.key.as_deref()).collect();
        assert_eq!(
            keys,
            [
                Some("3d-models/g-Resistor_SMD.3dshapes/R_0603_1608Metric.step"),
                None
            ]
        );
        let upload = footprint_upload(
            footprint.as_bytes(),
            "g-Resistor_SMD:R_0603_1608Metric",
            "R.kicad_mod",
        )
        .unwrap();
        assert_eq!(upload.bytes, footprint.as_bytes());
        assert!(
            footprint_upload(footprint.as_bytes(), "g-Resistor_SMD:R_0805", "f")
                .unwrap_err()
                .contains("must be R_0603_1608Metric")
        );
        assert!(read_footprint_file(b"(kicad_symbol_lib)").is_err());
        assert!(model_upload(b"", "3d-models/g-a.3dshapes/x.step", "x.step").is_err());
    }

    #[test]
    fn names_that_could_leave_a_folder_are_refused() {
        // A downloaded file names itself; the name becomes a file name in
        // the preview's staging folder and in KiCad's libraries.
        for name in [
            "../escaped",
            r"..\escaped",
            r"C:\escaped\x",
            "/escaped",
            "a\u{0}b",
            "a:b",
            "a\u{1}b",
            "-a",
            "..",
        ] {
            let footprint = format!("(footprint {name:?})");
            let error = footprint_upload(footprint.as_bytes(), &format!("g-res:{name}"), "f")
                .expect_err(name);
            assert!(error.contains("cannot be used"), "{name:?}: {error}");
            let library = format!("(kicad_symbol_lib (version 1) (symbol {name:?}))");
            let error = symbol_upload(library.as_bytes(), name, &format!("g-res:{name}"), "f")
                .expect_err(name);
            assert!(error.contains("cannot be used"), "{name:?}: {error}");
        }
        // The library someone types is checked the same way.
        for reference in [
            "g-:R",
            "g-a/b:R",
            "g-a:b:R",
            "..:R",
            "-g:R",
            "g-res\u{1}:R",
            " g-res:R",
        ] {
            let footprint = "(footprint \"R\")";
            assert!(
                footprint_upload(footprint.as_bytes(), reference, "f").is_err(),
                "{reference:?}"
            );
        }
        assert!(footprint_upload(b"(footprint \"R\")", "g-res:R", "f").is_ok());
    }

    #[test]
    fn catalog_3d_model_paths_stay_in_their_library_folder() {
        assert!(is_catalog_model_key(
            "3d-models/g-Resistor_SMD.3dshapes/R_0603_1608Metric.step"
        ));
        for key in [
            "3d-models/../x.step",
            "3d-models/x.step",
            "3d-models/g-a.3dshapes/../x.step",
            "3d-models/g-a.3dshapes/sub/x.step",
            "3d-models/g-.3dshapes/x.step",
            "3d-models/g-a.3dshapes/-x.step",
            "3d-models/g-a.3dshapes/x\u{1}.step",
            "models/g-a.3dshapes/x.step",
        ] {
            assert!(!is_catalog_model_key(key), "{key}");
            assert!(model_upload(b"x", key, "x.step").is_err(), "{key}");
        }
        // A footprint naming a model elsewhere does not ask for it.
        let footprint = "(footprint \"F\" (model \"${GITPLM_PARTS}/3d-models/x.step\"))";
        let file = read_footprint_file(footprint.as_bytes()).unwrap();
        assert_eq!(file.models[0].key, None);
    }

    #[test]
    fn footprint_libraries_are_suggested_from_the_package() {
        for (name, library) in [
            ("R_0603_1608Metric", Some("g-Resistor_SMD")),
            ("C_0805_2012Metric", Some("g-Capacitor_SMD")),
            ("L_1210_3225Metric", Some("g-Inductor_SMD")),
            ("D_SOD-323", Some("g-Diode_SMD")),
            ("LED_0603_1608Metric", Some("g-LED_SMD")),
            ("SOT-23-5", Some("g-Package_TO_SOT_SMD")),
            ("SC-70-6", Some("g-Package_TO_SOT_SMD")),
            ("TSOT-23-6", Some("g-Package_TO_SOT_SMD")),
            ("SOIC-8_3.9x4.9mm_P1.27mm", Some("g-Package_SO")),
            ("TSSOP-20_4.4x6.5mm_P0.65mm", Some("g-Package_SO")),
            ("QFN-32-1EP_5x5mm_P0.5mm", Some("g-Package_DFN_QFN")),
            ("DFN-8_2x2mm_P0.5mm", Some("g-Package_DFN_QFN")),
            ("BGA-64_9x9mm", Some("g-Package_BGA")),
            ("PinHeader_1x02_P2.54mm_Vertical", None),
        ] {
            assert_eq!(suggested_footprint_library(name), library, "{name}");
        }
    }
}
