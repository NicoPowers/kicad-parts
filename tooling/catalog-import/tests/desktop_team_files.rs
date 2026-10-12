//! The desktop's Edit part with files (seed kicad-parts-3b8b), through
//! catalog-browser against a disposable PocketBase with a temp copy of the
//! resistors imported: a chosen file is read and a reference suggested, it
//! is drawn with kicad-cli before saving, saving makes a new CAD item
//! revision that the history shows with the file's name, and the message
//! says how many other parts keep the older revision. A later CSV import
//! never replaces it: an unchanged CSV shows nothing, a changed CSV file
//! shows a notice whose value can only be kept.

use base64::Engine;
use catalog::CsvCatalog;
use catalog::pocketbase::{ImportCommand, ImportOutcome, read_import_state, submit_import};
use catalog_browser::team::TeamCatalog;
use catalog_browser::{BrowserConfig, CatalogBrowser};
use catalog_import::services;
use collaboration::PocketBaseClient;
use desktop_contract::{
    FileKind, FilePreviewRequest, LiveUpdates, PreviewOutcome, PublishResult, SignInRequest,
    TeamCsvReviewRequest, TeamEditRequest, TeamFile, TeamFileRequest, TeamPart, TeamSession,
};
use kicad_integration::SearchLocations;
use std::fs;
use std::path::Path;
use std::time::{Duration, Instant};

const IPN: &str = "RES-0000-0470";
const OTHER: &str = "RES-0000-1000";
const FOOTPRINT: &str = "g-Resistor_SMD:R_0603_1608Metric";
const FOOTPRINT_FILE: &str = "footprints/g-Resistor_SMD.pretty/R_0603_1608Metric.kicad_mod";
const TWO_SYMBOLS: &str = "(kicad_symbol_lib\n\t(version 20241209)\n\t(generator \"kicad_symbol_editor\")\n\t(symbol \"R_First\"\n\t\t(in_bom yes)\n\t\t(property \"Reference\" \"R\" (at 0 0 0) (effects (font (size 1.27 1.27))))\n\t\t(symbol \"R_First_0_1\" (rectangle (start -1 -2) (end 1 2) (stroke (width 0.25) (type default)) (fill (type none))))\n\t)\n\t(symbol \"R_Second\"\n\t\t(in_bom yes)\n\t\t(property \"Reference\" \"R\" (at 0 0 0) (effects (font (size 1.27 1.27))))\n\t\t(symbol \"R_Second_0_1\" (circle (center 0 0) (radius 2) (stroke (width 0.25) (type default)) (fill (type none))))\n\t)\n)\n";

fn wait_until(what: &str, mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !done() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn copy_folder(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_folder(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).unwrap();
        }
    }
}

/// Imports the copy as an importer does, reading the team catalog first.
fn import(admin: &PocketBaseClient, copy: &Path) -> ImportOutcome {
    let import = CsvCatalog::new(copy).import("g-res", None).unwrap();
    let state = read_import_state(admin).unwrap();
    let command = ImportCommand::new(&import, "Import the resistors").expecting(&state);
    submit_import(admin, &command).unwrap().outcome
}

fn encoded(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

fn opened(part: &TeamPart, reason: &str) -> TeamEditRequest {
    TeamEditRequest {
        correlation_id: None,
        ipn: part.ipn.clone(),
        expected_details: part.details.number,
        expected_revision: part.revision.number,
        reason: reason.into(),
        fields: part.details.fields.clone(),
        manufacturer_parts: part.details.manufacturer_parts.clone(),
        cad_fields: part.revision.cad_fields.clone(),
        files: Vec::new(),
    }
}

fn file(kind: FileKind, name: &str, bytes: &[u8], symbol: Option<&str>) -> TeamFile {
    TeamFile {
        kind,
        file_name: name.into(),
        base64: encoded(bytes),
        symbol: symbol.map(str::to_owned),
        model_key: None,
    }
}

fn read_request(kind: FileKind, name: &str, bytes: &[u8], current: &str) -> TeamFileRequest {
    TeamFileRequest {
        ipn: IPN.into(),
        kind,
        file_name: name.into(),
        base64: encoded(bytes),
        current_reference: current.into(),
    }
}

#[test]
fn the_desktop_uses_files_and_imports_never_replace_them() {
    let root = services::repository_root();
    let state = tempfile::tempdir().unwrap();
    let (server, admin, _superuser) = services::start(&root, state.path()).unwrap();
    let copy = tempfile::tempdir().unwrap();
    for folder in ["database", "symbols", "footprints", "3d-models"] {
        copy_folder(&root.join(folder), &copy.path().join(folder));
    }
    assert!(matches!(
        import(&admin, copy.path()),
        ImportOutcome::Applied { .. }
    ));
    let editor = services::add_member(&admin, "Ada Editor", "ada@example.test", true).unwrap();
    let url = server.url().to_owned();
    let team = TeamCatalog::open(state.path().join("team.json"), Some(url.clone()), None);
    let status = team.sign_in(SignInRequest {
        server_url: url,
        email: editor.email.clone(),
        password: editor.password.clone(),
    });
    assert!(status.session.is_signed_in(), "{status:?}");
    wait_until("live updates", || {
        matches!(
            team.status().session,
            TeamSession::SignedIn {
                live: LiveUpdates::Live,
                ..
            }
        )
    });

    // A footprint file: its name, the current library, nothing missing.
    let original = fs::read(copy.path().join(FOOTPRINT_FILE)).unwrap();
    let corrected = String::from_utf8(original.clone()).unwrap().replacen(
        "\t(fp_line",
        "\t(fp_circle (center 0 0) (end 2 0) (stroke (width 0.3) (type solid)) (fill no) (layer \"F.SilkS\"))\n\t(fp_line",
        1,
    );
    let reading = team
        .read_file(read_request(
            FileKind::Footprint,
            "R_0603_1608Metric.kicad_mod",
            corrected.as_bytes(),
            FOOTPRINT,
        ))
        .unwrap();
    assert_eq!(reading.names, ["R_0603_1608Metric"]);
    assert_eq!(reading.chosen, "R_0603_1608Metric");
    assert_eq!(reading.library, "g-Resistor_SMD");
    assert!(reading.missing_models.is_empty());
    // From KiCad's own library, the library is suggested from the package;
    // a Catalog 3D model the Catalog lacks is asked for.
    let with_model = corrected.replace(
        "${KICAD9_3DMODEL_DIR}/Resistor_SMD.3dshapes/R_0603_1608Metric.step",
        "${GITPLM_PARTS}/3d-models/g-Resistor_SMD.3dshapes/R_0603_1608Metric.step",
    );
    let reading = team
        .read_file(read_request(
            FileKind::Footprint,
            "f.kicad_mod",
            with_model.as_bytes(),
            "Resistor_SMD:R_0603_1608Metric",
        ))
        .unwrap();
    assert_eq!(reading.library, "g-Resistor_SMD");
    assert_eq!(
        reading.missing_models,
        ["3d-models/g-Resistor_SMD.3dshapes/R_0603_1608Metric.step"]
    );
    // A symbol library with two symbols: the first is chosen (none is named
    // like the current symbol) and the library is the current one, or the
    // part's CSV file's when the current symbol is KiCad's own.
    let reading = team
        .read_file(read_request(
            FileKind::Symbol,
            "two.kicad_sym",
            TWO_SYMBOLS.as_bytes(),
            "g-res:R_US",
        ))
        .unwrap();
    assert_eq!(reading.names, ["R_First", "R_Second"]);
    assert_eq!(reading.chosen, "R_First");
    assert_eq!(reading.library, "g-res");
    let reading = team
        .read_file(read_request(
            FileKind::Symbol,
            "two.kicad_sym",
            TWO_SYMBOLS.as_bytes(),
            "Device:R_Second",
        ))
        .unwrap();
    assert_eq!(
        (reading.chosen.as_str(), reading.library.as_str()),
        ("R_Second", "g-res")
    );
    let error = team
        .read_file(read_request(FileKind::Symbol, "x.kicad_sym", b"hello", ""))
        .unwrap_err();
    assert!(error.contains("not a KiCad symbol library"), "{error}");

    // Both are drawn with kicad-cli before saving.
    let data = tempfile::tempdir().unwrap();
    let browser = CatalogBrowser::open(BrowserConfig {
        catalog_root: copy.path().to_path_buf(),
        cache_dir: data.path().join("cache"),
        settings_file: data.path().join("settings.json"),
        kicad_locations: SearchLocations::Standard,
    });
    for (file, reference) in [
        (
            file(
                FileKind::Footprint,
                "f.kicad_mod",
                corrected.as_bytes(),
                None,
            ),
            FOOTPRINT,
        ),
        (
            file(
                FileKind::Symbol,
                "two.kicad_sym",
                TWO_SYMBOLS.as_bytes(),
                Some("R_Second"),
            ),
            "g-res:R_Second",
        ),
    ] {
        let outcome = browser.preview_file(&FilePreviewRequest {
            file,
            reference: reference.into(),
            selection: None,
        });
        let PreviewOutcome::Ready(set) = outcome else {
            panic!("not drawn: {outcome:?}");
        };
        assert_eq!(set.reference, reference);
        assert!(!set.images.is_empty());
        assert_eq!(set.cad_item_revision, None);
    }

    // A file that names itself so it would leave the staging folder is not
    // drawn, and nothing is written outside the app's cache.
    for name in [
        "../../../escaped",
        r"..\..\..\escaped",
        r"C:\escaped\x",
        "a:b",
        "a\u{1}b",
    ] {
        // Quoted as KiCad writes strings: only backslashes and quotes are
        // escaped, so the control character stays as it is.
        let quoted = name.replace('\\', "\\\\").replace('"', "\\\"");
        let hostile = format!("(footprint \"{quoted}\"\n\t(version 20241229)\n)\n");
        let outcome = browser.preview_file(&FilePreviewRequest {
            file: file(FileKind::Footprint, "f.kicad_mod", hostile.as_bytes(), None),
            reference: format!("g-Resistor_SMD:{name}"),
            selection: None,
        });
        assert!(
            matches!(outcome, PreviewOutcome::Problem(_)),
            "{name:?}: {outcome:?}"
        );
    }
    assert!(
        !data
            .path()
            .parent()
            .unwrap()
            .join("escaped.kicad_mod")
            .exists()
    );
    assert!(!std::path::Path::new(r"C:\escaped").exists());

    // Saving the footprint file: footprint revision 2, for this part only.
    let part = team.part(IPN).unwrap().unwrap();
    let mut request = opened(&part, "Corrected silkscreen");
    request.files = vec![file(
        FileKind::Footprint,
        "R_0603_1608Metric.kicad_mod",
        corrected.as_bytes(),
        None,
    )];
    let PublishResult::Saved {
        revision,
        part,
        older_uses,
    } = team.edit(request).unwrap()
    else {
        panic!("not saved");
    };
    assert_eq!(revision, Some(2));
    assert_eq!(older_uses.len(), 1);
    assert_eq!(
        (
            older_uses[0].kind.as_str(),
            older_uses[0].reference.as_str()
        ),
        ("footprint", FOOTPRINT)
    );
    assert!(older_uses[0].other_parts > 1);
    assert!(
        part.revision
            .cad_items
            .iter()
            .any(|item| item.reference == FOOTPRINT && item.binding == "footprint revision 2")
    );
    let entry = &part.history[0];
    assert_eq!(entry.revision, Some(2));
    let row = entry
        .changes
        .iter()
        .find(|change| change.label == "Footprint file")
        .unwrap();
    assert_eq!(
        row.after,
        format!("R_0603_1608Metric.kicad_mod (footprint revision 2 of {FOOTPRINT})")
    );
    // It replaced the file the import read.
    assert_eq!(
        row.before,
        format!("{FOOTPRINT_FILE} (footprint revision 1 of {FOOTPRINT})")
    );
    let other = team.part(OTHER).unwrap().unwrap();
    assert!(
        other
            .revision
            .cad_items
            .iter()
            .any(|item| item.reference == FOOTPRINT && item.binding == "footprint revision 1")
    );

    // A new symbol from the second symbol of a file.
    let mut request = opened(&part, "Own symbol");
    request
        .cad_fields
        .iter_mut()
        .find(|field| field.name == "Symbol")
        .unwrap()
        .value = "g-res:R_Wrong".into();
    request.files = vec![file(
        FileKind::Symbol,
        "two.kicad_sym",
        TWO_SYMBOLS.as_bytes(),
        Some("R_Second"),
    )];
    let PublishResult::Refused { message } = team.edit(request.clone()).unwrap() else {
        panic!("not refused");
    };
    assert!(message.contains("must be R_Second"), "{message}");
    request
        .cad_fields
        .iter_mut()
        .find(|field| field.name == "Symbol")
        .unwrap()
        .value = "g-res:R_Second".into();
    let PublishResult::Saved {
        revision,
        part,
        older_uses,
    } = team.edit(request).unwrap()
    else {
        panic!("not saved");
    };
    assert_eq!(revision, Some(3));
    assert!(older_uses.is_empty(), "{older_uses:?}");
    assert!(
        part.history[0]
            .changes
            .iter()
            .any(|change| change.label == "Symbol file"
                && change.before.is_empty()
                && change.after == "two.kicad_sym (symbol revision 1 of g-res:R_Second)")
    );

    // The CSV has not changed since it was imported: nothing to review.
    assert!(matches!(
        import(&admin, copy.path()),
        ImportOutcome::NothingChanged { .. }
    ));
    assert!(team.part(IPN).unwrap().unwrap().csv_changes.is_empty());
    assert!(team.part(OTHER).unwrap().unwrap().csv_changes.is_empty());

    // The CSV's footprint file changes: parts that use it show it, and its
    // value can only be kept.
    let changed = String::from_utf8(original).unwrap().replacen(
        "(tags \"resistor\")",
        "(tags \"resistor 0603\")",
        1,
    );
    fs::write(copy.path().join(FOOTPRINT_FILE), changed).unwrap();
    assert!(matches!(
        import(&admin, copy.path()),
        ImportOutcome::NothingChanged { .. }
    ));
    let other = team.part(OTHER).unwrap().unwrap();
    assert_eq!(other.csv_changes.len(), 1, "{:?}", other.csv_changes);
    let changes = &other.csv_changes[0];
    assert!(changes.cad);
    assert_eq!(changes.rows.len(), 1);
    let row = &changes.rows[0];
    assert_eq!(row.field, "Footprint");
    assert!(row.keep_only);
    assert_eq!(row.now, format!("{FOOTPRINT}, footprint revision 1"));
    assert!(row.csv.contains("changed in this app"), "{}", row.csv);
    let edited = team.part(IPN).unwrap().unwrap();
    assert!(
        edited
            .csv_changes
            .iter()
            .any(|changes| changes.rows.iter().any(
                |row| row.keep_only && row.now == format!("{FOOTPRINT}, footprint revision 2")
            )),
        "{:?}",
        edited.csv_changes
    );
    let PublishResult::CsvReviewed { accepted, part, .. } = team
        .csv_review(TeamCsvReviewRequest {
            correlation_id: None,
            ipn: OTHER.into(),
            problem: changes.problem.clone(),
            accept: Vec::new(),
            expected: changes.expected,
            reason: "Keep the footprint".into(),
        })
        .unwrap()
    else {
        panic!("not reviewed");
    };
    assert!(accepted.is_empty());
    assert!(part.csv_changes.is_empty(), "{:?}", part.csv_changes);
}
