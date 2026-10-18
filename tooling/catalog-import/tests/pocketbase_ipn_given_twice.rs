//! One IPN given to two different parts (seed kicad-parts-3b72): while a
//! team moves from the CSV catalog to the app, someone adds a part in the
//! app while someone else writes a CSV row with the same IPN for another
//! part. The import must never take that row as new values for the app's
//! part, or a CSV review could turn the part into a different one.
//!
//! The rule (catalog_lib.js `csvRowIsAnotherPart`): a row is another part
//! when the part with its IPN was added in the app and the row names none of
//! its MPNs. One shared MPN makes it the same part; no MPN on either side
//! makes it another part. Such a row gets an `ipn-given-to-another-part`
//! import problem that keeps it whole, the part is left exactly as it is,
//! and no CSV review can take the row.
//!
//! Each test starts its own disposable PocketBase on 127.0.0.1 and imports a
//! copy of the CSV catalog under `target/`; the repository is only read.

use catalog::pocketbase::{
    CSV_DETAILS_DIFFER, CatalogReadback, CreateCommand, CreateOutcome, CsvReviewCommand,
    CsvReviewOutcome, IPN_GIVEN_TO_ANOTHER_PART, ImportCommand, ImportOutcome, ImportRun,
    StoredImportProblem, read_back, read_import_state, submit_create, submit_csv_review,
    submit_import,
};
use catalog::{CsvCatalog, DetailField, ManufacturerPart};
use catalog_browser::csv_ipns::IpnsInCsv;
use catalog_browser::team::TeamCatalog;
use catalog_browser::{BrowserConfig, CatalogBrowser};
use catalog_import::services;
use collaboration::PocketBaseClient;
use desktop_contract::{
    CsvRowForAnotherPart, Field, PublishResult, SignInRequest, TeamNewPartRequest,
};
use kicad_integration::SearchLocations;
use native_services::PocketBaseServer;
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};

const CSV: &str = "database/g-res.csv";

/// Added in the app with MPN AR0603-1K; a CSV row gives its IPN to a
/// Panasonic resistor.
const UNRELATED: &str = "RES-0101-0001";
/// Added in the app with MPN AR0603-2K; the CSV row names the same MPN
/// with another description.
const SAME_MPN: &str = "RES-0102-0001";
/// Added in the app with two MPNs; the CSV row names only the second.
const ONE_MPN_SHARED: &str = "RES-0103-0001";
/// Added in the app with no MPN; the CSV row has one.
const PART_HAS_NO_MPN: &str = "RES-0104-0001";
/// Added in the app with an MPN; the CSV row has none.
const ROW_HAS_NO_MPN: &str = "RES-0105-0001";

struct Team {
    server: PocketBaseServer,
    admin: PocketBaseClient,
    editor: PocketBaseClient,
    /// The editor's email and password, to sign in to the desktop's team
    /// catalog.
    editor_login: (String, String),
    /// The copy of the CSV catalog the imports read, under `target/`.
    copy: tempfile::TempDir,
    _state: tempfile::TempDir,
}

impl Team {
    /// A team server with the copy's resistors imported, and an editor.
    fn start() -> Self {
        let root = services::repository_root();
        let state = tempfile::tempdir().unwrap();
        let (server, admin, _superuser) = services::start(&root, state.path()).unwrap();
        let copy = tempfile::Builder::new()
            .prefix("ipn-given-twice-")
            .tempdir_in(root.join("target"))
            .unwrap();
        for tree in ["database", "symbols", "footprints", "3d-models"] {
            copy_tree(&root.join(tree), &copy.path().join(tree));
        }
        let member = services::add_member(&admin, "Ada Editor", "ada@example.test", true).unwrap();
        let editor = PocketBaseClient::new(server.url())
            .authenticate("users", &member.email, &member.password)
            .unwrap();
        let team = Self {
            server,
            admin,
            editor,
            editor_login: (member.email.clone(), member.password.clone()),
            copy,
            _state: state,
        };
        let first = team.import("first");
        assert!(matches!(first, ImportOutcome::Applied { .. }), "{first:?}");
        team
    }

    fn root(&self) -> &Path {
        self.copy.path()
    }

    /// Imports the copy's resistors as an importer does: reading the team
    /// catalog first.
    fn import(&self, suffix: &str) -> ImportOutcome {
        let import = CsvCatalog::new(self.root()).import("g-res", None).unwrap();
        let state = read_import_state(&self.admin).unwrap();
        let mut command = ImportCommand::new(&import, "Import the resistors").expecting(&state);
        command.correlation_id.push_str(suffix);
        submit_import(&self.admin, &command).unwrap().outcome
    }

    /// Adds a resistor in the app, as Add part does, with these MPNs.
    fn add_in_app(&self, ipn: &str, description: &str, mpns: &[&str]) {
        let mut command = CreateCommand::new(ipn, "res", "A new resistor");
        command.fields = vec![
            field("Description", description),
            field("Value", "1k"),
            field("Tolerance", "1%"),
            field("Datasheet", ""),
        ];
        command.manufacturer_parts = mpns
            .iter()
            .map(|mpn| ManufacturerPart {
                manufacturer: "Acme Resistors".into(),
                mpn: (*mpn).into(),
            })
            .collect();
        command.cad_fields = vec![
            field("Symbol", "g-res:R_US"),
            field("Footprint", "g-Resistor_SMD:R_0603_1608Metric"),
        ];
        let reply = submit_create(&self.editor, &command).unwrap();
        assert!(
            matches!(reply.outcome, CreateOutcome::Applied { .. }),
            "{}",
            reply.body
        );
    }

    /// Adds a row to the copy's g-res.csv.
    fn add_row(&self, ipn: &str, mpn: &str, manufacturer: &str, description: &str) {
        let file = self.root().join(CSV);
        let mut text = fs::read_to_string(&file).unwrap();
        assert!(text.ends_with('\n'));
        text.push_str(&format!(
            "\"{ipn}\",\"{mpn}\",\"{manufacturer}\",\"{description}\",\"1k\",\"g-res:R_US\",\"g-Resistor_SMD:R_0603_1608Metric\",\"1%\",\"\",\"\",\"\",\"\"\n"
        ));
        fs::write(&file, text).unwrap();
    }

    /// Replaces exactly one occurrence of `from` in the copy's g-res.csv.
    fn change_csv(&self, from: &str, to: &str) {
        let file = self.root().join(CSV);
        let text = fs::read_to_string(&file).unwrap();
        assert_eq!(text.matches(from).count(), 1, "{from}");
        fs::write(&file, text.replacen(from, to, 1)).unwrap();
    }

    fn read(&self) -> CatalogReadback {
        read_back(&self.admin).unwrap()
    }

    /// The desktop's catalog browser over the copy, with its data in
    /// `folder`.
    fn browser(&self, folder: &Path) -> CatalogBrowser {
        CatalogBrowser::open(BrowserConfig {
            catalog_root: self.root().to_path_buf(),
            cache_dir: folder.join("cache"),
            settings_file: folder.join("settings.json"),
            kicad_locations: SearchLocations::Standard,
            kicad_config_dir: None,
            kicad_documents_dir: None,
        })
    }

    /// The desktop's team catalog, signed in as the editor.
    fn desktop(&self) -> TeamCatalog {
        let url = self.server.url().to_owned();
        let team = TeamCatalog::new();
        let (email, password) = self.editor_login.clone();
        let status = team.sign_in(SignInRequest {
            server_url: url,
            email,
            password,
        });
        assert!(status.session.is_signed_in(), "{status:?}");
        team
    }

    /// The zero-loss report of the copy against the team catalog: every
    /// field of every row is found again, here or in an import problem.
    fn assert_nothing_lost(&self, step: &str) {
        let oracle = csv_catalog_fixture::build_manifest(self.root()).unwrap();
        let report = catalog_import::report(&oracle, "g-res", &self.read(), self.root());
        assert!(
            report.is_lossless(),
            "{step}: {}",
            serde_json::to_string_pretty(&report.summary()).unwrap()
        );
    }
}

fn field(name: &str, value: &str) -> DetailField {
    DetailField {
        name: name.into(),
        value: value.into(),
    }
}

fn copy_tree(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let destination: PathBuf = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &destination);
        } else {
            fs::copy(entry.path(), destination).unwrap();
        }
    }
}

/// The latest import run of g-res.
fn latest_run(stored: &CatalogReadback) -> ImportRun {
    stored
        .audit_entries
        .iter()
        .filter(|entry| entry.csv_file == "g-res")
        .max_by_key(|entry| entry.csv_import_number)
        .and_then(|entry| entry.import_run())
        .unwrap()
}

/// The import problems of `ipn` that the latest run found, by code.
fn problems_of<'s>(stored: &'s CatalogReadback, ipn: &str) -> Vec<&'s StoredImportProblem> {
    let run = latest_run(stored);
    stored
        .problems
        .iter()
        .filter(|problem| problem.subject == ipn && run.problems.contains(&problem.id))
        .collect()
}

/// The `ipn-given-to-another-part` problem of `ipn` that the latest run
/// found.
fn flagged<'s>(stored: &'s CatalogReadback, ipn: &str) -> &'s StoredImportProblem {
    problems_of(stored, ipn)
        .into_iter()
        .find(|problem| problem.code == IPN_GIVEN_TO_ANOTHER_PART)
        .unwrap_or_else(|| panic!("{ipn} has no {IPN_GIVEN_TO_ANOTHER_PART} problem"))
}

/// The codes of the import problems that compare `ipn`'s row with the part
/// in the team catalog, which the latest run found. Problems about the row
/// alone, such as a blank MPN, are left out.
fn codes_of(stored: &CatalogReadback, ipn: &str) -> Vec<String> {
    const ABOUT_THE_ROW_ALONE: [&str; 1] = ["incomplete-main-manufacturer-part"];
    let mut codes: Vec<String> = problems_of(stored, ipn)
        .iter()
        .map(|problem| problem.code.clone())
        .filter(|code| !ABOUT_THE_ROW_ALONE.contains(&code.as_str()))
        .collect();
    codes.sort();
    codes
}

/// A part's current details and revision, as stored: numbers and snapshots.
fn state_of(stored: &CatalogReadback, ipn: &str) -> (u32, String, u32, String) {
    let part = stored.part(ipn).unwrap();
    let details = stored.part_details(&part.current_details).unwrap();
    let revision = stored.part_revision(&part.current_revision).unwrap();
    (
        details.number,
        details.snapshot.clone(),
        revision.number,
        revision.snapshot.clone(),
    )
}

#[test]
fn a_csv_row_that_gives_an_app_part_s_ipn_to_another_part_is_flagged_and_changes_nothing() {
    let team = Team::start();
    team.add_in_app(UNRELATED, "RES 1K 1% 1/10W 0603", &["AR0603-1K"]);
    team.add_in_app(SAME_MPN, "RES 2K 1% 1/10W 0603", &["AR0603-2K"]);
    team.add_in_app(
        ONE_MPN_SHARED,
        "RES 3K 1% 1/10W 0603",
        &["AR0603-3K", "RC0603FR-073KL"],
    );
    team.add_in_app(PART_HAS_NO_MPN, "RES 4K7 1% 1/10W 0603", &[]);
    team.add_in_app(ROW_HAS_NO_MPN, "RES 5K1 1% 1/10W 0603", &["AR0603-5K1"]);
    let before = team.read();

    // Someone else, working in the CSV catalog, writes rows with the same
    // IPNs.
    team.add_row(
        UNRELATED,
        "ERJ-3EKF1001V",
        "Panasonic",
        "RES 1K 1% 1/10W 0603",
    );
    team.add_row(SAME_MPN, "AR0603-2K", "Acme Resistors", "RES 2K 0.5% 0603");
    team.add_row(
        ONE_MPN_SHARED,
        "rc0603fr-073kl",
        "Yageo",
        "RES 3K 1% 1/10W 0603",
    );
    team.add_row(PART_HAS_NO_MPN, "ERJ-3EKF4701V", "Panasonic", "RES 4K7");
    team.add_row(ROW_HAS_NO_MPN, "", "", "RES 5K1 1% 1/10W 0603");
    let outcome = team.import("second");
    assert!(
        matches!(
            outcome,
            ImportOutcome::Applied { .. } | ImportOutcome::NothingChanged { .. }
        ),
        "{outcome:?}"
    );
    let stored = team.read();

    // The rows that name none of the part's MPNs, or where one side has no
    // MPN, may be other parts: each gets the new problem and nothing else.
    for ipn in [UNRELATED, PART_HAS_NO_MPN, ROW_HAS_NO_MPN] {
        assert_eq!(codes_of(&stored, ipn), [IPN_GIVEN_TO_ANOTHER_PART], "{ipn}");
        assert_eq!(state_of(&stored, ipn), state_of(&before, ipn), "{ipn}");
    }
    let unrelated = flagged(&stored, UNRELATED);
    assert_eq!(
        unrelated.message,
        format!(
            "In database/g-res.csv, the row with IPN {UNRELATED} has MPN ERJ-3EKF1001V. The team catalog already gave {UNRELATED} to a part added in the app: RES 1K 1% 1/10W 0603. That part's MPN is AR0603-1K, so nothing shows they are one part. The import left that part as it is and kept the row here. If the row is a different part, give it another IPN. If it is that part, put that part's MPN (AR0603-1K) in the row, or remove the row."
        )
    );
    // The problem keeps the whole row and both sides' MPNs.
    let detail: Value = serde_json::from_str(&unrelated.detail).unwrap();
    assert_eq!(detail["row_number"], 30);
    assert!(
        detail["raw_record"]
            .as_str()
            .unwrap()
            .starts_with(&format!("\"{UNRELATED}\",\"ERJ-3EKF1001V\"")),
        "{detail}"
    );
    assert_eq!(detail["csv_manufacturer_parts"][0]["mpn"], "ERJ-3EKF1001V");
    assert_eq!(detail["part_manufacturer_parts"][0]["mpn"], "AR0603-1K");
    assert!(flagged(&stored, PART_HAS_NO_MPN).message.contains(
        "That part has no MPN, and the row differs from it, so nothing shows they are one part."
    ),);
    assert!(
        flagged(&stored, ROW_HAS_NO_MPN)
            .message
            .contains(&format!("the row with IPN {ROW_HAS_NO_MPN} has no MPN.")),
    );

    // A shared MPN, even only one of two and written in another case, is
    // the same part: its new values wait for a CSV review, as before.
    for ipn in [SAME_MPN, ONE_MPN_SHARED] {
        assert_eq!(codes_of(&stored, ipn), [CSV_DETAILS_DIFFER], "{ipn}");
        assert_eq!(state_of(&stored, ipn), state_of(&before, ipn), "{ipn}");
    }

    // The run lists only the parts its rows are: not the other parts.
    let listed: Vec<String> = latest_run(&stored)
        .parts
        .iter()
        .map(|part| part.ipn.clone())
        .collect();
    assert!(listed.iter().any(|ipn| ipn == SAME_MPN), "{listed:?}");
    for ipn in [UNRELATED, PART_HAS_NO_MPN, ROW_HAS_NO_MPN] {
        assert!(!listed.iter().any(|listed| listed == ipn), "{ipn}");
    }
    team.assert_nothing_lost("rows for other parts");

    // The desktop shows each row on the part it has the IPN of, with the
    // next free IPN for it, and offers none of its values for review. A
    // part whose row shares an MPN shows the CSV's new values, as before.
    let data = tempfile::tempdir().unwrap();
    let desktop = team.desktop();
    let browser = team.browser(data.path());
    let unrelated = browser.team_part(UNRELATED, &desktop).unwrap().unwrap();
    assert_eq!(
        unrelated.csv_rows_for_other_parts,
        [CsvRowForAnotherPart {
            csv_file: "database/g-res.csv".into(),
            mpns: vec!["ERJ-3EKF1001V".into()],
            next_free_ipn: "RES-0106-0001".into(),
        }]
    );
    assert!(
        unrelated.csv_changes.is_empty(),
        "{:?}",
        unrelated.csv_changes
    );
    let no_mpn = desktop.part(ROW_HAS_NO_MPN).unwrap().unwrap();
    assert_eq!(
        no_mpn.csv_rows_for_other_parts[0].mpns,
        Vec::<String>::new()
    );
    let same = desktop.part(SAME_MPN).unwrap().unwrap();
    assert!(same.csv_rows_for_other_parts.is_empty());
    assert_eq!(same.csv_changes.len(), 1, "{:?}", same.csv_changes);

    // Importing the same files again finds the same problems and adds
    // nothing.
    assert!(
        matches!(team.import("again"), ImportOutcome::Noop { .. }),
        "a second import changed something"
    );

    // Given the next free IPN the app offered, the row becomes its own
    // part, and the app's part has no problem any more.
    team.change_csv(
        &format!("\"{UNRELATED}\",\"ERJ-3EKF1001V\""),
        "\"RES-0106-0001\",\"ERJ-3EKF1001V\"",
    );
    let outcome = team.import("renumbered");
    assert!(
        matches!(outcome, ImportOutcome::Applied { .. }),
        "{outcome:?}"
    );
    let stored = team.read();
    assert_eq!(codes_of(&stored, UNRELATED), Vec::<String>::new());
    assert_eq!(state_of(&stored, UNRELATED), state_of(&before, UNRELATED));
    let unrelated = desktop.part(UNRELATED).unwrap().unwrap();
    assert!(unrelated.csv_rows_for_other_parts.is_empty());
    let renumbered = stored.part("RES-0106-0001").unwrap();
    let details = stored.part_details(&renumbered.current_details).unwrap();
    assert!(
        details.snapshot.contains("ERJ-3EKF1001V"),
        "{}",
        details.snapshot
    );
    team.assert_nothing_lost("renumbered");
}

#[test]
fn a_csv_review_can_never_take_a_row_for_another_part() {
    let team = Team::start();
    team.add_in_app(UNRELATED, "RES 1K 1% 1/10W 0603", &["AR0603-1K"]);
    team.add_in_app(SAME_MPN, "RES 2K 1% 1/10W 0603", &["AR0603-2K"]);
    let before = team.read();

    // The same MPN: a CSV difference a person can review.
    team.add_row(
        UNRELATED,
        "ERJ-3EKF1001V",
        "Panasonic",
        "RES 1K 1% 1/10W 0603",
    );
    team.add_row(SAME_MPN, "AR0603-2K", "Acme Resistors", "RES 2K 0.5% 0603");
    team.import("second");
    let stored = team.read();
    let flagged = flagged(&stored, UNRELATED).id.clone();
    let reviewable = problems_of(&stored, SAME_MPN)[0].id.clone();

    // Taking the other part's row is refused: it is not a CSV change of
    // this part.
    let details_number = state_of(&before, UNRELATED).0;
    let refused = submit_csv_review(
        &team.editor,
        &CsvReviewCommand::new(
            UNRELATED,
            &flagged,
            vec!["Manufacturer parts".into(), "Description".into()],
            details_number,
            "Take the CSV's values",
        ),
    );
    let error = refused.expect_err("a review took a row for another part");
    assert!(
        error
            .to_string()
            .contains("has no such CSV change to review"),
        "{error}"
    );
    assert_eq!(team.read().counts(), stored.counts());

    // The row then changes to another MPN, so it is another part too. The
    // review the earlier run found is out of date: it cannot take the
    // row's old values onto the part either.
    team.change_csv(
        "\"AR0603-2K\",\"Acme Resistors\",\"RES 2K 0.5% 0603\"",
        "\"ERJ-3EKF2001V\",\"Panasonic\",\"RES 2K 0.5% 0603\"",
    );
    team.import("third");
    let stored = team.read();
    assert_eq!(codes_of(&stored, SAME_MPN), [IPN_GIVEN_TO_ANOTHER_PART]);
    let late = submit_csv_review(
        &team.editor,
        &CsvReviewCommand::new(
            SAME_MPN,
            &reviewable,
            vec!["Description".into()],
            state_of(&before, SAME_MPN).0,
            "Take the CSV's description",
        ),
    )
    .unwrap();
    assert!(
        matches!(late.outcome, CsvReviewOutcome::OutOfDate { .. }),
        "{}",
        late.body
    );
    assert_eq!(
        state_of(&team.read(), SAME_MPN),
        state_of(&before, SAME_MPN)
    );
    // An earlier run listed the part under this row; the row still has the
    // IPN, so nothing says the part left the file.
    assert!(
        !team
            .read()
            .problems
            .iter()
            .any(|problem| problem.code == "part-not-in-csv-file"),
    );
    team.assert_nothing_lost("a row that became another part");
}

/// A new resistor with `ipn` that uses the Catalog's own symbol and
/// footprint, as Add part sends it.
fn create_command(ipn: &str) -> CreateCommand {
    let mut command = CreateCommand::new(ipn, "res", "A new resistor");
    command.fields = vec![
        field("Description", "RES 10K 1% 1/10W 0603"),
        field("Value", "10k"),
        field("Tolerance", "1%"),
        field("Datasheet", ""),
    ];
    command.cad_fields = vec![
        field("Symbol", "g-res:R_US"),
        field("Footprint", "g-Resistor_SMD:R_0603_1608Metric"),
    ];
    command
}

#[test]
fn add_part_refuses_an_ipn_that_a_csv_row_has_even_when_the_row_made_no_part() {
    let team = Team::start();
    // A row with one field too many cannot become a part, and neither can a
    // row whose IPN has a space after it, as spreadsheets leave. Both IPNs
    // are meant for a part.
    const EXTRA_FIELDS: &str = "RES-0107-0001";
    const TRAILING_SPACE: &str = "RES-0110-0001";
    let file = team.root().join(CSV);
    let original = fs::read_to_string(&file).unwrap();
    let mut text = original.clone();
    text.push_str(&format!(
        "\"{EXTRA_FIELDS}\",\"ERJ-3EKF1002V\",\"Panasonic\",\"RES 10K\",\"10k\",\"g-res:R_US\",\"g-Resistor_SMD:R_0603_1608Metric\",\"1%\",\"\",\"\",\"\",\"\",\"\",\"\",\"\",\"\",\"one too many\"\n\"{TRAILING_SPACE} \",\"ERJ-3EKF1102V\",\"Panasonic\",\"RES 11K\",\"11k\",\"g-res:R_US\",\"g-Resistor_SMD:R_0603_1608Metric\",\"1%\",\"\"\n"
    ));
    fs::write(&file, text).unwrap();
    team.import("kept-rows");
    let stored = team.read();
    assert!(stored.part(EXTRA_FIELDS).is_none());
    assert!(stored.part(TRAILING_SPACE).is_none());

    for ipn in [EXTRA_FIELDS, TRAILING_SPACE] {
        let reply = submit_create(&team.editor, &create_command(ipn)).unwrap();
        let CreateOutcome::IpnTaken {
            message,
            added_by_you,
            csv_file,
            ..
        } = &reply.outcome
        else {
            panic!("{ipn} not refused: {}", reply.body);
        };
        assert!(!added_by_you);
        assert_eq!(csv_file, CSV);
        assert_eq!(
            message,
            &format!(
                "A row of database/g-res.csv has the IPN {ipn}, but the import could not make a part from it. An IPN is never reused."
            )
        );
    }
    assert_eq!(team.read().counts(), stored.counts());

    // The desktop passes the reason on, even when its own CSV folder lacks
    // the row.
    let desktop = team.desktop();
    let request = TeamNewPartRequest {
        correlation_id: None,
        ipn: EXTRA_FIELDS.into(),
        reason: "A new resistor".into(),
        category: "res".into(),
        fields: ["Description", "Value", "Tolerance", "Datasheet"]
            .iter()
            .map(|name| Field {
                name: (*name).into(),
                value: String::new(),
            })
            .collect(),
        manufacturer_parts: Vec::new(),
        symbol: "g-res:R_US".into(),
        footprint: "g-Resistor_SMD:R_0603_1608Metric".into(),
        files: Vec::new(),
        from: String::new(),
        downloads: Vec::new(),
    };
    let saved = desktop.add_part(request, &IpnsInCsv::default()).unwrap();
    let PublishResult::IpnTaken { message, .. } = saved else {
        panic!("not refused: {saved:?}");
    };
    assert!(
        message.starts_with(&format!(
            "A row of database/g-res.csv has the IPN {EXTRA_FIELDS}, but the import could not make a part from it, so nothing was saved."
        )),
        "{message}"
    );

    // Once the latest import no longer finds those rows, their IPNs are
    // free again: an older run's row may be gone.
    fs::write(&file, original).unwrap();
    team.import("rows-gone");
    for ipn in [EXTRA_FIELDS, TRAILING_SPACE] {
        let reply = submit_create(&team.editor, &create_command(ipn)).unwrap();
        assert!(
            matches!(reply.outcome, CreateOutcome::Applied { .. }),
            "{ipn}: {}",
            reply.body
        );
    }
}

#[test]
fn an_exact_copy_of_an_app_part_is_that_part_and_a_placeholder_mpn_is_no_mpn() {
    let team = Team::start();
    // A part with no MPN, such as a test point, copied exactly into the
    // CSV catalog: the same part details and the same symbol and footprint.
    const COPY: &str = "RES-0108-0001";
    team.add_in_app(COPY, "RES 1K 1% 1/10W 0603", &[]);
    // Parts whose only MPN is a placeholder: no MPN matches another.
    const PLACEHOLDER: &str = "RES-0109-0001";
    team.add_in_app(PLACEHOLDER, "RES 9K1 1% 1/10W 0603", &["TBD"]);
    let before = team.read();
    let file = team.root().join(CSV);
    let mut text = fs::read_to_string(&file).unwrap();
    text.push_str(&format!(
        "\"{COPY}\",\"\",\"\",\"RES 1K 1% 1/10W 0603\",\"1k\",\"g-res:R_US\",\"g-Resistor_SMD:R_0603_1608Metric\",\"1%\",\"\"\n"
    ));
    fs::write(&file, text).unwrap();
    team.add_row(PLACEHOLDER, "n/a", "Acme Resistors", "RES 9K1 0.5% 0603");
    team.import("copies");
    let stored = team.read();

    // The exact copy is the part: nothing is flagged, nothing changes.
    assert_eq!(
        codes_of(&stored, COPY),
        ["part-last-saved-without-csv-source"]
    );
    assert_eq!(state_of(&stored, COPY), state_of(&before, COPY));
    // TBD and n/a are no MPNs, so nothing shows the row is the part.
    assert_eq!(codes_of(&stored, PLACEHOLDER), [IPN_GIVEN_TO_ANOTHER_PART]);
    assert!(
        flagged(&stored, PLACEHOLDER)
            .message
            .contains("That part has no MPN"),
        "{}",
        flagged(&stored, PLACEHOLDER).message
    );
    assert_eq!(
        state_of(&stored, PLACEHOLDER),
        state_of(&before, PLACEHOLDER)
    );
    team.assert_nothing_lost("an exact copy and a placeholder MPN");
}
