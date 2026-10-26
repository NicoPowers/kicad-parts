//! CSV rows for parts added in the app (QA pass 2, 2026-10-12): a CSV row
//! for an added part, for one added then moved, and one naming a symbol only
//! the app has, which can only be kept. Split from
//! `desktop_team_workflows.rs` (kicad-parts-fbc2): these test the CSV
//! import, which kicad-parts-e257 removes with the CSV catalog. They import
//! temp copies of the resistors and capacitors.

use base64::Engine;
use catalog::CsvCatalog;
use catalog::pocketbase::{ImportCommand, ImportOutcome, read_import_state, submit_import};
use catalog_browser::team::TeamCatalog;
use catalog_browser::{BrowserConfig, CatalogBrowser};
use catalog_import::services;
use collaboration::PocketBaseClient;
use desktop_contract::{
    DownloadedFile, Field, LiveUpdates, ManufacturerPart, PublishResult, SignInRequest,
    TeamCsvReviewRequest, TeamDownload, TeamDownloadRequest, TeamEditRequest, TeamNewPartRequest,
    TeamPart, TeamSession,
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
        picks: Vec::new(),
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
        // The download's package group, else the one its category suggests.
        footprint: format!(
            "{}:{}",
            if reading.footprint_library.is_empty() {
                &chosen.footprint_library
            } else {
                &reading.footprint_library
            },
            reading.footprint_name
        ),
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

/// A team server with the resistors and capacitors imported from a temp
/// copy of the CSV catalog, an editor signed in, and the desktop's browser
/// over the copy.
struct Team {
    _server: PocketBaseServer,
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
        let team = Self::member(&server, &admin, "Ada Editor", "ada@example.test", true);
        let data = tempfile::tempdir().unwrap();
        let browser = CatalogBrowser::open(BrowserConfig {
            catalog_root: copy.path().to_path_buf(),
            cache_dir: data.path().join("cache"),
            settings_file: data.path().join("settings.json"),
            kicad_locations: SearchLocations::Standard,
            kicad_config_dir: None,
            kicad_documents_dir: None,
        });
        Self {
            _server: server,
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
        name: &str,
        email: &str,
        can_edit: bool,
    ) -> TeamCatalog {
        let member = services::add_member(admin, name, email, can_edit).unwrap();
        let url = server.url().to_owned();
        let team = TeamCatalog::new();
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
