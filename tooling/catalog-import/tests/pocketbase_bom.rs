//! Excluded from BOM follows the CAD of each new part revision
//! (kicad-parts-ccc0): the server works it out from the stored bytes of the
//! CAD item revisions a new revision binds, for imports, edits and CSV
//! reviews alike, and an existing revision never changes. Each test starts
//! its own disposable PocketBase on 127.0.0.1 and imports temp copies of the
//! CSV catalog; the repository is only read.

use catalog::pocketbase::{
    CSV_CAD_DIFFERS, CatalogReadback, CsvReviewCommand, EditCommand, ImportCommand, ImportOutcome,
    StoredImportProblem, read_back, read_import_state, submit_csv_review, submit_edit,
    submit_import,
};
use catalog::{CsvCatalog, CsvImport, PartRevision, sha256_hex};
use catalog_import::services;
use collaboration::{CollabError, PocketBaseClient};
use native_services::PocketBaseServer;
use std::fs;
use std::path::Path;

/// A resistor the tests edit.
const EDITED: &str = "RES-0000-0470";
/// A resistor only the CSV changes.
const IMPORTED: &str = "RES-0000-1000";
/// Another resistor only the CSV changes.
const OTHER: &str = "RES-0000-1500";
/// The test point, whose symbol g-art:TestPoint says (in_bom no).
const TEST_POINT: &str = "ART-0000-0001";
/// A footprint whose (attr ...) has exclude_from_bom.
const NO_BOM_FOOTPRINT: &str = "g-no_bom:TestPoint_Pad_D1.0mm";

struct Team {
    server: PocketBaseServer,
    admin: PocketBaseClient,
    copy: tempfile::TempDir,
    _state: tempfile::TempDir,
}

impl Team {
    /// A team server with the test points and the resistors imported.
    fn start() -> Self {
        let team = Self::empty(None);
        assert_eq!(team.import("g-art"), "applied");
        assert_eq!(team.import("g-res"), "applied");
        team
    }

    /// A team server with nothing imported yet and a copy of the CSV
    /// catalog. With `extra_hook`, the server also loads that hook file.
    fn empty(extra_hook: Option<&str>) -> Self {
        let root = services::repository_root();
        let state = tempfile::tempdir().unwrap();
        let mut options = services::options(&root, state.path());
        if let Some(hook) = extra_hook {
            let hooks = state.path().join("hooks");
            fs::create_dir_all(&hooks).unwrap();
            for entry in fs::read_dir(root.join("infra/pocketbase/pb_hooks")).unwrap() {
                let entry = entry.unwrap();
                fs::copy(entry.path(), hooks.join(entry.file_name())).unwrap();
            }
            fs::write(hooks.join("test_bom.pb.js"), hook).unwrap();
            options.hooks_dir = hooks;
        }
        let superuser = native_services::Superuser::generate();
        let binary = native_services::pocketbase_binary(&root).unwrap();
        let server = PocketBaseServer::start(&binary, &options, &superuser).unwrap();
        let admin = PocketBaseClient::new(server.url())
            .authenticate("_superusers", &superuser.email, &superuser.password)
            .unwrap();
        let copy = tempfile::tempdir().unwrap();
        for tree in ["database", "symbols", "footprints", "3d-models"] {
            copy_tree(&root.join(tree), &copy.path().join(tree));
        }
        Self {
            server,
            admin,
            copy,
            _state: state,
        }
    }

    fn plan(&self, csv_file: &str) -> CsvImport {
        CsvCatalog::new(self.copy.path())
            .import(csv_file, None)
            .unwrap()
    }

    /// Imports `csv_file` from the copy as an importer does; the zero-loss
    /// report must hold.
    fn import(&self, csv_file: &str) -> &'static str {
        let outcome = self.import_only(csv_file);
        let oracle = csv_catalog_fixture::build_manifest(self.copy.path()).unwrap();
        let report = catalog_import::report(&oracle, csv_file, &self.read(), self.copy.path());
        assert!(
            report.is_lossless(),
            "{}",
            serde_json::to_string_pretty(&report.summary()).unwrap()
        );
        outcome
    }

    /// Imports `csv_file` from the copy without the zero-loss report (whose
    /// oracle reads only UTF-8 CAD files).
    fn import_only(&self, csv_file: &str) -> &'static str {
        let import = self.plan(csv_file);
        let state = read_import_state(&self.admin).unwrap();
        let command = ImportCommand::new(&import, "Import").expecting(&state);
        let reply = submit_import(&self.admin, &command).unwrap();
        let outcome = match reply.outcome {
            ImportOutcome::Applied { .. } => "applied",
            ImportOutcome::NothingChanged { .. } => "nothing_changed",
            ImportOutcome::Noop { .. } => "noop",
            _ => panic!("the import did not go through: {}", reply.body),
        };
        outcome
    }

    /// Replaces exactly one `from` in the row of `ipn` in the copy's
    /// g-res.csv.
    fn change_row(&self, ipn: &str, from: &str, to: &str) {
        let file = self.copy.path().join("database/g-res.csv");
        let text = fs::read_to_string(&file).unwrap();
        let row = text
            .split_inclusive('\n')
            .find(|line| line.starts_with(&format!("\"{ipn}\"")))
            .unwrap()
            .to_owned();
        assert_eq!(row.matches(from).count(), 1, "{from:?} in {row}");
        fs::write(&file, text.replacen(&row, &row.replacen(from, to, 1), 1)).unwrap();
    }

    fn read(&self) -> CatalogReadback {
        read_back(&self.admin).unwrap()
    }
}

fn copy_tree(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let destination = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &destination);
        } else {
            fs::copy(entry.path(), destination).unwrap();
        }
    }
}

/// The part's current revision: its number and document.
fn current(stored: &CatalogReadback, ipn: &str) -> (u32, PartRevision) {
    let part = stored.part(ipn).unwrap();
    let revision = stored.part_revision(&part.current_revision).unwrap();
    (
        revision.number,
        serde_json::from_str(&revision.snapshot).unwrap(),
    )
}

/// The stored snapshot of revision `number` of `ipn`.
fn snapshot(stored: &CatalogReadback, ipn: &str, number: u32) -> String {
    let part = stored.part(ipn).unwrap();
    stored
        .part_revisions
        .iter()
        .find(|revision| revision.part == part.id && revision.number == number)
        .unwrap()
        .snapshot
        .clone()
}

/// Sets the Symbol or Footprint of `ipn` with a CAD edit and returns the
/// new current revision.
fn edit_cad(team: &Team, ipn: &str, field: &str, value: &str) -> (u32, PartRevision) {
    let stored = team.read();
    let (number, revision) = current(&stored, ipn);
    let mut command = EditCommand::new(ipn, &format!("Use {value}")).editing_cad(&revision, number);
    assert!(command.set_field(field, value));
    let reply = submit_edit(&team.admin, &command).unwrap();
    assert_eq!(reply.status, 200, "{}", reply.body);
    current(&team.read(), ipn)
}

#[test]
fn an_edit_recalculates_excluded_from_bom_from_the_new_cad() {
    let team = Team::start();
    let stored = team.read();
    // The import set it for the test point, from its symbol's (in_bom no).
    assert!(current(&stored, TEST_POINT).1.excluded_from_bom);
    assert!(!current(&stored, EDITED).1.excluded_from_bom);
    let first = snapshot(&stored, EDITED, 1);

    // Switching to the test point symbol sets it; revision 1 is unchanged.
    let (number, revision) = edit_cad(&team, EDITED, "Symbol", "g-art:TestPoint");
    assert_eq!(number, 2);
    assert!(revision.excluded_from_bom);
    assert!(snapshot(&team.read(), EDITED, 2).ends_with(r#","excluded_from_bom":true}"#));
    assert_eq!(
        snapshot(&team.read(), EDITED, 1),
        first,
        "revision 1 never changes"
    );

    // Switching back clears it; revision 2 keeps it.
    let (number, revision) = edit_cad(&team, EDITED, "Symbol", "g-pas:R_US");
    assert_eq!(number, 3);
    assert!(!revision.excluded_from_bom);
    let stored = team.read();
    assert!(!snapshot(&stored, EDITED, 3).contains("excluded_from_bom"));
    let second: PartRevision = serde_json::from_str(&snapshot(&stored, EDITED, 2)).unwrap();
    assert!(second.excluded_from_bom, "revision 2 never changes");

    // A symbol from KiCad's own library is not read, so it gives no
    // evidence either way.
    let (_, revision) = edit_cad(&team, EDITED, "Symbol", "Device:R");
    assert!(!revision.excluded_from_bom);

    // Footprint choices: the import keeps disagreeing ones in an import
    // problem and leaves the setting unset...
    team.change_row(
        IMPORTED,
        "\"g-res:R_0603_1608Metric\"",
        &format!("\"g-res:R_0603_1608Metric;{NO_BOM_FOOTPRINT}\""),
    );
    assert_eq!(team.import("g-res"), "applied");
    let stored = team.read();
    assert!(!current(&stored, IMPORTED).1.excluded_from_bom);
    assert!(stored.problems.iter().any(|problem| {
        problem.code == "footprint-choices-disagree-on-bom" && problem.subject == IMPORTED
    }));
    // ...an edit to only the marked footprint sets it, even with KiCad's
    // own symbol...
    let (_, revision) = edit_cad(&team, EDITED, "Footprint", NO_BOM_FOOTPRINT);
    assert!(revision.excluded_from_bom);
    // ...and an edit to disagreeing choices leaves it unset.
    let (_, revision) = edit_cad(
        &team,
        EDITED,
        "Footprint",
        &format!("g-res:R_0603_1608Metric;{NO_BOM_FOOTPRINT}"),
    );
    assert!(!revision.excluded_from_bom);

    // The zero-loss report checks every revision, including those edits
    // added: a revision 2 that lost the setting is a loss.
    let mut tampered = team.read();
    let part = tampered.part(EDITED).unwrap().id.clone();
    let second = tampered
        .part_revisions
        .iter_mut()
        .find(|revision| revision.part == part && revision.number == 2)
        .unwrap();
    second.snapshot = second.snapshot.replace(r#","excluded_from_bom":true"#, "");
    let oracle = csv_catalog_fixture::build_manifest(team.copy.path()).unwrap();
    let report = catalog_import::report(&oracle, "g-res", &tampered, team.copy.path());
    assert!(
        report
            .problems
            .iter()
            .any(|problem| problem
                .contains(&format!("{EDITED} revision 2: Excluded from BOM is false"))),
        "{:?}",
        report.problems
    );
    team.server.stop();
}

/// The CSV CAD problem about `ipn` that the latest import run found.
fn cad_problem(stored: &CatalogReadback, ipn: &str) -> StoredImportProblem {
    let run = stored
        .audit_entries
        .iter()
        .filter(|entry| entry.csv_import_number > 0)
        .max_by_key(|entry| entry.csv_import_number)
        .and_then(|entry| entry.import_run())
        .unwrap();
    stored
        .problems
        .iter()
        .find(|problem| {
            problem.code == CSV_CAD_DIFFERS
                && problem.subject == ipn
                && run.problems.contains(&problem.id)
        })
        .cloned()
        .unwrap()
}

#[test]
fn imports_and_csv_reviews_recalculate_it_the_same_way() {
    let team = Team::start();
    // The edited resistor's CAD is now an app edit (revision 2).
    let (number, _) = edit_cad(&team, EDITED, "Symbol", "Device:R");
    assert_eq!(number, 2);
    // The CSV gives both resistors the test point symbol.
    team.change_row(EDITED, "\"g-pas:R_US\"", "\"g-art:TestPoint\"");
    team.change_row(IMPORTED, "\"g-pas:R_US\"", "\"g-art:TestPoint\"");
    assert_eq!(team.import("g-res"), "applied");
    let stored = team.read();
    // The import applies it to the resistor nobody edited, and sets it.
    let (number, revision) = current(&stored, IMPORTED);
    assert_eq!(number, 2);
    assert!(revision.excluded_from_bom);
    // Accepting the CSV symbol on the edited resistor sets it the same way.
    let problem = cad_problem(&stored, EDITED);
    let review = CsvReviewCommand::new(
        EDITED,
        &problem.id,
        vec!["Symbol".into()],
        2,
        "Accept CSV values from g-res.csv",
    );
    let reply = submit_csv_review(&team.admin, &review).unwrap();
    assert_eq!(reply.status, 200, "{}", reply.body);
    let stored = team.read();
    let (number, accepted) = current(&stored, EDITED);
    assert_eq!(number, 3);
    assert!(accepted.excluded_from_bom);
    assert_eq!(
        accepted.cad_references, revision.cad_references,
        "the same CAD as the imported resistor"
    );
    team.server.stop();
}

#[test]
fn an_import_cannot_claim_a_different_setting_than_its_cad_says() {
    let team = Team::start();
    let before = team.read();
    for (ipn, claim, says) in [
        (EDITED, true, "cannot be Excluded from BOM"),
        (TEST_POINT, false, "must be Excluded from BOM"),
    ] {
        let csv_file = if ipn == TEST_POINT { "g-art" } else { "g-res" };
        let mut import = team.plan(csv_file);
        let part = import
            .parts
            .iter_mut()
            .find(|part| part.ipn.as_str() == ipn)
            .unwrap();
        part.revision.excluded_from_bom = claim;
        part.revision_json = part.revision.canonical_json();
        part.revision_sha256 = sha256_hex(&part.revision_json);
        let state = read_import_state(&team.admin).unwrap();
        let command = ImportCommand::new(&import, "Import").expecting(&state);
        match submit_import(&team.admin, &command) {
            Err(CollabError::Http {
                status: 400,
                message,
            }) => assert!(message.contains(says), "{ipn}: {message}"),
            other => panic!("{ipn}: expected 400, got {other:?}"),
        }
    }
    assert_eq!(team.read(), before, "nothing was written");
    team.server.stop();
}

/// A route that answers what the server's reader says about some bytes.
const BOM_MARK_HOOK: &str = r#"
routerAdd("POST", "/api/test/bom-mark", (e) => {
  const lib = require(`${__hooks}/catalog_lib.js`);
  const body = e.requestInfo().body;
  return e.json(200, { mark: lib.bomMark(body.role, lib.base64Decode(body.base64)) });
}, $apis.requireSuperuserAuth());
"#;

#[test]
fn the_server_reads_bom_flags_exactly_as_the_importer_does() {
    use kicad_integration::library_file::{
        footprint_says_excluded_from_bom, symbol_entries_say_not_in_bom,
    };
    let team = Team::empty(Some(BOM_MARK_HOOK));
    let footprint = |attr: &[u8]| -> Vec<u8> {
        let mut bytes = b"(footprint \"F\"\n\t(version 20241229)\n\t(descr \"plain\")\n\t".to_vec();
        bytes.extend_from_slice(attr);
        bytes.extend_from_slice(b"\n)\n");
        bytes
    };
    let symbol = |inner: &[u8]| -> Vec<u8> {
        let mut bytes = b"(symbol \"TP\"\n\t\t".to_vec();
        bytes.extend_from_slice(inner);
        bytes.extend_from_slice(b"\n\t)");
        bytes
    };
    let latin1 = b"(footprint \"F\" (descr \"caf\xe9\") (attr smd exclude_from_bom))".to_vec();
    let cases: Vec<(&str, &str, Vec<u8>)> = vec![
        (
            "footprint",
            "flagged",
            footprint(b"(attr smd exclude_from_bom)"),
        ),
        ("footprint", "not flagged", footprint(b"(attr smd)")),
        ("footprint", "a Latin-1 byte", latin1),
        (
            "footprint",
            "NBSP",
            footprint(b"(attr smd\xc2\xa0exclude_from_bom)"),
        ),
        (
            "footprint",
            "VT",
            footprint(b"(attr smd\x0bexclude_from_bom)"),
        ),
        (
            "footprint",
            "FF",
            footprint(b"(attr smd\x0cexclude_from_bom)"),
        ),
        (
            "footprint",
            "CR",
            footprint(b"(attr smd\rexclude_from_bom)"),
        ),
        (
            "footprint",
            "U+FEFF",
            footprint(b"(attr smd\xef\xbb\xbfexclude_from_bom)"),
        ),
        (
            "footprint",
            "U+2028",
            footprint(b"(attr smd\xe2\x80\xa8exclude_from_bom)"),
        ),
        (
            "footprint",
            "a string",
            footprint(b"(attr smd \"exclude_from_bom\")"),
        ),
        (
            "footprint",
            "nested",
            footprint(b"(attr (smd exclude_from_bom))"),
        ),
        (
            "footprint",
            "elsewhere",
            footprint(b"(fp_text value \"x\" exclude_from_bom)"),
        ),
        (
            "footprint",
            "module",
            b"(module F (attr exclude_from_bom))".to_vec(),
        ),
        (
            "footprint",
            "unbalanced",
            b"(footprint F (attr exclude_from_bom)".to_vec(),
        ),
        (
            "footprint",
            "a byte order mark",
            b"\xef\xbb\xbf(footprint F (attr exclude_from_bom))".to_vec(),
        ),
        (
            "footprint",
            "a trailing backslash",
            b"(footprint F (attr exclude_from_bom) \"\\".to_vec(),
        ),
        ("symbol", "in_bom no", symbol(b"(in_bom no)")),
        ("symbol", "in_bom yes", symbol(b"(in_bom yes)")),
        ("symbol", "VT", symbol(b"(in_bom\x0bno)")),
        ("symbol", "FF", symbol(b"(in_bom\x0cno)")),
        ("symbol", "NBSP", symbol(b"(in_bom\xc2\xa0no)")),
        ("symbol", "a quoted head", symbol(b"(\"in_bom\" \"no\")")),
        (
            "symbol",
            "a Latin-1 byte",
            symbol(b"(property \"x\" \"caf\xe9\") (in_bom no)"),
        ),
        (
            "symbol",
            "only a unit",
            symbol(b"(symbol \"TP_0_1\" (in_bom no))"),
        ),
        ("symbol", "unnamed", b"(symbol (in_bom no))".to_vec()),
        (
            "symbol",
            "an unnamed entry beside a marked one",
            b"(symbol \"TP\" (in_bom no)) (symbol (in_bom yes))".to_vec(),
        ),
        (
            "symbol",
            "a parenthesis in a string",
            symbol(b"(property \"x\" \"a ) b\") (in_bom no)"),
        ),
    ];
    for (role, what, bytes) in cases {
        let rust = if role == "symbol" {
            symbol_entries_say_not_in_bom(&bytes)
        } else {
            footprint_says_excluded_from_bom(&bytes)
        };
        let reply = team
            .admin
            .post_json(
                "/api/test/bom-mark",
                &serde_json::json!({ "role": role, "base64": catalog::pocketbase::base64(&bytes) }),
            )
            .unwrap();
        assert_eq!(reply.status, 200, "{role} {what}: {}", reply.body);
        let server: serde_json::Value = serde_json::from_str(&reply.body).unwrap();
        assert_eq!(server["mark"], rust, "{role} with {what}");
    }
    team.server.stop();
}

#[test]
fn non_utf8_and_unusual_whitespace_import_and_edit_as_the_importer_reads_them() {
    let team = Team::empty(None);
    let pretty = team.copy.path().join("footprints/g-res.pretty");
    // Flagged, with a Latin-1 byte in its description.
    fs::write(
        pretty.join("NonUtf8.kicad_mod"),
        b"(footprint \"NonUtf8\"\n\t(version 20241229)\n\t(descr \"r\xe9sistance\")\n\t(attr smd exclude_from_bom)\n)\n",
    )
    .unwrap();
    // A no-break space is not a separator, so this is not flagged.
    fs::write(
        pretty.join("Nbsp.kicad_mod"),
        "(footprint \"Nbsp\"\n\t(version 20241229)\n\t(attr smd\u{a0}exclude_from_bom)\n)\n",
    )
    .unwrap();
    team.change_row(IMPORTED, "\"g-res:R_0603_1608Metric\"", "\"g-res:NonUtf8\"");
    team.change_row(OTHER, "\"g-res:R_0603_1608Metric\"", "\"g-res:Nbsp\"");
    // The CSV catalog fixture's oracle reads only UTF-8 CAD, so the
    // zero-loss report is left out here.
    assert_eq!(team.import_only("g-art"), "applied");
    assert_eq!(team.import_only("g-res"), "applied");
    let stored = team.read();
    assert!(current(&stored, IMPORTED).1.excluded_from_bom);
    assert!(!current(&stored, OTHER).1.excluded_from_bom);
    // An edit reads the same stored bytes the same way.
    let (_, revision) = edit_cad(&team, EDITED, "Footprint", "g-res:NonUtf8");
    assert!(revision.excluded_from_bom);
    let (_, revision) = edit_cad(&team, EDITED, "Footprint", "g-res:Nbsp");
    assert!(!revision.excluded_from_bom);
    team.server.stop();
}

#[test]
fn an_import_cannot_bind_a_reference_to_another_cad_item() {
    let team = Team::start();
    let before = team.read();
    let mut import = team.plan("g-res");
    let part = import
        .parts
        .iter_mut()
        .find(|part| part.ipn.as_str() == IMPORTED)
        .unwrap();
    // Point the symbol reference at the footprint's CAD item revision.
    let footprint = part
        .revision
        .cad_references
        .iter()
        .find(|reference| reference.role == catalog::CadReferenceRole::Footprint)
        .unwrap()
        .target
        .clone();
    let symbol = part
        .revision
        .cad_references
        .iter_mut()
        .find(|reference| reference.role == catalog::CadReferenceRole::Symbol)
        .unwrap();
    symbol.target = footprint;
    part.revision_json = part.revision.canonical_json();
    part.revision_sha256 = sha256_hex(&part.revision_json);
    let state = read_import_state(&team.admin).unwrap();
    let command = ImportCommand::new(&import, "Import").expecting(&state);
    match submit_import(&team.admin, &command) {
        Err(CollabError::Http {
            status: 400,
            message,
        }) => assert!(
            message.contains("must bind the CAD item of that symbol"),
            "{message}"
        ),
        other => panic!("expected 400, got {other:?}"),
    }
    assert_eq!(team.read(), before, "nothing was written");
    team.server.stop();
}
