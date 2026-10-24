//! The scan over made-up repos, each laid out the way real client
//! libraries are.

use super::*;

/// A repo's files and what the scan reads of them, as the app's copy
/// lists them.
fn repo(files: &[(&str, &str)]) -> (Vec<RepoFile>, Contents) {
    let listed: Vec<RepoFile> = files
        .iter()
        .map(|(path, text)| RepoFile {
            path: (*path).to_owned(),
            size: text.len() as u64,
            blob: format!("{:040x}", path.len()),
        })
        .collect();
    let bytes: HashMap<&str, &str> = files.iter().copied().collect();
    let contents = read_contents(&listed, |paths| {
        Ok::<_, ()>(
            paths
                .iter()
                .map(|path| bytes.get(path).map(|text| text.as_bytes().to_vec()))
                .collect(),
        )
    })
    .unwrap();
    (listed, contents)
}

fn scanned(files: &[(&str, &str)]) -> LibraryLayout {
    let (listed, read) = repo(files);
    scan(COMMIT, &listed, &read)
}

const COMMIT: &str = "0123456789abcdef0123456789abcdef01234567";
const SYMBOLS: &str = "(kicad_symbol_lib (version 20241209))\n";

fn footprint(links: &[&str]) -> String {
    let models = links
        .iter()
        .map(|link| format!("\t(model \"{link}\")\n"))
        .collect::<Vec<_>>()
        .concat();
    format!("(footprint \"F\"\n\t(version 20241229)\n{models})\n")
}

fn library(nickname: &str, path: &str, from_table: bool) -> LayoutLibrary {
    LayoutLibrary {
        nickname: nickname.into(),
        path: path.into(),
        from_table,
    }
}

fn variable(name: &str, folder: Option<&str>, links: usize) -> LayoutVariable {
    LayoutVariable {
        name: name.into(),
        folder: folder.map(str::to_owned),
        links,
    }
}

#[test]
fn with_no_lib_tables_nicknames_are_file_names() {
    let fp = footprint(&["${ACME_3D}/R_0603.step"]);
    let layout = scanned(&[
        ("symbols/Acme_Passives.kicad_sym", SYMBOLS),
        ("footprints/Acme.pretty/R_0603.kicad_mod", &fp),
        ("3d/R_0603.step", "ISO-10303-21;"),
        ("README.md", "Our library"),
    ]);
    assert_eq!(
        layout.symbol_libraries,
        [library(
            "Acme_Passives",
            "symbols/Acme_Passives.kicad_sym",
            false
        )]
    );
    assert_eq!(
        layout.footprint_libraries,
        [library("Acme", "footprints/Acme.pretty", false)]
    );
    assert_eq!(layout.model_folders, ["3d"]);
    assert_eq!(layout.model_files, 1);
    assert_eq!(layout.path_variables, [variable("ACME_3D", Some("3d"), 1)]);
    assert!(layout.doubts.is_empty(), "{:?}", layout.doubts);
    assert!(layout.lib_tables.is_empty());
    assert_eq!(layout.commit, COMMIT);
    // A variable may point to any folder that holds models or libraries.
    assert_eq!(layout.folders, ["", "3d", "footprints", "symbols"]);
}

#[test]
fn lib_tables_give_nicknames_and_kiprjmod_is_the_tables_folder() {
    let layout = scanned(&[
        (
            "kicad/sym-lib-table",
            r#"(sym_lib_table (version 7)
  (lib (name "ACME")(type "KiCad")(uri "${KIPRJMOD}/../lib/acme_symbols.kicad_sym")(options "")(descr ""))
  (lib (name "Device")(type "KiCad")(uri "${KICAD10_SYMBOL_DIR}/Device.kicad_sym")(options "")(descr ""))
  (lib (name "Off")(type "KiCad")(uri "${KIPRJMOD}/../lib/old.kicad_sym")(options "")(descr "")(disabled)))"#,
        ),
        (
            "kicad/fp-lib-table",
            r#"(fp_lib_table (version 7)
  (lib (name "Acme_FP")(type "KiCad")(uri "../lib/Acme.pretty")(options "")(descr "")))"#,
        ),
        ("lib/acme_symbols.kicad_sym", SYMBOLS),
        ("lib/old.kicad_sym", SYMBOLS),
        ("lib/Acme.pretty/R.kicad_mod", &footprint(&[])),
    ]);
    assert_eq!(
        layout.symbol_libraries,
        [
            library("ACME", "lib/acme_symbols.kicad_sym", true),
            // A disabled row names nothing, so the file name stays.
            library("old", "lib/old.kicad_sym", false),
        ]
    );
    // A relative row path is relative to its table's folder.
    assert_eq!(
        layout.footprint_libraries,
        [library("Acme_FP", "lib/Acme.pretty", true)]
    );
    assert_eq!(
        layout.lib_tables,
        ["kicad/fp-lib-table", "kicad/sym-lib-table"]
    );
    // KiCad's own variables are KiCad's, not the repo's.
    assert!(layout.path_variables.is_empty());
}

#[test]
fn a_table_in_a_subfolder_and_a_variable_in_a_row_are_followed() {
    // A top table includes a nested one; rows use a path variable whose
    // folder is found by matching the libraries the rows name.
    let layout = scanned(&[
        (
            "fp-lib-table",
            r#"(fp_lib_table (version 7)
  (lib (name "More")(type "Table")(uri "${KIPRJMOD}/sub/fp-lib-table")(options "")(descr "")))"#,
        ),
        (
            "sub/fp-lib-table",
            r#"(fp_lib_table (version 7)
  (lib (name "Conn")(type "KiCad")(uri "${CLIENT_LIB}/footprints/Connectors.pretty")(options "")(descr ""))
  (lib (name "Gone")(type "KiCad")(uri "${CLIENT_LIB}/footprints/Gone.pretty")(options "")(descr "")))"#,
        ),
        (
            "libs/footprints/Connectors.pretty/J1.kicad_mod",
            &footprint(&[]),
        ),
    ]);
    assert_eq!(
        layout.footprint_libraries,
        [library("Conn", "libs/footprints/Connectors.pretty", true)]
    );
    assert_eq!(
        layout.path_variables,
        [variable("CLIENT_LIB", Some("libs"), 2)]
    );
}

#[test]
fn models_next_to_footprints_count_in_the_footprint_folder() {
    let layout = scanned(&[
        (
            "Acme.pretty/SOT-23.kicad_mod",
            &footprint(&[
                "${KIPRJMOD}/Acme.pretty/SOT-23.step",
                "${ACME}/Acme.pretty/SOT-23.wrl",
            ]),
        ),
        ("Acme.pretty/SOT-23.step", "ISO-10303-21;"),
        ("Acme.pretty/SOT-23.wrl", "#VRML V2.0 utf8"),
    ]);
    assert_eq!(layout.model_folders, ["Acme.pretty"]);
    assert_eq!(layout.model_files, 2);
    // ${KIPRJMOD} is the project's folder, which a library repo can't
    // know, so it is no variable to set.
    assert_eq!(layout.path_variables, [variable("ACME", Some(""), 1)]);
    assert!(layout.doubts.is_empty(), "{:?}", layout.doubts);
}

#[test]
fn kicads_3dshapes_folders_count_in_the_folder_above_them() {
    let layout = scanned(&[
        (
            "footprints/Acme_R.pretty/R_0603.kicad_mod",
            &footprint(&["${ACME_LIB}/3dmodels/Acme_R.3dshapes/R_0603.step"]),
        ),
        (
            "footprints/Acme_C.pretty/C_0603.kicad_mod",
            &footprint(&["${ACME_3D}/Acme_C.3dshapes/C_0603.step"]),
        ),
        ("3dmodels/Acme_R.3dshapes/R_0603.step", "ISO-10303-21;"),
        ("3dmodels/Acme_C.3dshapes/C_0603.step", "ISO-10303-21;"),
    ]);
    assert_eq!(layout.model_folders, ["3dmodels"]);
    // Two variables: one for the repo's top, one for its model folder.
    assert_eq!(
        layout.path_variables,
        [
            variable("ACME_3D", Some("3dmodels"), 1),
            variable("ACME_LIB", Some(""), 1),
        ]
    );
}

#[test]
fn links_to_missing_models_and_unmatched_variables_are_doubts() {
    let layout = scanned(&[
        (
            "Acme.pretty/A.kicad_mod",
            &footprint(&[
                "${ACME_3D}/A.step",
                "${ACME_3D}/Gone.step",
                "${HOME_3D}/x.step",
            ]),
        ),
        ("3d/A.step", "ISO-10303-21;"),
    ]);
    assert_eq!(
        layout.path_variables,
        [
            variable("ACME_3D", Some("3d"), 2),
            variable("HOME_3D", None, 1),
        ]
    );
    assert_eq!(
        layout.doubts,
        [
            LayoutDoubt::MissingModel {
                footprint: "Acme.pretty/A.kicad_mod".into(),
                link: "${ACME_3D}/Gone.step".into(),
            },
            LayoutDoubt::VariableNotFound {
                name: "HOME_3D".into()
            },
        ]
    );
}

#[test]
fn a_name_that_differs_only_in_capitals_works_only_on_some_systems() {
    let layout = scanned(&[
        (
            "Acme.pretty/A.kicad_mod",
            &footprint(&["${ACME_3D}/a.STEP"]),
        ),
        ("Models/A.step", "ISO-10303-21;"),
    ]);
    // The folder is still found, by the file's own capitals.
    assert_eq!(
        layout.path_variables,
        [variable("ACME_3D", Some("Models"), 1)]
    );
    assert_eq!(
        layout.doubts,
        [LayoutDoubt::CaseDiffers {
            footprint: "Acme.pretty/A.kicad_mod".into(),
            link: "${ACME_3D}/a.STEP".into(),
            file: "Models/A.step".into(),
        }]
    );
}

#[test]
fn two_libraries_with_one_nickname_clash() {
    let layout = scanned(&[
        ("old/Acme.kicad_sym", SYMBOLS),
        ("new/Acme.kicad_sym", SYMBOLS),
        // Nicknames differing only in capitals are two nicknames in KiCad.
        ("new/acme.kicad_sym", SYMBOLS),
    ]);
    assert_eq!(layout.symbol_libraries.len(), 3);
    assert_eq!(
        layout.doubts,
        [LayoutDoubt::NicknameClash {
            nickname: "Acme".into(),
            paths: vec!["new/Acme.kicad_sym".into(), "old/Acme.kicad_sym".into()],
        }]
    );
}

#[test]
fn git_lfs_pointers_are_reported_not_read_as_models() {
    let pointer = "version https://git-lfs.github.com/spec/v1\noid sha256:4d7a214614ab2935c943f9e0ff69d22eadbb8f32b1258daaa5e2ca24d17e2393\nsize 12345\n";
    let layout = scanned(&[
        (
            "Acme.pretty/A.kicad_mod",
            &footprint(&["${ACME_3D}/A.step"]),
        ),
        ("3d/A.step", pointer),
        ("3d/B.step", "ISO-10303-21;"),
    ]);
    // Still a model file in the model folder, so links to it work.
    assert_eq!(layout.model_files, 2);
    assert_eq!(layout.path_variables, [variable("ACME_3D", Some("3d"), 1)]);
    assert_eq!(
        layout.doubts,
        [LayoutDoubt::GitLfs {
            path: "3d/A.step".into()
        }]
    );
}

#[test]
fn database_libraries_symbol_folders_and_bad_tables_are_found() {
    let layout = scanned(&[
        ("db/acme.kicad_dbl", "{}"),
        ("symbols/Acme.kicad_symdir/R.kicad_sym", SYMBOLS),
        ("symbols/Acme.kicad_symdir/C.kicad_sym", SYMBOLS),
        ("sym-lib-table", "(sym_lib_table (lib"),
    ]);
    assert_eq!(layout.database_libraries, ["db/acme.kicad_dbl"]);
    // A KiCad 10 symbol folder is one library, not one per file.
    assert_eq!(
        layout.symbol_libraries,
        [library("Acme", "symbols/Acme.kicad_symdir", false)]
    );
    assert!(matches!(
        layout.doubts.as_slice(),
        [LayoutDoubt::TableUnreadable { table, .. }] if table == "sym-lib-table"
    ));
}

#[test]
fn only_small_models_and_every_footprint_and_table_are_read() {
    let big = "x".repeat(2000);
    let (listed, _) = repo(&[
        ("3d/big.step", &big),
        ("3d/small.step", "ISO"),
        ("Acme.pretty/A.kicad_mod", "(footprint)"),
        ("Acme.pretty/notes.txt", "notes"),
        ("fp-lib-table", "(fp_lib_table)"),
        ("Acme.kicad_sym", &big),
    ]);
    assert_eq!(
        files_to_read(&listed),
        ["3d/small.step", "Acme.pretty/A.kicad_mod", "fp-lib-table"]
    );
    // A footprint outside a library is read too, for its links.
    let (listed, _) = repo(&[("loose/F.kicad_mod", "(footprint)")]);
    assert_eq!(files_to_read(&listed), ["loose/F.kicad_mod"]);
}

#[test]
fn library_files_windows_cannot_keep_are_left_out_and_said() {
    let layout = scanned(&[
        ("CON.kicad_sym", SYMBOLS),
        ("Acme.pretty/R.kicad_mod", &footprint(&[])),
        ("lib./Acme.pretty/R.kicad_mod", &footprint(&[])),
        ("Good.kicad_sym", SYMBOLS),
    ]);
    assert_eq!(
        layout.symbol_libraries,
        [library("Good", "Good.kicad_sym", false)]
    );
    assert_eq!(layout.footprint_libraries.len(), 1);
    assert_eq!(
        layout.doubts,
        [
            LayoutDoubt::PathNotUsable {
                path: "CON.kicad_sym".into()
            },
            LayoutDoubt::PathNotUsable {
                path: "lib./Acme.pretty/R.kicad_mod".into()
            },
        ]
    );
}

#[test]
fn what_kicad_cannot_use_as_a_library_is_said_not_skipped() {
    let layout = scanned(&[
        // An old-format symbol library, as DigiKey's library has many.
        ("symbols/Old.lib", "EESchema-LIBRARY Version 2.4\n"),
        // A footprint outside any .pretty folder, whose link still counts.
        ("parts/F.kicad_mod", &footprint(&["${AMZ}/F.step"])),
        ("3d/F.step", "ISO"),
        // KiCad's own folders: its 3D models are fine, an add-on is not.
        (
            "Acme.pretty/A.kicad_mod",
            &footprint(&[
                "${KICAD8_3DMODEL_DIR}/R.step",
                "${KICAD8_3RD_PARTY}/3dmodels/x.step",
                "${KICAD8_3RD_PARTY}/3dmodels/y.step",
            ]),
        ),
    ]);
    assert_eq!(layout.symbol_libraries, []);
    assert_eq!(layout.path_variables, [variable("AMZ", Some("3d"), 1)]);
    assert_eq!(
        layout.doubts,
        [
            LayoutDoubt::OldSymbolLibrary {
                path: "symbols/Old.lib".into()
            },
            LayoutDoubt::FootprintOutsideLibrary {
                path: "parts/F.kicad_mod".into()
            },
            LayoutDoubt::KicadVariable {
                name: "KICAD8_3RD_PARTY".into(),
                links: 2
            },
        ]
    );
}

#[test]
fn files_are_read_in_batches_and_a_huge_footprint_is_left_unread() {
    let big = RepoFile {
        path: "Acme.pretty/Logo.kicad_mod".into(),
        size: MAX_READ_BYTES + 1,
        blob: "b".repeat(40),
    };
    let many: Vec<RepoFile> = (0..5)
        .map(|n| RepoFile {
            path: format!("Acme.pretty/F{n}.kicad_mod"),
            size: MAX_READ_BYTES,
            blob: "a".repeat(40),
        })
        .chain([big])
        .collect();
    let mut batches = Vec::new();
    let contents = read_contents(&many, |paths| {
        batches.push(paths.len());
        Ok::<_, ()>(
            paths
                .iter()
                .map(|_| Some(b"(footprint)".to_vec()))
                .collect(),
        )
    })
    .unwrap();
    // Four files of a quarter of a batch each fill one read.
    assert_eq!(BATCH_BYTES, 4 * MAX_READ_BYTES);
    assert_eq!(batches, [4, 1]);
    let layout = scan(COMMIT, &many, &contents);
    assert_eq!(
        layout.doubts,
        [LayoutDoubt::TooLargeToCheck {
            path: "Acme.pretty/Logo.kicad_mod".into()
        }]
    );
}
