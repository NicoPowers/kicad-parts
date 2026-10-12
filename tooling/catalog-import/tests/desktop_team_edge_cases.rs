//! Edge cases of the desktop's team catalog use cases (catalog-browser's
//! TeamCatalog) against a disposable PocketBase with the resistors
//! imported: spaces around typed values, blank manufacturer rows, and a team
//! server that stops while the app is open.

use catalog::PartDetails;
use catalog::pocketbase::{EditCommand, ImportCommand, read_back, submit_edit, submit_import};
use catalog_browser::team::TeamCatalog;
use catalog_import::services;
use desktop_contract::{
    ManufacturerPart, PublishResult, SignInRequest, TeamArchiveRequest, TeamEditRequest, TeamPart,
    TeamSession,
};
use native_services::PocketBaseServer;

const IPN: &str = "RES-0000-0470";

/// A started team server with the resistors imported, the desktop's team
/// catalog signed in as an editor, and the state folder that must outlive
/// them.
struct Desktop {
    server: PocketBaseServer,
    admin: collaboration::PocketBaseClient,
    team: TeamCatalog,
    _state: tempfile::TempDir,
}

fn start() -> Desktop {
    let root = services::repository_root();
    let state = tempfile::tempdir().unwrap();
    let (server, admin, _superuser) = services::start(&root, state.path()).unwrap();
    let import = services::plan(&root, "g-res").unwrap();
    submit_import(
        &admin,
        &ImportCommand::new(&import, "Import the resistors")
            .allowing_uncommitted_changes(services::has_uncommitted_changes(&import)),
    )
    .unwrap();
    let editor = services::add_member(&admin, "Ada Editor", "ada@example.test", true).unwrap();
    let url = server.url().to_owned();
    let team = TeamCatalog::open(
        state.path().join("desktop").join("team.json"),
        Some(url.clone()),
        None,
    );
    let status = team.sign_in(SignInRequest {
        server_url: url,
        email: editor.email,
        password: editor.password,
    });
    assert!(
        matches!(status.session, TeamSession::SignedIn { .. }),
        "{status:?}"
    );
    Desktop {
        server,
        admin,
        team,
        _state: state,
    }
}

/// An edit of `part` as the edit form opens it: every value as it is now.
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
    }
}

fn set(request: &mut TeamEditRequest, name: &str, value: &str) {
    request
        .fields
        .iter_mut()
        .chain(request.cad_fields.iter_mut())
        .find(|field| field.name == name)
        .unwrap_or_else(|| panic!("no field {name}"))
        .value = value.into();
}

#[test]
fn spaces_around_typed_values_are_not_saved() {
    let desktop = start();
    let team = &desktop.team;
    let part = team.part(IPN).unwrap().unwrap();
    assert_eq!(part.details.field("Value"), Some("47"));

    // Only spaces added: nothing to save, and no new details number that
    // would show "47" changed to "47" in the history.
    let mut request = opened(&part, "Tidy");
    set(&mut request, "Value", " 47  ");
    set(&mut request, "Tolerance", "1%\t");
    assert_eq!(
        team.edit(request).unwrap(),
        PublishResult::NothingToSave {
            message: "Nothing changed, so nothing was saved.".into()
        }
    );
    // A value pasted with spaces around it is saved without them, and so
    // is a pasted MPN, so it cannot sit next to the same MPN as a second
    // manufacturer part.
    let mut request = opened(&part, "Exact value");
    set(&mut request, "Value", "  47R\u{a0}");
    request.manufacturer_parts.push(ManufacturerPart {
        manufacturer: " Vishay".into(),
        mpn: "CRCW060347R0FKEA \t".into(),
    });
    let PublishResult::Saved { part, .. } = team.edit(request).unwrap() else {
        panic!("not saved");
    };
    assert_eq!(part.details.field("Value"), Some("47R"));
    assert_eq!(
        part.details.manufacturer_parts.last().unwrap(),
        &ManufacturerPart {
            manufacturer: "Vishay".into(),
            mpn: "CRCW060347R0FKEA".into(),
        }
    );
    let mut request = opened(&part, "Second source");
    request.manufacturer_parts.push(ManufacturerPart {
        manufacturer: "Vishay".into(),
        mpn: "CRCW060347R0FKEA ".into(),
    });
    let PublishResult::Refused { message } = team.edit(request).unwrap() else {
        panic!("a repeated MPN was not refused");
    };
    assert!(message.contains("listed twice"), "{message}");

    // A Symbol pasted with a space after it adds no revision either.
    let mut request = opened(&part, "Same symbol");
    let symbol = part.revision.field("Symbol").unwrap().to_owned();
    set(&mut request, "Symbol", &format!("{symbol} "));
    assert!(matches!(
        team.edit(request).unwrap(),
        PublishResult::NothingToSave { .. }
    ));

    // A value someone else saved with spaces (the server keeps values as
    // sent) stays exactly as it is when the app saves another field.
    let readback = read_back(&desktop.admin).unwrap();
    let stored = readback.part(IPN).unwrap();
    let details: PartDetails = serde_json::from_str(
        &readback
            .part_details(&stored.current_details)
            .unwrap()
            .snapshot,
    )
    .unwrap();
    let mut theirs = EditCommand::new(IPN, "Spaced description")
        .editing_details(&details, stored.current_details_number);
    theirs.set_field("Description", " RES 47 OHM ");
    assert_eq!(submit_edit(&desktop.admin, &theirs).unwrap().status, 200);
    let part = team.part(IPN).unwrap().unwrap();
    let mut request = opened(&part, "Tighter tolerance");
    set(&mut request, "Tolerance", "0.5%");
    let PublishResult::Saved { part, .. } = team.edit(request).unwrap() else {
        panic!("not saved");
    };
    assert_eq!(part.details.field("Description"), Some(" RES 47 OHM "));
    assert_eq!(part.details.field("Tolerance"), Some("0.5%"));
    desktop.server.stop();
}

#[test]
fn a_blank_manufacturer_row_is_left_out() {
    let desktop = start();
    let team = &desktop.team;
    let part = team.part(IPN).unwrap().unwrap();
    // "Add a manufacturer part" pressed and left empty: the row is left out
    // instead of refusing the whole save.
    let mut request = opened(&part, "Exact value");
    set(&mut request, "Value", "47R");
    request.manufacturer_parts.push(ManufacturerPart {
        manufacturer: String::new(),
        mpn: " ".into(),
    });
    let PublishResult::Saved { part, .. } = team.edit(request).unwrap() else {
        panic!("not saved");
    };
    assert_eq!(part.details.manufacturer_parts.len(), 1);
    // Only a blank row added: nothing to save.
    let mut request = opened(&part, "Nothing");
    request.manufacturer_parts.push(ManufacturerPart {
        manufacturer: String::new(),
        mpn: String::new(),
    });
    assert!(matches!(
        team.edit(request).unwrap(),
        PublishResult::NothingToSave { .. }
    ));
    // Half a manufacturer part is still refused, in plain words.
    let mut request = opened(&part, "Second source");
    request.manufacturer_parts.push(ManufacturerPart {
        manufacturer: "Vishay".into(),
        mpn: String::new(),
    });
    let PublishResult::Refused { message } = team.edit(request).unwrap() else {
        panic!("half a manufacturer part was not refused");
    };
    assert_eq!(
        message,
        "Every manufacturer part needs a manufacturer and an MPN."
    );
    desktop.server.stop();
}

#[test]
fn saving_while_the_team_server_is_stopped_says_so_plainly() {
    let desktop = start();
    let team = &desktop.team;
    let part = team.part(IPN).unwrap().unwrap();
    desktop.server.stop();
    let plain = PublishResult::Refused {
        message: "Could not reach the team server. Check that it is running, then save again."
            .into(),
    };
    let mut request = opened(&part, "Exact value");
    set(&mut request, "Value", "47R");
    assert_eq!(team.edit(request), Ok(plain.clone()));
    let archive = TeamArchiveRequest {
        correlation_id: None,
        ipn: IPN.into(),
        revision: 1,
        archived: true,
        reason: "Bad symbol".into(),
    };
    assert_eq!(team.archive(archive), Ok(plain));
}

#[test]
fn a_part_keeps_one_symbol_and_each_footprint_choice_once() {
    let desktop = start();
    let team = &desktop.team;
    let part = team.part(IPN).unwrap().unwrap();
    let refused = |field: &str, value: &str| {
        let mut request = opened(&part, "Change the CAD");
        set(&mut request, field, value);
        match team.edit(request).unwrap() {
            PublishResult::Refused { message } => message,
            other => panic!("{field} {value:?} was not refused: {other:?}"),
        }
    };
    // KiCad gives a part one symbol, so a second one is refused, not saved
    // as a revision KiCad cannot use.
    assert_eq!(
        refused("Symbol", "g-res:R_US;Device:R"),
        "A part has one symbol, so the Symbol field can name only one; it names 2."
    );
    // Several footprint choices are fine, but not the same one twice.
    assert_eq!(
        refused(
            "Footprint",
            "g-Resistor_SMD:R_0603_1608Metric; g-Resistor_SMD:R_0603_1608Metric"
        ),
        "The Footprint field lists g-Resistor_SMD:R_0603_1608Metric twice."
    );
    assert_eq!(team.part(IPN).unwrap().unwrap().revision.number, 1);
    let mut request = opened(&part, "Allow the 0805 footprint too");
    set(
        &mut request,
        "Footprint",
        "g-Resistor_SMD:R_0603_1608Metric;Resistor_SMD:R_0805_2012Metric",
    );
    assert!(matches!(
        team.edit(request).unwrap(),
        PublishResult::Saved {
            revision: Some(2),
            ..
        }
    ));
    desktop.server.stop();
}
