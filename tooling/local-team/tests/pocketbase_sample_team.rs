//! A new team server starts with the starting categories (kicad-parts-e257),
//! a superuser may set up more, and the sample parts are added as Add part
//! adds a part (kicad-parts-fbc2). Every test team server starts so.

use catalog::category::starting_categories;
use catalog::pocketbase::{
    CategoryCommand, CategoryOutcome, changes_since, current_categories, read_back, submit_category,
};
use catalog::{CadReferenceTarget, CategoryDetails, ChangeKind, PartDetails, PartRevision};
use collaboration::PocketBaseClient;
use local_team::services;
use sample_library::Sample;

#[test]
fn a_new_team_server_starts_with_the_starting_categories() {
    let root = services::repository_root();
    let state = tempfile::tempdir().unwrap();
    let (server, admin, superuser) = services::start(&root, state.path()).unwrap();
    let mut categories = current_categories(&admin).unwrap();
    categories.sort_by(|a, b| a.code.cmp(&b.code));
    let mut starting = starting_categories().to_vec();
    starting.sort_by(|a, b| a.code.cmp(&b.code));
    assert_eq!(categories, starting);
    assert_eq!(categories.len(), 21);
    // All in one Change, the first.
    let changes = changes_since(&admin, 0).unwrap();
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].kind, ChangeKind::Category.as_str());
    assert_eq!(changes[0].seq, 1);
    // Started again, it sets up nothing more.
    let server = server.stop_for_a_while().start_again().unwrap();
    let admin = PocketBaseClient::new(server.url())
        .authenticate("_superusers", &superuser.email, &superuser.password)
        .unwrap();
    assert_eq!(changes_since(&admin, 0).unwrap().len(), 1);
    server.stop();
}

/// A category no team server starts with.
fn fuses() -> CategoryDetails {
    CategoryDetails {
        fields: vec!["Description".into(), "Current".into()],
        ..CategoryDetails::starting("fus")
    }
}

#[test]
fn a_superuser_sets_up_a_category_once_and_only_a_superuser_may() {
    let root = services::repository_root();
    let state = tempfile::tempdir().unwrap();
    let (server, admin, _superuser) = services::start(&root, state.path()).unwrap();
    let starting = current_categories(&admin).unwrap().len();
    let fuses = fuses();

    let command = CategoryCommand::new(fuses.clone(), "Set up the fuses");
    let reply = submit_category(&admin, &command).unwrap();
    let CategoryOutcome::Applied { seq, .. } = &reply.outcome else {
        panic!("{}", reply.body);
    };
    assert_eq!(*seq, 2);
    let categories = current_categories(&admin).unwrap();
    assert_eq!(categories.len(), starting + 1);
    let stored = categories.iter().find(|c| c.code == "fus").unwrap();
    assert_eq!(*stored, fuses);
    assert_eq!(stored.name, "fus");
    assert_eq!(stored.ipn_prefix, "FUS");
    let changes = changes_since(&admin, 1).unwrap();
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].kind, ChangeKind::Category.as_str());

    // The same command again is replayed; another command with its
    // correlation ID is refused.
    let again = submit_category(&admin, &command).unwrap();
    assert!(again.replayed);
    assert_eq!(again.body, reply.body);
    let mut other = CategoryCommand::new(CategoryDetails::starting("xyz"), "Another category");
    other.correlation_id = command.correlation_id.clone();
    assert!(matches!(
        submit_category(&admin, &other).unwrap().outcome,
        CategoryOutcome::CorrelationReused { .. }
    ));

    // A category the Catalog has is never changed this way, whether a
    // superuser or the setup made it.
    for code in ["fus", "res"] {
        let again = CategoryDetails {
            fields: vec!["Description".into()],
            ..CategoryDetails::starting(code)
        };
        let twice = submit_category(&admin, &CategoryCommand::new(again, "Again"))
            .unwrap_err()
            .to_string();
        assert!(
            twice.contains(&format!("already has the category {code}")),
            "{twice}"
        );
    }
    // A category with a CAD field is refused.
    let mut wrong = CategoryDetails::starting("abc");
    wrong.fields = vec!["Description".into(), "Symbol".into()];
    let refused = submit_category(&admin, &CategoryCommand::new(wrong, "Wrong"))
        .unwrap_err()
        .to_string();
    assert!(refused.contains("Symbol or Footprint"), "{refused}");
    // So is one with a field named Category, in any case: that is the
    // part's category in this app.
    let mut named = CategoryDetails::starting("abc");
    named.fields = vec!["Description".into(), " category".into()];
    let refused = submit_category(&admin, &CategoryCommand::new(named, "Wrong"))
        .unwrap_err()
        .to_string();
    assert!(
        refused.contains("Category, which is the part's category in this app"),
        "{refused}"
    );

    // A member who can edit the Catalog still cannot set up categories.
    let editor = services::add_member(&admin, "Ada Editor", "ada@example.test", true).unwrap();
    let client = PocketBaseClient::new(server.url())
        .authenticate("users", &editor.email, &editor.password)
        .unwrap();
    let other = CategoryCommand::new(CategoryDetails::starting("abc"), "Another");
    assert!(submit_category(&client, &other).is_err());
    assert_eq!(current_categories(&admin).unwrap().len(), starting + 1);
    server.stop();
}

#[test]
fn the_sample_parts_are_added_with_their_files_and_kicads_own_items_stay_external() {
    let root = services::repository_root();
    let state = tempfile::tempdir().unwrap();
    let (server, admin, _superuser) = services::start(&root, state.path()).unwrap();
    let sample = Sample::load();

    let seeded = sample_library::seed(&admin, &[]).unwrap();
    assert_eq!(seeded.parts.len(), sample.parts.len());
    // The starting categories' Change, then one per part.
    assert_eq!(seeded.last_seq, 1 + sample.parts.len() as u64);
    let changes = changes_since(&admin, 0).unwrap();
    assert!(
        changes
            .iter()
            .all(|change| change.kind == ChangeKind::Category.as_str()
                || change.kind == ChangeKind::Create.as_str()),
        "only the starting categories and new parts"
    );

    let readback = read_back(&admin).unwrap();
    for part in &sample.parts {
        let stored = readback.part(&part.ipn).unwrap();
        let details: PartDetails = serde_json::from_str(
            &readback
                .part_details(&stored.current_details)
                .unwrap()
                .snapshot,
        )
        .unwrap();
        assert_eq!(details.category, part.category, "{}", part.ipn);
        for (name, value) in &part.fields {
            assert_eq!(details.field(name), Some(value.as_str()), "{}", part.ipn);
        }
        let revision: PartRevision = serde_json::from_str(
            &readback
                .part_revision(&stored.current_revision)
                .unwrap()
                .snapshot,
        )
        .unwrap();
        // References to the sample library's own g- libraries are bound to
        // revision 1 of a CAD item; KiCad's own stay external.
        for reference in &revision.cad_references {
            let own = reference.reference.starts_with("g-");
            match &reference.target {
                CadReferenceTarget::CadItemRevision { number, .. } => {
                    assert!(own, "{}", reference.reference);
                    assert_eq!(*number, 1);
                }
                CadReferenceTarget::External => assert!(!own, "{}", reference.reference),
                other => panic!("{}: {other:?}", reference.reference),
            }
        }
    }
    // The capacitor footprint's own 3D model went with it.
    assert!(
        readback
            .cad_items
            .iter()
            .any(|item| item.key == "3d-models/g-Capacitor_SMD.3dshapes/C_0603_1608Metric.step")
    );
    server.stop();
}
