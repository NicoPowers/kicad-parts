//! Real team workflows across features (QA pass 2, 2026-10-12): Add part
//! (kicad-parts-9693) together with Edit part, files (kicad-parts-3b8b),
//! categories (kicad-parts-ae44), archive, CSV imports and CSV reviews,
//! through catalog-browser against a disposable PocketBase with temp copies
//! of the resistors and capacitors imported. Downloads are small ZIP files
//! built here from the repository's own CAD files (only read).

use base64::Engine;
use catalog::CsvCatalog;
use catalog::pocketbase::{ImportCommand, ImportOutcome, read_import_state, submit_import};
use catalog_browser::team::TeamCatalog;
use catalog_browser::{BrowserConfig, CatalogBrowser};
use catalog_import::services;
use collaboration::PocketBaseClient;
use desktop_contract::{
    DownloadedFile, Field, FileKind, LiveUpdates, ManufacturerPart, PublishResult, SignInRequest,
    TeamArchiveRequest, TeamCsvReviewRequest, TeamDownload, TeamDownloadRequest, TeamEditRequest,
    TeamFile, TeamNewPartRequest, TeamPart, TeamSession,
};
use kicad_integration::SearchLocations;
use native_services::PocketBaseServer;
use std::fs;
use std::io::Write;
use std::path::Path;
use std::time::{Duration, Instant};

const MPN: &str = "CL10B104KB8NNNC";
const NEW_FOOTPRINT: &str = "g-Capacitor_SMD:C_0603_New";
const NEW_FOOTPRINT_FILE: &str = "footprints/g-Capacitor_SMD.pretty/C_0603_New.kicad_mod";

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

fn import(admin: &PocketBaseClient, copy: &Path, csv_file: &str) -> ImportOutcome {
    let import = CsvCatalog::new(copy).import(csv_file, None).unwrap();
    let state = read_import_state(admin).unwrap();
    let command = ImportCommand::new(&import, "Import").expecting(&state);
    submit_import(admin, &command).unwrap().outcome
}

fn repository_text(path: &str) -> String {
    String::from_utf8(fs::read(services::repository_root().join(path)).unwrap()).unwrap()
}

/// The capacitor symbol renamed `name`, with the fields a SamacSys download
/// carries.
fn symbol_named(name: &str, mpn: &str) -> String {
    repository_text("symbols/g-cap.kicad_sym")
        .replace("(symbol \"C_", &format!("(symbol \"{name}_"))
        .replace("(symbol \"C\"", &format!("(symbol \"{name}\""))
        .replace("\"Value\" \".Value\"", "\"Value\" \"100n\"")
        .replace(
            "\"Description\" \"Unpolarized capacitor\"",
            "\"Description\" \"CAP CER 0.1UF 50V X7R 0603\"",
        )
        .replacen(
            "\t\t(property \"Reference\"",
            &format!(
                "\t\t(property \"Manufacturer_Name\" \"Samsung Electro-Mechanics\" (at 0 0 0) (effects (hide yes)))\n\t\t(property \"Manufacturer_Part_Number\" \"{mpn}\" (at 0 0 0) (effects (hide yes)))\n\t\t(property \"Reference\""
            ),
            1,
        )
}

/// The 0603 capacitor footprint renamed `name`, naming its model where the
/// vendor put it.
fn footprint_named(name: &str) -> String {
    repository_text("footprints/g-Capacitor_SMD.pretty/C_0603_1608Metric.kicad_mod")
        .replace(
            "(footprint \"C_0603_1608Metric\"",
            &format!("(footprint \"{name}\""),
        )
        .replace(
            "${KICAD9_3DMODEL_DIR}/Capacitor_SMD.3dshapes/C_0603_1608Metric.step",
            &format!("${{KIPRJMOD}}/3D/{name}.stp"),
        )
}

fn zip_of(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let options =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (name, bytes) in entries {
        writer.start_file(*name, options).unwrap();
        writer.write_all(bytes).unwrap();
    }
    writer.finish().unwrap().into_inner()
}

const MODEL: &[u8] = b"ISO-10303-21;\nEND-ISO-10303-21;\n";

/// A SamacSys download of the new capacitor.
fn download(mpn: &str) -> Vec<u8> {
    download_with_symbol("C_New", mpn)
}

/// [`download`] with the symbol named `name`.
fn download_with_symbol(name: &str, mpn: &str) -> Vec<u8> {
    let symbol = symbol_named(name, mpn);
    let footprint = footprint_named("C_0603_New");
    zip_of(&[
        ("CL10B104/KiCad/C_New.kicad_sym", symbol.as_bytes()),
        ("CL10B104/KiCad/C_0603_New.kicad_mod", footprint.as_bytes()),
        ("CL10B104/3D/C_0603_New.stp", MODEL),
    ])
}

/// The new footprint as the Catalog stores it: its model linked to the
/// Catalog's folder.
fn stored_footprint() -> String {
    catalog::download::link_model(
        &footprint_named("C_0603_New"),
        "${GITPLM_PARTS}/3d-models/g-Capacitor_SMD.3dshapes/C_0603_New.stp",
        "C_0603_New.stp",
    )
}

fn encoded(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

fn request(zip: &[u8], name: &str) -> TeamDownloadRequest {
    TeamDownloadRequest {
        files: vec![DownloadedFile {
            file_name: name.into(),
            base64: encoded(zip),
        }],
        symbol: None,
        footprint: None,
        without_models: false,
    }
}

/// The request Add part sends for `reading`, accepting what it offers.
fn new_part(reading: &TeamDownload, ipn: &str, category: &str) -> TeamNewPartRequest {
    let chosen = reading
        .categories
        .iter()
        .find(|offered| offered.code == category)
        .unwrap();
    let mut files: Vec<_> = [reading.symbol_file.clone(), reading.footprint_file.clone()]
        .into_iter()
        .flatten()
        .collect();
    if let Some(model) = reading.model {
        files.push(reading.model_files[model].clone());
    }
    TeamNewPartRequest {
        correlation_id: None,
        ipn: ipn.into(),
        reason: "A new part".into(),
        category: category.into(),
        fields: chosen.fields.clone(),
        manufacturer_parts: vec![ManufacturerPart {
            manufacturer: reading.manufacturer.clone(),
            mpn: reading.mpn.clone(),
        }],
        symbol: format!("g-{category}:{}", reading.symbol_name),
        footprint: format!("{}:{}", reading.footprint_library, reading.footprint_name),
        files,
        from: reading.from.clone(),
        downloads: vec!["download.zip".into()],
    }
}

fn opened(part: &TeamPart, reason: &str) -> TeamEditRequest {
    TeamEditRequest {
        correlation_id: None,
        ipn: part.ipn.clone(),
        expected_details: part.details.number,
        expected_revision: part.revision.number,
        reason: reason.into(),
        category: String::new(),
        fields: part.details.fields.clone(),
        manufacturer_parts: part.details.manufacturer_parts.clone(),
        cad_fields: part.revision.cad_fields.clone(),
        files: Vec::new(),
    }
}

fn set_field(fields: &mut [Field], name: &str, value: &str) {
    fields
        .iter_mut()
        .find(|field| field.name == name)
        .unwrap_or_else(|| panic!("no field {name}"))
        .value = value.into();
}

fn team_file(kind: FileKind, name: &str, bytes: &[u8], symbol: Option<&str>) -> TeamFile {
    TeamFile {
        kind,
        file_name: name.into(),
        base64: encoded(bytes),
        symbol: symbol.map(str::to_owned),
        model_key: None,
        replaces: None,
        label: None,
    }
}

/// A team server with the resistors and capacitors imported from a temp
/// copy of the CSV catalog, an editor signed in, and the desktop's browser
/// over the copy.
struct Team {
    server: PocketBaseServer,
    admin: PocketBaseClient,
    copy: tempfile::TempDir,
    _state: tempfile::TempDir,
    _data: tempfile::TempDir,
    team: TeamCatalog,
    browser: CatalogBrowser,
}

impl Team {
    fn start() -> Self {
        let root = services::repository_root();
        let state = tempfile::tempdir().unwrap();
        let (server, admin, _superuser) = services::start(&root, state.path()).unwrap();
        let copy = tempfile::tempdir().unwrap();
        for folder in ["database", "symbols", "footprints", "3d-models"] {
            copy_folder(&root.join(folder), &copy.path().join(folder));
        }
        for csv in ["g-res", "g-cap"] {
            assert!(matches!(
                import(&admin, copy.path(), csv),
                ImportOutcome::Applied { .. }
            ));
        }
        let team = Self::member(
            &server,
            &admin,
            &state,
            "Ada Editor",
            "ada@example.test",
            true,
        );
        let data = tempfile::tempdir().unwrap();
        let browser = CatalogBrowser::open(BrowserConfig {
            catalog_root: copy.path().to_path_buf(),
            cache_dir: data.path().join("cache"),
            settings_file: data.path().join("settings.json"),
            kicad_locations: SearchLocations::Standard,
        });
        Self {
            server,
            admin,
            copy,
            _state: state,
            _data: data,
            team,
            browser,
        }
    }

    /// Another app copy, signed in as a new member.
    fn member(
        server: &PocketBaseServer,
        admin: &PocketBaseClient,
        state: &tempfile::TempDir,
        name: &str,
        email: &str,
        can_edit: bool,
    ) -> TeamCatalog {
        let member = services::add_member(admin, name, email, can_edit).unwrap();
        let url = server.url().to_owned();
        let team = TeamCatalog::open(
            state.path().join(format!("{email}.json")),
            Some(url.clone()),
            None,
        );
        let status = team.sign_in(SignInRequest {
            server_url: url,
            email: member.email.clone(),
            password: member.password.clone(),
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
        wait_until("every imported part", || {
            team.current_values().len() >= 29 + 17
        });
        team
    }

    fn second(&self, name: &str, email: &str, can_edit: bool) -> TeamCatalog {
        Self::member(
            &self.server,
            &self.admin,
            &self._state,
            name,
            email,
            can_edit,
        )
    }

    fn read(&self, zip: &[u8]) -> TeamDownload {
        self.browser
            .read_download(&request(zip, "download.zip"), &self.team)
            .unwrap()
    }

    fn add(&self, team: &TeamCatalog, request: TeamNewPartRequest) -> PublishResult {
        self.browser.add_part(request, team).unwrap()
    }

    /// Adds the new capacitor from the standard download as `ipn`.
    fn add_capacitor(&self, ipn: &str) -> TeamPart {
        let reading = self.read(&download(MPN));
        match self.add(&self.team, new_part(&reading, ipn, "cap")) {
            PublishResult::Added { part } => part,
            other => panic!("not added: {other:?}"),
        }
    }
}

/// An engineer adds a part, then works on it as on any other part: edits
/// its details, moves it to another category, replaces its footprint with a
/// file, archives and unarchives the revision that adds.
#[test]
fn an_added_part_is_edited_moved_given_a_file_and_archived() {
    let team = Team::start();
    let part = team.add_capacitor("CAP-0004-0001");
    assert_eq!(part.revision.number, 1);

    // Edit its details.
    let mut edit = opened(&part, "Voltage from the datasheet");
    set_field(&mut edit.fields, "Voltage", "50V");
    let PublishResult::Saved { revision, part, .. } = team.team.edit(edit).unwrap() else {
        panic!("not saved");
    };
    assert_eq!(revision, None);
    assert_eq!(part.details.number, 2);

    // Move it to another category (resistors here, as only two categories
    // are imported); the IPN stays.
    let mut moved = opened(&part, "Wrong category");
    moved.category = "res".into();
    let res = part.categories.iter().find(|c| c.code == "res").unwrap();
    for name in &res.fields {
        if !moved.fields.iter().any(|field| &field.name == name) {
            moved.fields.push(Field {
                name: name.clone(),
                value: String::new(),
            });
        }
    }
    let PublishResult::Saved { part, .. } = team.team.edit(moved).unwrap() else {
        panic!("not moved");
    };
    assert_eq!(part.details.category, "res");
    assert_eq!(part.ipn, "CAP-0004-0001");

    // Replace its footprint (made by Add part) with a corrected file.
    let corrected = footprint_named("C_0603_New").replacen(
        "\t(fp_line",
        "\t(fp_circle (center 0 0) (end 2 0) (stroke (width 0.3) (type solid)) (fill no) (layer \"F.SilkS\"))\n\t(fp_line",
        1,
    );
    let mut with_file = opened(&part, "Corrected silkscreen");
    with_file.files = vec![team_file(
        FileKind::Footprint,
        "C_0603_New.kicad_mod",
        corrected.as_bytes(),
        None,
    )];
    let saved = team.team.edit(with_file).unwrap();
    let PublishResult::Saved { revision, part, .. } = saved else {
        panic!("not saved: {saved:?}");
    };
    assert_eq!(revision, Some(2));

    // Archive revision 2, then unarchive it.
    let archived = team
        .team
        .archive(TeamArchiveRequest {
            correlation_id: None,
            ipn: part.ipn.clone(),
            revision: 2,
            archived: true,
            reason: "Silkscreen too thick".into(),
        })
        .unwrap();
    let PublishResult::Archived { current, .. } = archived else {
        panic!("not archived: {archived:?}");
    };
    assert_eq!(current, 1);
    let unarchived = team
        .team
        .archive(TeamArchiveRequest {
            correlation_id: None,
            ipn: part.ipn.clone(),
            revision: 2,
            archived: false,
            reason: "It was fine".into(),
        })
        .unwrap();
    let PublishResult::Archived { current, part, .. } = unarchived else {
        panic!("not unarchived: {unarchived:?}");
    };
    assert_eq!(current, 2);
    assert!(part.history.len() >= 6, "{:?}", part.history);
}

/// A part added in the app shows in a second app copy by itself; a viewer
/// sees it and cannot add one; two editors editing it at once get a
/// conflict they can resolve.
#[test]
fn others_see_an_added_part_and_edit_it_together() {
    let team = Team::start();
    let second = team.second("Bea Editor", "bea@example.test", true);
    let viewer = team.second("Vic Viewer", "vic@example.test", false);
    let part = team.add_capacitor("CAP-0004-0001");
    wait_until("the second app sees the new part", || {
        second.current_values().contains_key("CAP-0004-0001")
    });
    wait_until("the viewer sees the new part", || {
        viewer.current_values().contains_key("CAP-0004-0001")
    });
    let seen = viewer.part("CAP-0004-0001").unwrap().unwrap();
    assert_eq!(seen.details.category, "cap");
    let refused = team.add(
        &viewer,
        new_part(&team.read(&download("X1")), "CAP-0005-0001", "cap"),
    );
    assert!(
        matches!(refused, PublishResult::Refused { .. }),
        "{refused:?}"
    );

    // Both open Edit part on it; Bea saves first; Ada's save conflicts.
    let theirs_part = second.part("CAP-0004-0001").unwrap().unwrap();
    let mut mine = opened(&part, "Voltage");
    set_field(&mut mine.fields, "Voltage", "50V");
    let mut theirs = opened(&theirs_part, "Tolerance");
    set_field(&mut theirs.fields, "Tolerance", "10%");
    assert!(matches!(
        second.edit(theirs).unwrap(),
        PublishResult::Saved { .. }
    ));
    let PublishResult::Conflict(conflict) = team.team.edit(mine).unwrap() else {
        panic!("no conflict");
    };
    let PublishResult::Saved { part, .. } = team.team.edit(conflict.merged.clone()).unwrap() else {
        panic!("merged not saved");
    };
    let value = |name: &str| {
        part.details
            .fields
            .iter()
            .find(|field| field.name == name)
            .unwrap()
            .value
            .clone()
    };
    assert_eq!(
        (value("Voltage"), value("Tolerance")),
        ("50V".into(), "10%".into())
    );
}

/// The engineer prefers the Catalog's own generic capacitor symbol (or
/// KiCad's) to the one in the download, and types its reference in the
/// Symbol field; likewise a footprint reference typed by hand. The files
/// the part then does not use are not sent.
#[test]
fn an_added_part_can_use_the_catalogs_symbol_instead_of_the_downloads() {
    let team = Team::start();
    let reading = team.read(&download(MPN));
    for (ipn, symbol, footprint) in [
        ("CAP-0004-0001", "g-cap:C", NEW_FOOTPRINT),
        (
            "CAP-0005-0001",
            "Device:C",
            "Capacitor_SMD:C_0603_1608Metric",
        ),
        (
            "CAP-0006-0001",
            "g-cap:C_New",
            "g-Capacitor_SMD:C_0603_1608Metric",
        ),
    ] {
        let mut request = new_part(&reading, ipn, "cap");
        request.symbol = symbol.into();
        request.footprint = footprint.into();
        let saved = team.add(&team.team, request);
        let PublishResult::Added { part } = saved else {
            panic!("{symbol} and {footprint} not used: {saved:?}");
        };
        let used = |name: &str| {
            part.revision
                .cad_fields
                .iter()
                .find(|field| field.name == name)
                .unwrap()
                .value
                .clone()
        };
        assert_eq!(
            (used("Symbol"), used("Footprint")),
            (symbol.into(), footprint.into())
        );
    }
    // A typo in a Catalog reference is the server's to refuse plainly.
    let mut request = new_part(&reading, "CAP-0007-0001", "cap");
    request.symbol = "g-cap:C_Nwe".into();
    let PublishResult::Refused { message } = team.add(&team.team, request) else {
        panic!("not refused");
    };
    assert!(
        message.contains("g-cap:C_Nwe is not in the Catalog"),
        "{message}"
    );
}

/// Two engineers add parts at the same time with the same new footprint
/// name: identical content is shared; other content is refused plainly.
#[test]
fn two_engineers_add_the_same_new_footprint() {
    let team = Team::start();
    let second = team.second("Bea Editor", "bea@example.test", true);
    team.add_capacitor("CAP-0004-0001");
    wait_until("Bea sees it", || {
        second.current_values().contains_key("CAP-0004-0001")
    });

    // The same download for another MPN: the same footprint bytes.
    let reading = team
        .browser
        .read_download(
            &request(
                &download_with_symbol("C_Other", "CL10B104KB8NNNX"),
                "download.zip",
            ),
            &second,
        )
        .unwrap();
    let saved = team.add(&second, new_part(&reading, "CAP-0005-0001", "cap"));
    let PublishResult::Added { part } = saved else {
        panic!("same footprint not shared: {saved:?}");
    };
    let footprint = part
        .revision
        .cad_fields
        .iter()
        .find(|field| field.name == "Footprint")
        .unwrap();
    assert_eq!(footprint.value, NEW_FOOTPRINT);

    // Other content under the same name: refused, with a way forward.
    let other = footprint_named("C_0603_New").replace("(at -0.775 0)", "(at -0.8 0)");
    assert_ne!(other, footprint_named("C_0603_New"));
    let zip = zip_of(&[
        (
            "KiCad/C_Third.kicad_sym",
            symbol_named("C_Third", "OTHER1").as_bytes(),
        ),
        ("KiCad/C_0603_New.kicad_mod", other.as_bytes()),
        ("3D/C_0603_New.stp", MODEL),
    ]);
    let reading = team
        .browser
        .read_download(&request(&zip, "other.zip"), &second)
        .unwrap();
    let saved = team.add(&second, new_part(&reading, "CAP-0006-0001", "cap"));
    let PublishResult::Refused { message } = saved else {
        panic!("not refused: {saved:?}");
    };
    assert!(message.contains("already has"), "{message}");
}

/// Later, the CSV catalog gets a row for a part first added in the app:
/// the import keeps the part as it is and the app shows the CSV notice.
#[test]
fn a_csv_row_for_a_part_added_in_the_app() {
    let team = Team::start();
    team.add_capacitor("CAP-0004-0001");
    // The CSV row someone adds by hand, for the same part, with the same
    // footprint file (byte for byte what the Catalog stored) and the
    // Catalog's symbol.
    let csv = team.copy.path().join("database/g-cap.csv");
    let mut text = fs::read_to_string(&csv).unwrap();
    text.push_str(&format!(
        "\"CAP-0004-0001\",\"{MPN}\",\"Samsung Electro-Mechanics\",\"CAP CER 0.1UF 50V X7R 0603\",\"g-cap:C\",\"{NEW_FOOTPRINT}\",\"100n\",\"50V\",\"X7R\",\"±10%\",\"\",\"\",\"\",\"\",\"\",\"\",\"\",\"\"\n"
    ));
    fs::write(&csv, text).unwrap();
    fs::write(
        team.copy.path().join(NEW_FOOTPRINT_FILE),
        stored_footprint(),
    )
    .unwrap();
    let models = team.copy.path().join("3d-models/g-Capacitor_SMD.3dshapes");
    fs::create_dir_all(&models).unwrap();
    fs::write(models.join("C_0603_New.stp"), MODEL).unwrap();
    let outcome = import(&team.admin, team.copy.path(), "g-cap");
    assert!(
        !matches!(outcome, ImportOutcome::Conflict { .. }),
        "{outcome:?}"
    );
    let now = team.team.part("CAP-0004-0001").unwrap().unwrap();
    assert!(!now.csv_changes.is_empty(), "no CSV notice: {outcome:?}");
    // The CSV's footprint file is byte for byte the one the app stored, so
    // the footprint is not among the CSV changes.
    assert!(
        !now.csv_changes
            .iter()
            .any(|changes| changes.rows.iter().any(|row| row.field == "Footprint")),
        "{:?}",
        now.csv_changes
    );
}

/// Real downloads vary: upper-case extensions, a KiCad 5 footprint with an
/// unquoted model path, a .wrl model, a model name with spaces, a model
/// missing from the ZIP, only a footprint, only a symbol.
#[test]
fn odd_downloads_still_add_a_part() {
    let team = Team::start();
    let symbol = symbol_named("C_Odd", "ODD-1");
    let footprint = footprint_named("C_0603_Odd");
    let kicad5 = footprint_named("C_0603_Five")
        .replacen("(footprint \"C_0603_Five\"", "(module C_0603_Five", 1)
        .replace(
            "\"${KIPRJMOD}/3D/C_0603_Five.stp\"",
            "${KIPRJMOD}/3D/C_0603_Five.stp",
        );
    let spaced = footprint_named("C_0603_Spaced").replace(
        "${KIPRJMOD}/3D/C_0603_Spaced.stp",
        "${KIPRJMOD}/3D/C 0603 Spaced.wrl",
    );
    let owned = |entries: &[(&str, &[u8])]| -> Vec<(String, Vec<u8>)> {
        entries
            .iter()
            .map(|(name, bytes)| ((*name).to_owned(), bytes.to_vec()))
            .collect()
    };
    let cases: Vec<(&str, Vec<(String, Vec<u8>)>, bool)> = vec![
        (
            "upper-case extensions",
            owned(&[
                ("KiCad/C_ODD.KICAD_SYM", symbol.as_bytes()),
                ("KiCad/C_0603_ODD.KICAD_MOD", footprint.as_bytes()),
                ("3D/C_0603_Odd.STEP", MODEL),
            ]),
            true,
        ),
        (
            "KiCad 5 footprint",
            owned(&[
                (
                    "KiCad/C_Odd.kicad_sym",
                    symbol.replace("C_Odd", "C_Five").as_bytes(),
                ),
                ("KiCad/C_0603_Five.kicad_mod", kicad5.as_bytes()),
                ("3D/C_0603_Five.stp", MODEL),
            ]),
            true,
        ),
        (
            "a .wrl model with spaces in its name",
            owned(&[
                (
                    "C_Spaced.kicad_sym",
                    symbol.replace("C_Odd", "C_Spaced").as_bytes(),
                ),
                ("C_0603_Spaced.kicad_mod", spaced.as_bytes()),
                ("C 0603 Spaced.wrl", b"#VRML V2.0 utf8\n"),
            ]),
            true,
        ),
        (
            "no 3D model",
            owned(&[
                (
                    "C_None.kicad_sym",
                    symbol.replace("C_Odd", "C_None").as_bytes(),
                ),
                (
                    "C_0603_None.kicad_mod",
                    footprint_named("C_0603_None").as_bytes(),
                ),
            ]),
            false,
        ),
    ];
    let mut number = 10;
    for (what, entries, has_model) in cases {
        let entries: Vec<(&str, &[u8])> = entries
            .iter()
            .map(|(name, bytes)| (name.as_str(), bytes.as_slice()))
            .collect();
        let reading = team.read(&zip_of(&entries));
        assert!(reading.symbol.is_some(), "{what}: {:?}", reading.notes);
        assert!(reading.footprint.is_some(), "{what}: {:?}", reading.notes);
        assert_eq!(
            reading.model.is_some(),
            has_model,
            "{what}: {:?}",
            reading.notes
        );
        assert_eq!(
            reading.guesses[0].code, "cap",
            "{what}: {:?}",
            reading.guesses
        );
        let ipn = format!("CAP-{number:04}-0001");
        number += 1;
        let saved = team.add(&team.team, new_part(&reading, &ipn, "cap"));
        assert!(
            matches!(saved, PublishResult::Added { .. }),
            "{what}: not added: {saved:?}"
        );
        // The footprint names its model in the Catalog's folder, and the
        // model is stored there.
        if has_model {
            let stored = catalog::pocketbase::read_back(&team.admin).unwrap();
            let footprint = stored
                .cad_items
                .iter()
                .find(|item| item.key == format!("g-Capacitor_SMD:{}", reading.footprint_name))
                .unwrap();
            let revision = stored
                .cad_item_revisions
                .iter()
                .find(|revision| revision.cad_item == footprint.id)
                .unwrap();
            let model = &reading.models[reading.model.unwrap()];
            let file = model
                .split(" in ")
                .next()
                .unwrap()
                .rsplit('/')
                .next()
                .unwrap();
            let key = format!("3d-models/g-Capacitor_SMD.3dshapes/{file}");
            assert!(
                stored.cad_items.iter().any(|item| item.key == key),
                "{what}: no 3D model {key}"
            );
            assert!(
                revision
                    .snapshot
                    .contains(&format!("${{GITPLM_PARTS}}/{key}"))
                    && revision.snapshot.contains("\"kind\":\"cad-item-revision\"")
                    && !revision.snapshot.contains("\"missing\""),
                "{what}: {}",
                revision.snapshot
            );
        }
    }

    // Only a footprint: the engineer types the Catalog's symbol.
    let reading = team.read(&zip_of(&[(
        "C_0603_Lone.kicad_mod",
        footprint_named("C_0603_Lone").as_bytes(),
    )]));
    assert!(reading.symbol.is_none());
    let mut request = new_part(&reading, "CAP-0020-0001", "cap");
    request.symbol = "g-cap:C".into();
    let saved = team.add(&team.team, request);
    assert!(
        matches!(saved, PublishResult::Added { .. }),
        "footprint only: {saved:?}"
    );
    // Only a symbol: the engineer types the Catalog's footprint.
    let reading = team.read(&zip_of(&[(
        "C_Lone.kicad_sym",
        symbol.replace("C_Odd", "C_Lone").as_bytes(),
    )]));
    assert!(reading.footprint.is_none());
    let mut request = new_part(&reading, "CAP-0021-0001", "cap");
    request.footprint = "g-Capacitor_SMD:C_0603_1608Metric".into();
    let saved = team.add(&team.team, request);
    assert!(
        matches!(saved, PublishResult::Added { .. }),
        "symbol only: {saved:?}"
    );
}

/// A symbol whose fields carry long values and characters beyond ASCII.
#[test]
fn long_and_unusual_symbol_values() {
    let team = Team::start();
    let long = "Ceramic capacitor ±10 % 0.1 µF 50 V X7R, AEC-Q200 – ".repeat(20);
    let symbol = symbol_named("C_Long", "LONG-1").replace("CAP CER 0.1UF 50V X7R 0603", &long);
    let reading = team.read(&zip_of(&[
        ("C_Long.kicad_sym", symbol.as_bytes()),
        (
            "C_0603_Long.kicad_mod",
            footprint_named("C_0603_Long").as_bytes(),
        ),
    ]));
    let saved = team.add(&team.team, new_part(&reading, "CAP-0004-0001", "cap"));
    match saved {
        PublishResult::Added { part } => {
            let description = part
                .details
                .fields
                .iter()
                .find(|field| field.name == "Description")
                .unwrap();
            assert_eq!(description.value, long.trim());
        }
        PublishResult::Refused { message } => {
            // Refused is fine if it says plainly what to do.
            assert!(message.contains("Description"), "{message}");
        }
        other => panic!("{other:?}"),
    }
}

/// A big 3D model (vendors' connector models can be tens of MB): the
/// download is read (up to 100 MB), but one save sends at most 32 MB. The
/// form says so as soon as the files are read, not only after the engineer
/// has filled it in, and the refusal says what to do: save without it.
#[test]
fn a_3d_model_too_big_to_save_is_said_early_with_a_way_forward() {
    let team = Team::start();
    let mut model = b"ISO-10303-21;
"
    .to_vec();
    model.resize(40 * 1024 * 1024, b'x');
    let reading = team.read(&zip_of(&[
        ("C_Big.kicad_sym", symbol_named("C_Big", "BIG-1").as_bytes()),
        (
            "C_0603_Big.kicad_mod",
            footprint_named("C_0603_Big").as_bytes(),
        ),
        ("C_0603_Big.stp", &model),
    ]));
    assert!(
        reading.notes.iter().any(|note| note
            == "The 3D model C_0603_Big.stp is too big to save with the part: one save can send at most 32 MB. Choose No 3D model, or a smaller model."),
        "{:?}",
        reading.notes
    );
    let PublishResult::Refused { message } =
        team.add(&team.team, new_part(&reading, "CAP-0004-0001", "cap"))
    else {
        panic!("not refused");
    };
    assert!(message.contains("Choose No 3D model"), "{message}");
    // Without the model, it saves.
    let mut request = new_part(&reading, "CAP-0004-0001", "cap");
    request.files.retain(|file| file.kind != FileKind::Model3d);
    let saved = team.add(&team.team, request);
    assert!(matches!(saved, PublishResult::Added { .. }), "{saved:?}");
}

/// A part added in the app, then moved to another category in the app; the
/// CSV later gets a row for it in its first category, with other values
/// and a changed footprint file. The import keeps the part as it is; the
/// notice lists the category, the values and the footprint (keep only);
/// accepting the CSV's values moves it back.
#[test]
fn a_csv_row_for_a_part_added_then_moved_in_the_app() {
    let team = Team::start();
    let part = team.add_capacitor("CAP-0004-0001");
    let mut moved = opened(&part, "Wrong category");
    moved.category = "res".into();
    let res = part.categories.iter().find(|c| c.code == "res").unwrap();
    for name in &res.fields {
        if !moved.fields.iter().any(|field| &field.name == name) {
            moved.fields.push(Field {
                name: name.clone(),
                value: String::new(),
            });
        }
    }
    assert!(matches!(
        team.team.edit(moved).unwrap(),
        PublishResult::Saved { .. }
    ));
    let csv = team.copy.path().join("database/g-cap.csv");
    let mut text = fs::read_to_string(&csv).unwrap();
    text.push_str(&format!(
        "\"CAP-0004-0001\",\"{MPN}\",\"Samsung Electro-Mechanics\",\"CAP CER 0.1UF 50V X7R 0603 (CSV)\",\"g-cap:C_New\",\"{NEW_FOOTPRINT}\",\"100n\",\"50V\",\"X7R\",\"±10%\",\"\",\"\",\"\",\"\",\"\",\"\",\"\",\"\"
"
    ));
    fs::write(&csv, text).unwrap();
    fs::write(
        team.copy.path().join(NEW_FOOTPRINT_FILE),
        stored_footprint().replacen(
            "(tags",
            "(descr \"changed\")
	(tags",
            1,
        ),
    )
    .unwrap();
    let models = team.copy.path().join("3d-models/g-Capacitor_SMD.3dshapes");
    fs::create_dir_all(&models).unwrap();
    fs::write(models.join("C_0603_New.stp"), MODEL).unwrap();
    let outcome = import(&team.admin, team.copy.path(), "g-cap");
    assert!(
        matches!(outcome, ImportOutcome::NothingChanged { .. }),
        "{outcome:?}"
    );
    let now = team.team.part("CAP-0004-0001").unwrap().unwrap();
    assert_eq!(now.details.category, "res");
    let rows: Vec<_> = now
        .csv_changes
        .iter()
        .flat_map(|changes| changes.rows.iter())
        .collect();
    let row = |field: &str| {
        rows.iter()
            .find(|row| row.field == field)
            .unwrap_or_else(|| panic!("no {field} row in {rows:?}"))
    };
    assert_eq!(
        (row("Category").now.as_str(), row("Category").csv.as_str()),
        ("Resistors", "Capacitors")
    );
    assert!(row("Footprint").keep_only);
    // The CSV catalog's g-cap.kicad_sym lacks the symbol added in the app.
    assert!(row("Symbol").keep_only);
    let details = now.csv_changes.iter().find(|changes| !changes.cad).unwrap();
    let accept: Vec<String> = details.rows.iter().map(|row| row.field.clone()).collect();
    let reply = team
        .team
        .csv_review(TeamCsvReviewRequest {
            correlation_id: None,
            ipn: "CAP-0004-0001".into(),
            problem: details.problem.clone(),
            accept,
            expected: details.expected,
            reason: "Take the CSV's values".into(),
        })
        .unwrap();
    let PublishResult::CsvReviewed { part, revision, .. } = reply else {
        panic!("not reviewed: {reply:?}");
    };
    assert_eq!(revision, None);
    assert_eq!(part.details.category, "cap");
    // The footprint and symbol notices stay until someone keeps them.
    assert_eq!(part.csv_changes.len(), 2, "{:?}", part.csv_changes);
}

/// A part added in the app, then a CSV row for it that names the symbol
/// added in the app, which the CSV catalog's library file does not have.
/// The notice must never offer to replace the Catalog's symbol with one
/// KiCad cannot find: the row can only be kept, and the server refuses it.
#[test]
fn a_csv_row_naming_a_symbol_only_the_app_has_can_only_be_kept() {
    let team = Team::start();
    team.add_capacitor("CAP-0004-0001");
    let csv = team.copy.path().join("database/g-cap.csv");
    let mut text = fs::read_to_string(&csv).unwrap();
    text.push_str(&format!(
        "\"CAP-0004-0001\",\"{MPN}\",\"Samsung Electro-Mechanics\",\"CAP CER 0.1UF 50V X7R 0603\",\"g-cap:C_New\",\"{NEW_FOOTPRINT}\",\"100n\",\"\",\"\",\"\",\"\",\"\",\"\",\"\",\"\",\"\",\"\",\"\"
"
    ));
    fs::write(&csv, text).unwrap();
    fs::write(
        team.copy.path().join(NEW_FOOTPRINT_FILE),
        stored_footprint(),
    )
    .unwrap();
    let models = team.copy.path().join("3d-models/g-Capacitor_SMD.3dshapes");
    fs::create_dir_all(&models).unwrap();
    fs::write(models.join("C_0603_New.stp"), MODEL).unwrap();
    import(&team.admin, team.copy.path(), "g-cap");
    let now = team.team.part("CAP-0004-0001").unwrap().unwrap();
    let notice = now
        .csv_changes
        .iter()
        .find(|changes| changes.rows.iter().any(|row| row.field == "Symbol"))
        .unwrap_or_else(|| panic!("{:?}", now.csv_changes));
    let row = notice
        .rows
        .iter()
        .find(|row| row.field == "Symbol")
        .unwrap();
    assert!(row.keep_only, "{row:?}");
    assert_eq!(
        row.csv,
        "The CSV catalog does not have g-cap:C_New. It cannot be accepted here, because the Catalog has it."
    );
    let review = |accept: Vec<String>| {
        team.team
            .csv_review(TeamCsvReviewRequest {
                correlation_id: None,
                ipn: "CAP-0004-0001".into(),
                problem: notice.problem.clone(),
                accept,
                expected: notice.expected,
                reason: "Review the CSV".into(),
            })
            .unwrap()
    };
    let PublishResult::Refused { message } = review(vec!["Symbol".into()]) else {
        panic!("accepted a symbol KiCad cannot find");
    };
    assert!(
        message.contains("does not have the symbol g-cap:C_New"),
        "{message}"
    );
    let PublishResult::CsvReviewed { part, revision, .. } = review(Vec::new()) else {
        panic!("not kept");
    };
    assert_eq!(revision, None);
    assert!(part.csv_changes.is_empty(), "{:?}", part.csv_changes);
}
