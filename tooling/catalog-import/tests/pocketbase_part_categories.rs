//! Categories and remembered matches through the protected Catalog routes
//! (seed kicad-parts-ae44): each CSV file makes one category, which later
//! imports change only by its columns; a part's category is part of its
//! details, so changing it in an edit keeps the IPN and every field and adds
//! the new category's fields; the person's choice is remembered with the
//! part's clues, and nothing else ever makes a remembered match.
//!
//! Each test starts its own disposable PocketBase on 127.0.0.1 and imports a
//! temp copy of the CSV catalog; the repository is only read.

use catalog::category::{footprint_family, reference_letter};
use catalog::guess::{ClueKind, Clues, guess_category};
use catalog::pocketbase::{
    CSV_DETAILS_DIFFER, CatalogReadback, CsvReviewCommand, EDIT_ROUTE, EditCommand, ImportCommand,
    ImportOutcome, PublishOutcome, current_categories, read_back, read_import_state,
    remembered_matches, submit_csv_review, submit_edit, submit_import,
};
use catalog::{CategoryDetails, CsvCatalog, DifferenceKind, PartDetails, compare_details};
use catalog_import::services;
use collaboration::PocketBaseClient;
use native_services::PocketBaseServer;
use std::fs;
use std::path::Path;

const IPN: &str = "RES-0000-0470";
const OTHER: &str = "RES-0000-1000";

/// Copies the CSV catalog trees the import and the zero-loss oracle read
/// into `target`.
fn copy_catalog(target: &Path) {
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
    for tree in ["database", "symbols", "footprints", "3d-models"] {
        copy_tree(&services::repository_root().join(tree), &target.join(tree));
    }
}

struct Team {
    server: PocketBaseServer,
    admin: PocketBaseClient,
    /// A copy of the CSV catalog, which the tests change.
    copy: tempfile::TempDir,
    state: tempfile::TempDir,
}

impl Team {
    /// A team server with the resistors and capacitors imported.
    fn start() -> Self {
        Self::start_with(None)
    }

    /// With `extra_hook`, also loads that hook file (for tests that save
    /// records directly, as buggy server code might).
    fn start_with(extra_hook: Option<&str>) -> Self {
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
            fs::write(hooks.join("test_category_match.pb.js"), hook).unwrap();
            options.hooks_dir = hooks;
        }
        let superuser = native_services::Superuser::generate();
        let binary = native_services::pocketbase_binary(&root).unwrap();
        let server = PocketBaseServer::start(&binary, &options, &superuser).unwrap();
        let admin = PocketBaseClient::new(server.url())
            .authenticate("_superusers", &superuser.email, &superuser.password)
            .unwrap();
        let copy = tempfile::tempdir().unwrap();
        copy_catalog(copy.path());
        let team = Self {
            server,
            admin,
            copy,
            state,
        };
        assert_eq!(team.import("g-res"), "applied");
        assert_eq!(team.import("g-cap"), "applied");
        team
    }

    fn member(&self, name: &str, can_edit: bool) -> PocketBaseClient {
        let email = format!("{}@example.test", name.to_lowercase().replace(' ', "."));
        let member = services::add_member(&self.admin, name, &email, can_edit).unwrap();
        PocketBaseClient::new(self.server.url())
            .authenticate("users", &member.email, &member.password)
            .unwrap()
    }

    fn read(&self) -> CatalogReadback {
        read_back(&self.admin).unwrap()
    }

    /// Imports `csv_file` from the copy as an importer does (reading the
    /// team catalog first) and returns the outcome; the zero-loss report
    /// must hold.
    fn import(&self, csv_file: &str) -> &'static str {
        let import = CsvCatalog::new(self.copy.path())
            .import(csv_file, None)
            .unwrap();
        self.import_planned(&import)
    }

    fn import_planned(&self, import: &catalog::CsvImport) -> &'static str {
        let state = read_import_state(&self.admin).unwrap();
        let command = ImportCommand::new(import, "Import the CSV file").expecting(&state);
        let reply = submit_import(&self.admin, &command).unwrap();
        let outcome = match reply.outcome {
            ImportOutcome::Applied { .. } => "applied",
            ImportOutcome::NothingChanged { .. } => "nothing_changed",
            ImportOutcome::Noop { .. } => "noop",
            _ => panic!("the import did not go through: {}", reply.body),
        };
        let oracle = csv_catalog_fixture::build_manifest(self.copy.path()).unwrap();
        let report =
            catalog_import::report(&oracle, &import.csv_file, &self.read(), self.copy.path());
        assert!(
            report.is_lossless(),
            "{}",
            serde_json::to_string_pretty(&report.summary()).unwrap()
        );
        outcome
    }

    /// Rewrites `database/<csv_file>.csv` in the copy.
    fn change_csv(&self, csv_file: &str, change: impl FnOnce(String) -> String) {
        let path = self.copy.path().join(format!("database/{csv_file}.csv"));
        let text = fs::read_to_string(&path).unwrap();
        let changed = change(text.clone());
        assert_ne!(changed, text, "the CSV file did not change");
        fs::write(path, changed).unwrap();
    }
}

/// A part's current details and their number.
fn details_of(readback: &CatalogReadback, ipn: &str) -> (u32, PartDetails) {
    let part = readback.part(ipn).unwrap();
    let details = readback.part_details(&part.current_details).unwrap();
    (
        details.number,
        serde_json::from_str(&details.snapshot).unwrap(),
    )
}

fn category<'a>(categories: &'a [CategoryDetails], code: &str) -> &'a CategoryDetails {
    categories
        .iter()
        .find(|category| category.code == code)
        .unwrap()
}

fn names(details: &PartDetails) -> Vec<&str> {
    details
        .fields
        .iter()
        .map(|field| field.name.as_str())
        .collect()
}

#[test]
fn each_csv_file_makes_a_category_that_later_imports_change_only_by_its_columns() {
    let team = Team::start();
    let stored = team.read();
    assert_eq!(stored.categories.len(), 2);
    assert_eq!(stored.category_revisions.len(), 2);
    let categories = current_categories(&team.admin).unwrap();
    let listed: Vec<(&str, &str, &str)> = categories
        .iter()
        .map(|category| {
            (
                category.code.as_str(),
                category.name.as_str(),
                category.ipn_prefix.as_str(),
            )
        })
        .collect();
    // Listed by name, as people see them.
    assert_eq!(
        listed,
        [("cap", "Capacitors", "CAP"), ("res", "Resistors", "RES")]
    );
    let resistors = category(&categories, "res");
    assert_eq!(
        resistors.fields,
        ["Description", "Value", "Tolerance", "Datasheet"]
    );
    assert_eq!(resistors.rules.reference_letters, ["R"]);
    // Every part is in the category of its CSV file, with its fields.
    let (_, details) = details_of(&stored, IPN);
    assert_eq!(details.category, "res");
    assert_eq!(names(&details), resistors.fields);
    let (_, capacitor) = details_of(&stored, "CAP-0000-0104");
    assert_eq!(capacitor.category, "cap");
    // Each category revision is in a csv-import Change with a reason and
    // records the header line it came from.
    let revision = &stored.category_revisions[0];
    let change = stored
        .changes
        .iter()
        .find(|change| change.id == revision.change)
        .unwrap();
    assert_eq!(change.kind, "csv-import");
    assert!(
        revision.source.contains("header_raw"),
        "{}",
        revision.source
    );
    // Imports never make a remembered match.
    assert!(stored.category_matches.is_empty());

    // Importing again changes nothing.
    assert_eq!(team.import("g-res"), "noop");
    // An import takes only the fields of a category the Catalog has: a
    // different name or rules in the request change nothing.
    let mut renamed = CsvCatalog::new(team.copy.path())
        .import("g-res", None)
        .unwrap();
    renamed.category.details.name = "Chip resistors".into();
    renamed.category.details.rules.keywords.push("chip".into());
    renamed.category.details_json = renamed.category.details.canonical_json();
    renamed.category.details_sha256 = catalog::sha256_hex(&renamed.category.details_json);
    assert_eq!(team.import_planned(&renamed), "noop");
    assert_eq!(
        category(&current_categories(&team.admin).unwrap(), "res").name,
        "Resistors"
    );

    // A changed part-details column changes the category's fields in its
    // next revision, keeping its name and rules, and every part's details.
    team.change_csv("g-res", |text| {
        text.replacen("\"Tolerance\"", "\"Tolerance (%)\"", 1)
    });
    assert_eq!(team.import("g-res"), "applied");
    let stored = team.read();
    let revisions: Vec<_> = stored
        .category_revisions
        .iter()
        .filter(|revision| revision.category == catalog::category::category_id("res"))
        .collect();
    assert_eq!(revisions.len(), 2);
    assert_eq!(revisions[1].number, 2);
    assert_eq!(revisions[1].predecessor, revisions[0].id);
    let resistors = category(&current_categories(&team.admin).unwrap(), "res").clone();
    assert_eq!(
        resistors.fields,
        ["Description", "Value", "Tolerance (%)", "Datasheet"]
    );
    assert_eq!(resistors.name, "Resistors");
    assert_eq!(resistors.rules.reference_letters, ["R"]);
    let (number, details) = details_of(&stored, IPN);
    assert_eq!(number, 2);
    assert_eq!(names(&details), resistors.fields);
    // Supplier data is no field, so changing it leaves the category.
    team.change_csv("g-res", |text| text.replacen("\"C23182\"", "\"C23183\"", 1));
    let before = team.read().category_revisions.len();
    assert_eq!(team.import("g-res"), "nothing_changed");
    assert_eq!(team.read().category_revisions.len(), before);
    team.server.stop();
}

#[test]
fn changing_a_category_keeps_the_ipn_and_every_field_and_remembers_the_persons_choice() {
    let team = Team::start();
    let editor = team.member("Ada Editor", true);
    let categories = current_categories(&editor).unwrap();
    let capacitors = category(&categories, "cap");
    let stored = team.read();
    let (number, before) = details_of(&stored, IPN);
    let revisions = stored.part_revisions.len();

    let mut edit = EditCommand::new(IPN, "It is a capacitor").editing_details(&before, number);
    edit.choose_category(&before, capacitors);
    // A field the new category adds can be filled in the same edit.
    assert!(edit.set_field("Voltage", "50V"));
    let reply = submit_edit(&editor, &edit).unwrap();
    assert_eq!(reply.status, 200, "{}", reply.body);
    let PublishOutcome::Applied {
        remembered_matches: remembered,
        revision,
        ..
    } = reply.outcome
    else {
        panic!("{}", reply.body);
    };
    assert!(revision.is_none(), "a category change adds no revision");
    let clues: Vec<(&str, &str)> = remembered
        .iter()
        .map(|clue| (clue.clue.as_str(), clue.value.as_str()))
        .collect();
    // The server works out the clues exactly as crates/catalog does.
    let family = footprint_family("g-Resistor_SMD:R_0603_1608Metric");
    assert_eq!(family, "R");
    assert_eq!(reference_letter("R"), "R");
    assert_eq!(
        clues,
        [
            ("footprint-family", family.as_str()),
            ("reference-letter", "R"),
            ("manufacturer", "Yageo"),
        ]
    );

    let stored = team.read();
    let part = stored.part(IPN).unwrap();
    assert_eq!(part.ipn, IPN, "the IPN never changes");
    assert_eq!(part.ipn_prefix, "RES");
    assert_eq!(stored.part_revisions.len(), revisions);
    let (number, after) = details_of(&stored, IPN);
    assert_eq!(number, 2);
    assert_eq!(after.category, "cap");
    // Every field the part had, then the fields of the new category it
    // lacked.
    assert_eq!(
        names(&after),
        [
            "Description",
            "Value",
            "Tolerance",
            "Datasheet",
            "Voltage",
            "Material"
        ]
    );
    assert_eq!(after.field("Voltage"), Some("50V"));
    assert_eq!(after.field("Material"), Some(""));
    assert_eq!(after.field("Value"), before.field("Value"));
    let difference = &compare_details(&before, &after)[0];
    assert_eq!(difference.what, DifferenceKind::Category);
    assert_eq!(
        (difference.before.as_str(), difference.after.as_str()),
        ("res", "cap")
    );

    // Each remembered match says who confirmed it and when (its Change).
    let change = stored.changes.last().unwrap();
    assert_eq!(stored.category_matches.len(), 3);
    for stored_match in &stored.category_matches {
        assert_eq!(stored_match.category, catalog::category::category_id("cap"));
        assert_eq!(stored_match.change, change.id);
        assert_eq!(stored_match.seq, change.seq);
        assert_eq!(stored_match.actor_name, "Ada Editor");
        assert!(stored_match.actor.starts_with("users:"));
        assert_eq!(stored_match.part, part.id);
    }
    // The guess learns from it: another R_ footprint now also points to
    // capacitors, because a person said so once.
    let matches = remembered_matches(&editor).unwrap();
    assert_eq!(matches.len(), 3);
    let ranked = guess_category(
        &Clues {
            footprints: vec!["R_0805_2012Metric".into()],
            ..Clues::default()
        },
        &categories,
        &matches,
        &[],
    );
    let capacitor_guess = ranked.iter().find(|guess| guess.category == "cap").unwrap();
    assert!(
        capacitor_guess
            .reasons
            .contains(&"people chose this category once for footprint family R".to_owned()),
        "{capacitor_guess:?}"
    );
    assert!(
        matches
            .iter()
            .all(|remembered| remembered.clue != ClueKind::SupplierCategory)
    );

    // An edit that keeps the category remembers nothing.
    let mut value = EditCommand::new(IPN, "Fix the value").editing_details(&after, 2);
    assert!(value.set_field("Value", "47R"));
    assert_eq!(submit_edit(&editor, &value).unwrap().status, 200);
    assert_eq!(team.read().category_matches.len(), 3);

    // Refused, and nothing written: a category the Catalog does not have,
    // a new category without its fields, someone who cannot edit.
    let stored = team.read();
    let (number, current) = details_of(&stored, IPN);
    let mut body = EditCommand::new(IPN, "Move it")
        .editing_details(&current, number)
        .body();
    body["details"]["category"] = "nope".into();
    let refused = editor.post_json(EDIT_ROUTE, &body).unwrap();
    assert_eq!(refused.status, 400, "{}", refused.body);
    assert!(
        refused.body.contains("category nope is not in the Catalog"),
        "{}",
        refused.body
    );
    let mut body = EditCommand::new(OTHER, "Move it")
        .editing_details(&details_of(&stored, OTHER).1, 1)
        .body();
    body["details"]["category"] = "cap".into();
    let refused = editor.post_json(EDIT_ROUTE, &body).unwrap();
    assert_eq!(refused.status, 400, "{}", refused.body);
    assert!(
        refused.body.contains("must list the same 6 fields as now"),
        "{}",
        refused.body
    );
    let viewer = team.member("Vic Viewer", false);
    let mut move_other =
        EditCommand::new(OTHER, "Move it").editing_details(&details_of(&stored, OTHER).1, 1);
    move_other.choose_category(&details_of(&stored, OTHER).1, capacitors);
    let refused = viewer.post_json(EDIT_ROUTE, &move_other.body()).unwrap();
    assert_eq!(refused.status, 403, "{}", refused.body);
    assert_eq!(team.read(), stored);

    // Moving it back keeps every field; that choice is remembered too.
    let resistors = category(&categories, "res");
    let mut back =
        EditCommand::new(IPN, "It is a resistor after all").editing_details(&current, number);
    back.choose_category(&current, resistors);
    assert_eq!(submit_edit(&editor, &back).unwrap().status, 200);
    let stored = team.read();
    let (_, back) = details_of(&stored, IPN);
    assert_eq!(back.category, "res");
    assert_eq!(names(&back), names(&current));
    assert_eq!(stored.category_matches.len(), 6);

    // Nobody writes a remembered match through the record API, superusers
    // included, nor changes one.
    let direct = team
        .admin
        .post_json(
            "/api/collections/catalog_category_matches/records",
            &serde_json::json!({ "value": "R" }),
        )
        .unwrap();
    assert_eq!(direct.status, 403, "{}", direct.body);
    let path = format!(
        "/api/collections/catalog_category_matches/records/{}",
        stored.category_matches[0].id
    );
    let changed = team
        .admin
        .patch_json(&path, &serde_json::json!({ "value": "C" }))
        .unwrap();
    assert_eq!(changed.status, 403, "{}", changed.body);
    team.server.stop();
}

/// The row of `OTHER` and of `IPN` in g-res, rewritten in g-cap's columns.
fn as_capacitor_row(ipn: &str, mpn: &str, value: &str) -> String {
    format!(
        "\"{ipn}\",\"{mpn}\",\"Yageo\",\"RES {value} OHM 1% 1/10W 0603\",\"g-res:R_US\",\"g-Resistor_SMD:R_0603_1608Metric\",\"{value}\",\"25V\",\"\",\"1%\",\"https://www.yageogroup.com/content/datasheet/asset/file/PYU-RC_GROUP_51_ROHS_L\",\"\",\"\",\"\",\"\",\"\",\"\",\"\"\n"
    )
}

#[test]
fn a_csv_that_moves_a_row_to_another_file_changes_the_category_like_any_detail() {
    let team = Team::start();
    let editor = team.member("Ada Editor", true);
    // IPN is edited in the app first; OTHER is not.
    let (number, details) = details_of(&team.read(), IPN);
    let mut edit = EditCommand::new(IPN, "Note the value").editing_details(&details, number);
    assert!(edit.set_field("Value", "47 ohm"));
    assert_eq!(submit_edit(&editor, &edit).unwrap().status, 200);

    // Both rows move from g-res to g-cap.
    team.change_csv("g-res", |text| {
        text.split_inclusive('\n')
            .filter(|line| !line.contains(IPN) && !line.contains(OTHER))
            .collect()
    });
    team.change_csv("g-cap", |text| {
        text + &as_capacitor_row(OTHER, "RC0603FR-07100RL", "100")
            + &as_capacitor_row(IPN, "RC0603FR-0747RL", "47")
    });
    assert_eq!(team.import("g-cap"), "applied");
    let stored = team.read();
    // Not edited in the app: the CSV's new category and fields apply.
    let (number, other) = details_of(&stored, OTHER);
    assert_eq!(number, 2);
    assert_eq!(other.category, "cap");
    assert_eq!(other.field("Voltage"), Some("25V"));
    assert_eq!(stored.part(OTHER).unwrap().ipn, OTHER);
    // Edited in the app: the part keeps its details and a CSV review notice
    // keeps the row.
    let (number, kept) = details_of(&stored, IPN);
    assert_eq!(kept.category, "res");
    let problem = stored
        .problems
        .iter()
        .find(|problem| problem.code == CSV_DETAILS_DIFFER && problem.subject == IPN)
        .unwrap();
    // The old file no longer has them: kept as import problems, nothing
    // deleted.
    assert_eq!(team.import("g-res"), "nothing_changed");
    assert!(
        team.read()
            .problems
            .iter()
            .any(|problem| problem.code == "part-not-in-csv-file" && problem.subject == OTHER)
    );

    // Taking the CSV's category in the review moves the part, keeping its
    // fields and adding the CSV's other fields with their values. A review
    // takes what the CSV says, so it remembers no match.
    let review = CsvReviewCommand::new(
        IPN,
        &problem.id,
        vec!["Category".into()],
        number,
        "Take the CSV's category",
    );
    let reply = submit_csv_review(&editor, &review).unwrap();
    assert_eq!(reply.status, 200, "{}", reply.body);
    let stored = team.read();
    let (_, moved) = details_of(&stored, IPN);
    assert_eq!(moved.category, "cap");
    assert_eq!(moved.field("Value"), Some("47 ohm"));
    assert_eq!(moved.field("Voltage"), Some("25V"));
    assert_eq!(
        names(&moved),
        [
            "Description",
            "Value",
            "Tolerance",
            "Datasheet",
            "Voltage",
            "Material"
        ]
    );
    assert!(stored.category_matches.is_empty());
    team.server.stop();
}

/// A route that saves a remembered match directly, as buggy server code
/// might, for the Change and part the body names.
const DIRECT_SAVE_HOOK: &str = r#"
routerAdd("POST", "/api/test/category-match", (e) => {
  const lib = require(`${__hooks}/catalog_lib.js`);
  const body = e.requestInfo().body;
  try {
    $app.runInTransaction((txApp) => {
      const change = txApp.findRecordById("catalog_changes", body.change);
      const part = txApp.findFirstRecordByData("catalog_parts", "ipn", body.ipn);
      const match = new Record(txApp.findCollectionByNameOrId("catalog_category_matches"));
      match.set("id", body.id || lib.deriveId("category-match", change.id + "\n" + body.clue));
      match.set("category", lib.deriveId("category", body.category));
      match.set("clue", body.clue);
      match.set("value", "Chip Resistor - Surface Mount");
      match.set("part", part.id);
      match.set("change", change.id);
      match.set("seq", body.seq || change.getInt("seq"));
      match.set("actor", body.actor || change.getString("actor"));
      match.set("actor_name", change.getString("actor_name"));
      txApp.save(match);
    });
  } catch (error) {
    return e.json(409, { refused: String(error) });
  }
  return e.json(200, { saved: true });
}, $apis.requireSuperuserAuth());
"#;

#[test]
fn server_code_can_only_save_a_remembered_match_with_a_persons_category_change() {
    let team = Team::start_with(Some(DIRECT_SAVE_HOOK));
    let editor = team.member("Ada Editor", true);
    let categories = current_categories(&editor).unwrap();
    let stored = team.read();
    let import_change = stored.changes[0].id.clone();
    let (number, details) = details_of(&stored, IPN);
    // An edit that keeps the category.
    let mut value = EditCommand::new(IPN, "Fix the value").editing_details(&details, number);
    assert!(value.set_field("Value", "47R"));
    assert_eq!(submit_edit(&editor, &value).unwrap().status, 200);
    let value_change = team.read().changes.last().unwrap().id.clone();
    // An edit that moves the part to the capacitors.
    let (number, details) = details_of(&team.read(), IPN);
    let mut moved = EditCommand::new(IPN, "It is a capacitor").editing_details(&details, number);
    moved.choose_category(&details, category(&categories, "cap"));
    assert_eq!(submit_edit(&editor, &moved).unwrap().status, 200);
    let stored = team.read();
    let move_change = stored.changes.last().unwrap().id.clone();

    let save = |overrides: serde_json::Value| {
        let mut body = serde_json::json!({
            "change": move_change,
            "ipn": IPN,
            "category": "cap",
            "clue": "supplier-category",
        });
        for (key, value) in overrides.as_object().unwrap() {
            body[key] = value.clone();
        }
        team.admin
            .post_json("/api/test/category-match", &body)
            .unwrap()
    };
    for (overrides, says) in [
        // An import, or any Change that is not an edit.
        (
            serde_json::json!({ "change": import_change }),
            "belong to an edit",
        ),
        // An edit that did not move the part to that category.
        (
            serde_json::json!({ "change": value_change }),
            "moves its part to the match's category",
        ),
        (
            serde_json::json!({ "category": "res" }),
            "moves its part to the match's category",
        ),
        (
            serde_json::json!({ "ipn": OTHER }),
            "moves its part to the match's category",
        ),
        (serde_json::json!({ "seq": 999 }), "copy its seq"),
        (
            serde_json::json!({ "actor": "users:someone" }),
            "who made it",
        ),
        (
            serde_json::json!({ "id": "aaaaaaaaaaaaaaa" }),
            "ID derived from its Change",
        ),
    ] {
        let reply = save(overrides.clone());
        assert_eq!(reply.status, 409, "{overrides}: {}", reply.body);
        assert!(reply.body.contains(says), "{says}: {}", reply.body);
    }
    assert_eq!(team.read(), stored, "nothing was written");
    // The person's own confirmation in that edit is accepted (here a
    // supplier category, which only ever counts once a person confirms it).
    let valid = save(serde_json::json!({}));
    assert_eq!(valid.status, 200, "{}", valid.body);
    assert_eq!(team.read().category_matches.len(), 4);
    team.server.stop();
}

#[test]
fn team_server_data_from_before_categories_is_refused_not_changed() {
    let team = Team::start();
    let details = team.read().part_details.len();
    team.server.stop();
    let options = services::options(&services::repository_root(), team.state.path());
    // Make the data look like data saved before categories: the migration
    // not yet applied, with part details stored.
    let database = rusqlite::Connection::open(options.data_dir.join("data.db")).unwrap();
    assert_eq!(
        database
            .execute(
                "DELETE FROM _migrations WHERE file = '1792300000_categories.js'",
                [],
            )
            .unwrap(),
        1
    );
    drop(database);
    let refused = services::start(&services::repository_root(), team.state.path())
        .err()
        .unwrap();
    let message = refused.to_string();
    assert!(
        message.contains("saved before parts had a category")
            && message.contains("cannot be upgraded")
            && message.contains("Start again with an empty data folder"),
        "{message}"
    );
    let database = rusqlite::Connection::open_with_flags(
        options.data_dir.join("data.db"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    let applied: i64 = database
        .query_row(
            "SELECT COUNT(*) FROM _migrations WHERE file = '1792300000_categories.js'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(applied, 0);
    let kept: i64 = database
        .query_row("SELECT COUNT(*) FROM catalog_part_details", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(kept, details as i64);
}
