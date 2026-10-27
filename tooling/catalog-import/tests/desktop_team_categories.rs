//! The desktop's team catalog use cases for categories (catalog-browser's
//! TeamCatalog) against a disposable PocketBase with the sample library's
//! resistors and capacitors: a part shows its category by name, Edit part offers
//! every category, choosing another keeps the IPN and every field and adds
//! the new category's fields, the history says so in plain names, and a
//! conflict over the category can be saved on top.

use catalog::PartDetails;
use catalog::pocketbase::{EditCommand, current_categories, submit_edit};
use catalog_browser::team::TeamCatalog;
use catalog_import::services;
use collaboration::PocketBaseClient;
use desktop_contract::{Field, PublishResult, SignInRequest, TeamEditRequest, TeamPart};

const IPN: &str = "RES-0000-0470";

fn opened(part: &TeamPart, reason: &str) -> TeamEditRequest {
    TeamEditRequest {
        correlation_id: None,
        ipn: part.ipn.clone(),
        expected_details: part.details.number,
        expected_revision: part.revision.number,
        reason: reason.into(),
        category: part.details.category.clone(),
        fields: part.details.fields.clone(),
        manufacturer_parts: part.details.manufacturer_parts.clone(),
        cad_fields: part.revision.cad_fields.clone(),
        files: Vec::new(),
    }
}

fn names(fields: &[Field]) -> Vec<&str> {
    fields.iter().map(|field| field.name.as_str()).collect()
}

#[test]
fn the_desktop_shows_and_changes_a_parts_category() {
    let root = services::repository_root();
    let state = tempfile::tempdir().unwrap();
    let (server, admin, _superuser) = services::start(&root, state.path()).unwrap();
    sample_library::seed(&admin, &["res", "cap"]).unwrap();
    let editor = services::add_member(&admin, "Ada Editor", "ada@example.test", true).unwrap();
    let other = services::add_member(&admin, "Grace Editor", "grace@example.test", true).unwrap();
    let url = server.url().to_owned();
    let team = TeamCatalog::new();
    team.sign_in(SignInRequest {
        server_url: url.clone(),
        email: editor.email.clone(),
        password: editor.password.clone(),
    });

    // The part shows its category by name, and Edit part can offer every
    // category, by name, with its fields: the starting categories every
    // new team server has, in name order.
    let part = team.part(IPN).unwrap().unwrap();
    assert_eq!(part.details.category, "res");
    assert_eq!(part.details.category_name, "Resistors");
    let offered: Vec<&str> = part
        .categories
        .iter()
        .map(|category| category.name.as_str())
        .collect();
    let mut by_name = offered.clone();
    by_name.sort_by_key(|name| name.to_lowercase());
    assert_eq!(offered, by_name);
    assert_eq!(offered.len(), 21);
    let capacitors = part
        .categories
        .iter()
        .find(|category| category.code == "cap")
        .unwrap();
    assert_eq!(capacitors.name, "Capacitors");
    assert_eq!(
        capacitors.fields,
        [
            "Description",
            "Value",
            "Voltage",
            "Material",
            "Tolerance",
            "Datasheet"
        ]
    );

    // Choosing another category keeps every field and the IPN, and adds the
    // new category's fields, here one typed in and one left blank.
    let mut request = opened(&part, "It is a capacitor");
    request.category = "cap".into();
    request.fields.push(Field {
        name: "Voltage".into(),
        value: " 50V ".into(),
    });
    let PublishResult::Saved { revision, part, .. } = team.edit(request).unwrap() else {
        panic!("not saved");
    };
    assert_eq!(revision, None, "a category change adds no revision");
    assert_eq!(part.ipn, IPN);
    assert_eq!(part.details.category_name, "Capacitors");
    assert_eq!(
        names(&part.details.fields),
        [
            "Description",
            "Value",
            "Tolerance",
            "Datasheet",
            "Voltage",
            "Material"
        ]
    );
    assert_eq!(part.details.field("Voltage"), Some("50V"));
    assert_eq!(part.details.field("Material"), Some(""));
    let latest = &part.history[0];
    assert_eq!(latest.action, "Edited");
    let rows: Vec<(&str, &str, &str)> = latest
        .changes
        .iter()
        .map(|row| (row.label.as_str(), row.before.as_str(), row.after.as_str()))
        .collect();
    // Material, added blank, needs no row of its own.
    assert_eq!(
        rows,
        [
            ("Category", "Resistors", "Capacitors"),
            ("Voltage", "", "50V"),
        ]
    );

    // Someone else moves it back while this person edits its value: the
    // conflict names the category by name, and saving on top keeps theirs.
    let mut stale = opened(&part, "Exact value");
    stale
        .fields
        .iter_mut()
        .find(|field| field.name == "Value")
        .unwrap()
        .value = "47R".into();
    let theirs_client = PocketBaseClient::new(&url)
        .authenticate("users", &other.email, &other.password)
        .unwrap();
    let categories = current_categories(&theirs_client).unwrap();
    let resistors = categories
        .iter()
        .find(|category| category.code == "res")
        .unwrap();
    let current: PartDetails = {
        let stored = catalog::pocketbase::read_back(&admin).unwrap();
        let details = stored
            .part_details(&stored.part(IPN).unwrap().current_details)
            .unwrap()
            .snapshot
            .clone();
        serde_json::from_str(&details).unwrap()
    };
    let mut back = EditCommand::new(IPN, "It is a resistor").editing_details(&current, 2);
    back.choose_category(&current, resistors);
    assert_eq!(submit_edit(&theirs_client, &back).unwrap().status, 200);
    let PublishResult::Conflict(conflict) = team.edit(stale).unwrap() else {
        panic!("no conflict");
    };
    let category = conflict
        .rows
        .iter()
        .find(|row| row.label == "Category")
        .unwrap();
    assert_eq!(
        (
            category.started_from.as_str(),
            category.theirs.as_str(),
            category.yours.as_str(),
            category.you_changed,
        ),
        ("Capacitors", "Resistors", "Capacitors", false)
    );
    assert_eq!(conflict.merged.category, "res");
    let PublishResult::Saved { part, .. } = team.edit(conflict.merged.clone()).unwrap() else {
        panic!("not saved on top");
    };
    assert_eq!(part.details.category_name, "Resistors");
    assert_eq!(part.details.field("Value"), Some("47R"));
    assert_eq!(
        part.details.field("Voltage"),
        Some("50V"),
        "nothing is dropped"
    );

    // A category the team catalog does not have is refused plainly.
    let mut unknown = opened(&part, "Move it");
    unknown.category = "nope".into();
    assert_eq!(
        team.edit(unknown).unwrap(),
        PublishResult::Refused {
            message: "The category nope is not in the team catalog.".into()
        }
    );
    server.stop();
}
