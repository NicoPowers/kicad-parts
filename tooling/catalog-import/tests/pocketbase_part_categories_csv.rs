//! Categories made by CSV imports (seed kicad-parts-ae44): each CSV file
//! makes one category, which later imports change only by its columns, and
//! a CSV that moves a row to another file changes the part's category like
//! any detail. Split from `pocketbase_part_categories.rs`
//! (kicad-parts-fbc2): these test the CSV import, which kicad-parts-e257
//! removes with the CSV catalog.
//!
//! Each test starts its own disposable PocketBase on 127.0.0.1 and imports a
//! temp copy of the CSV catalog; the repository is only read.

use catalog::pocketbase::{
    CSV_DETAILS_DIFFER, CatalogReadback, CsvReviewCommand, EditCommand, ImportCommand,
    ImportOutcome, current_categories, read_back, read_import_state, submit_csv_review,
    submit_edit, submit_import,
};
use catalog::{CategoryDetails, CsvCatalog, PartDetails};
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
    _state: tempfile::TempDir,
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
            _state: state,
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

#[test]
fn the_server_refuses_a_field_named_category_too() {
    // The planner refuses such a CSV column; a request built some other
    // way is refused by the route as well, and nothing is written.
    let team = Team::start();
    let stored = team.read();
    let mut import = CsvCatalog::new(team.copy.path())
        .import("g-ind", None)
        .unwrap();
    import.category.details.fields.push("category".into());
    import.category.details_json = import.category.details.canonical_json();
    import.category.details_sha256 = catalog::sha256_hex(&import.category.details_json);
    for part in &mut import.parts {
        part.details.fields.push(catalog::DetailField {
            name: "category".into(),
            value: "x".into(),
        });
        part.details_json = part.details.canonical_json();
        part.details_sha256 = catalog::sha256_hex(&part.details_json);
    }
    let state = read_import_state(&team.admin).unwrap();
    let command = ImportCommand::new(&import, "Import").expecting(&state);
    let refused = team
        .admin
        .post_json(catalog::pocketbase::CSV_IMPORT_ROUTE, &command.body());
    let refused = refused.unwrap();
    assert_eq!(refused.status, 400, "{}", refused.body);
    assert!(
        refused
            .body
            .contains("Category, which is the part's category in this app"),
        "{}",
        refused.body
    );
    assert_eq!(team.read(), stored);
    team.server.stop();
}
