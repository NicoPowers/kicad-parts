//! Categories and remembered matches through the protected Catalog routes
//! (seed kicad-parts-ae44): a part's category is part of its details, so
//! changing it in an edit keeps the IPN and every field and adds the new
//! category's fields; the person's choice is remembered with the part's
//! clues, and nothing else ever makes a remembered match.
//!
//! Each test starts its own disposable PocketBase on 127.0.0.1 with the
//! sample library's resistors and capacitors; the repository is only read.

use catalog::category::{footprint_family, reference_letter};
use catalog::guess::{ClueKind, Clues, guess_category};
use catalog::pocketbase::{
    CatalogReadback, EDIT_ROUTE, EditCommand, PublishOutcome, current_categories, read_back,
    remembered_matches, submit_edit,
};
use catalog::{CategoryDetails, DifferenceKind, PartDetails, compare_details};
use collaboration::PocketBaseClient;
use local_team::services;
use native_services::PocketBaseServer;
use std::fs;

const IPN: &str = "RES-0000-0470";
const OTHER: &str = "RES-0000-1000";

struct Team {
    server: PocketBaseServer,
    admin: PocketBaseClient,
    state: tempfile::TempDir,
}

impl Team {
    /// A team server with the sample resistors and capacitors.
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
        sample_library::seed(&admin, &["res", "cap"]).unwrap();
        Self {
            server,
            admin,
            state,
        }
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
fn changing_a_category_keeps_the_ipn_and_every_field_and_remembers_the_persons_choice() {
    let team = Team::start();
    let editor = team.member("Ada Editor", true);
    let categories = current_categories(&editor).unwrap();
    let capacitors = category(&categories, "cap");
    let stored = team.read();
    let (number, before) = details_of(&stored, IPN);
    let revisions = stored.part_revisions.len();
    // Adding the sample parts remembered their clues already, as Add part
    // does.
    let earlier = stored.category_matches.len();

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
    assert_eq!(stored.category_matches.len(), earlier + 3);
    for stored_match in stored
        .category_matches
        .iter()
        .filter(|m| m.change == change.id)
    {
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
    assert_eq!(matches.len(), earlier + 3);
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
    assert_eq!(team.read().category_matches.len(), earlier + 3);

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
    assert_eq!(stored.category_matches.len(), earlier + 6);

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

/// A route that saves a remembered match directly, as buggy server code
/// might, for the Change and part the body names. With `newEdit`, it
/// first saves what an edit route would: a new edit Change and the part's
/// new details in the match's category. The match then names that Change.
const DIRECT_SAVE_HOOK: &str = r#"
routerAdd("POST", "/api/test/category-match", (e) => {
  const lib = require(`${__hooks}/catalog_lib.js`);
  const body = e.requestInfo().body;
  try {
    $app.runInTransaction((txApp) => {
      const part = txApp.findFirstRecordByData("catalog_parts", "ipn", body.ipn);
      let change = null;
      if (body.newEdit) {
        const last = txApp.findRecordsByFilter("catalog_changes", "seq > 0", "-seq", 1, 0)[0];
        const correlation = "test-" + (last.getInt("seq") + 1);
        change = new Record(txApp.findCollectionByNameOrId("catalog_changes"));
        change.set("id", lib.deriveId("change", correlation));
        change.set("seq", last.getInt("seq") + 1);
        change.set("kind", "edit");
        change.set("reason", "test");
        change.set("correlation_id", correlation);
        change.set("actor", "users:test");
        change.set("actor_name", "Test");
        change.set("sources", "{}");
        change.set("summary", "{}");
        txApp.save(change);
        const current = txApp.findRecordById("catalog_part_details", part.getString("current_details"));
        const snapshot = JSON.parse(current.getString("snapshot"));
        snapshot.category = body.category;
        const number = current.getInt("number") + 1;
        const details = new Record(txApp.findCollectionByNameOrId("catalog_part_details"));
        details.set("id", lib.deriveId("part-details", part.id + "\n" + number));
        details.set("part", part.id);
        details.set("number", number);
        details.set("predecessor", current.id);
        details.set("change", change.id);
        details.set("snapshot", JSON.stringify(snapshot));
        details.set("snapshot_sha256", $security.sha256(JSON.stringify(snapshot)));
        details.set("source", "{}");
        txApp.save(details);
      } else {
        change = txApp.findRecordById("catalog_changes", body.change);
      }
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
    let first_change = stored.changes[0].id.clone();
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
        // A Change that is not an edit, here the one that set up a
        // category.
        (
            serde_json::json!({ "change": first_change }),
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
    // Even a match that would pass every check above is refused: it names
    // the edit's Change, saved in an earlier transaction, so desktops that
    // applied that Change would never fetch it (kicad-parts-2707). Only the
    // edit route itself saves matches.
    let late = save(serde_json::json!({}));
    assert_eq!(late.status, 409, "{}", late.body);
    assert!(
        late.body.contains("this transaction saved no Change"),
        "{}",
        late.body
    );
    assert_eq!(team.read(), stored, "nothing was written");
    // A valid match is still accepted: the person's own confirmation, in
    // the edit Change saved in the same transaction (here a supplier
    // category, which only ever counts once a person confirms it).
    let valid = save(serde_json::json!({ "ipn": OTHER, "newEdit": true }));
    assert_eq!(valid.status, 200, "{}", valid.body);
    assert_eq!(
        team.read().category_matches.len(),
        stored.category_matches.len() + 1
    );
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
