//! Edge cases of the desktop's team catalog use cases (catalog-browser's
//! TeamCatalog) against a disposable PocketBase with the resistors
//! imported: spaces around typed values, blank manufacturer rows, and a team
//! server that stops while the app is open or loses the answer to a save
//! (through a small proxy that can drop connections).

use catalog::PartDetails;
use catalog::pocketbase::{EditCommand, ImportCommand, read_back, submit_edit, submit_import};
use catalog_browser::team::TeamCatalog;
use catalog_import::services;
use desktop_contract::{
    ManufacturerPart, PublishResult, SignInRequest, TeamArchiveRequest, TeamEditRequest, TeamPart,
    TeamSession,
};
use native_services::PocketBaseServer;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};

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
    start_through(None)
}

/// What the proxy in front of the team server does to each request.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Fault {
    /// Passes every request and answer on.
    None,
    /// Passes a save (POST /api/catalog/...) on but drops its answer, as a
    /// connection that breaks after the server saved would.
    LoseSaveAnswer,
    /// Passes the next save and its answer on, then acts as a stopped
    /// server.
    DownAfterSave,
    /// Acts as a stopped server: closes every connection at once.
    Down,
}

/// A proxy on 127.0.0.1 in front of `upstream` (http://127.0.0.1:port),
/// whose fault the test sets. Each request gets its own connection to the
/// team server, closed after the answer, so answers (including the live
/// update stream) are copied as they come.
fn proxy(upstream: &str) -> (String, Arc<Mutex<Fault>>) {
    let upstream = upstream.trim_start_matches("http://").to_owned();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let fault = Arc::new(Mutex::new(Fault::None));
    let shared = fault.clone();
    std::thread::spawn(move || {
        for client in listener.incoming().flatten() {
            let upstream = upstream.clone();
            let fault = shared.clone();
            std::thread::spawn(move || {
                let _ = pass_on(client, &upstream, &fault);
            });
        }
    });
    (url, fault)
}

fn pass_on(mut client: TcpStream, upstream: &str, fault: &Mutex<Fault>) -> std::io::Result<()> {
    // The request head, then as much body as it says.
    let mut request = Vec::new();
    let mut chunk = [0u8; 8192];
    let head_end = loop {
        let read = client.read(&mut chunk)?;
        if read == 0 {
            return Ok(());
        }
        request.extend_from_slice(&chunk[..read]);
        if let Some(at) = request.windows(4).position(|w| w == b"\r\n\r\n") {
            break at + 4;
        }
    };
    let head = String::from_utf8_lossy(&request[..head_end]).into_owned();
    let length = head
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            if name.eq_ignore_ascii_case("content-length") {
                value.trim().parse::<usize>().ok()
            } else {
                None
            }
        })
        .unwrap_or(0);
    while request.len() < head_end + length {
        let read = client.read(&mut chunk)?;
        if read == 0 {
            break;
        }
        request.extend_from_slice(&chunk[..read]);
    }
    let save = head.starts_with("POST /api/catalog/");
    let now = *fault.lock().unwrap();
    if now == Fault::Down {
        return Ok(());
    }
    if save && now == Fault::DownAfterSave {
        *fault.lock().unwrap() = Fault::Down;
    }
    // One request per connection to the team server.
    let mut lines: Vec<&str> = head
        .trim_end()
        .lines()
        .filter(|line| !line.to_ascii_lowercase().starts_with("connection:"))
        .collect();
    lines.push("Connection: close");
    let mut server = TcpStream::connect(upstream)?;
    server.write_all(format!("{}\r\n\r\n", lines.join("\r\n")).as_bytes())?;
    server.write_all(&request[head_end..])?;
    if save && now == Fault::LoseSaveAnswer {
        // The server saves and answers; the answer never arrives.
        std::io::copy(&mut server, &mut std::io::sink())?;
        return Ok(());
    }
    std::io::copy(&mut server, &mut client)?;
    Ok(())
}

/// Like [`start`], with the desktop talking to the team server through a
/// proxy when `through` is Some; the proxy's fault is put there.
fn start_through(through: Option<&mut Option<Arc<Mutex<Fault>>>>) -> Desktop {
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
    let url = match through {
        Some(slot) => {
            let (url, fault) = proxy(server.url());
            *slot = Some(fault);
            url
        }
        None => server.url().to_owned(),
    };
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
        category: String::new(),
        fields: part.details.fields.clone(),
        manufacturer_parts: part.details.manufacturer_parts.clone(),
        cad_fields: part.revision.cad_fields.clone(),
        files: Vec::new(),
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

#[test]
fn someone_else_saving_exactly_your_changes_is_not_a_conflict() {
    let desktop = start();
    let team = &desktop.team;
    let part = team.part(IPN).unwrap().unwrap();
    // Two people fix the same value the same way at the same time.
    let mut theirs = opened(&part, "Exact value");
    set(&mut theirs, "Value", "47R");
    let mut yours = theirs.clone();
    yours.reason = "Write the value as the BOM does".into();
    assert!(matches!(
        team.edit(theirs).unwrap(),
        PublishResult::Saved { .. }
    ));
    let PublishResult::AlreadySaved { message, part } = team.edit(yours).unwrap() else {
        panic!("not taken as already saved");
    };
    assert_eq!(
        message,
        "These changes are already saved, so nothing more was saved."
    );
    assert_eq!(part.details.number, 2);
    assert_eq!(part.details.field("Value"), Some("47R"));
    assert_eq!(part.history.len(), 2, "nothing more was saved");
    desktop.server.stop();
}

#[test]
fn a_conflict_lists_only_what_truly_differs() {
    let desktop = start();
    let team = &desktop.team;
    let part = team.part(IPN).unwrap().unwrap();
    // They change the value; you make the same change and also tighten the
    // tolerance.
    let mut theirs = opened(&part, "Exact value");
    set(&mut theirs, "Value", "47R");
    assert!(matches!(
        team.edit(theirs).unwrap(),
        PublishResult::Saved { .. }
    ));
    let mut yours = opened(&part, "Exact value and tolerance");
    set(&mut yours, "Value", "47R");
    set(&mut yours, "Tolerance", "0.5%");
    let PublishResult::Conflict(conflict) = team.edit(yours).unwrap() else {
        panic!("no conflict");
    };
    // The value you both set the same way is not a difference.
    let rows: Vec<(&str, &str, &str)> = conflict
        .rows
        .iter()
        .map(|row| (row.label.as_str(), row.theirs.as_str(), row.yours.as_str()))
        .collect();
    assert_eq!(rows, [("Tolerance", "1%", "0.5%")]);
    // Saving on top adds only the tolerance.
    let PublishResult::Saved { part, .. } = team.edit(conflict.merged.clone()).unwrap() else {
        panic!("not saved on top");
    };
    assert_eq!(
        (part.details.field("Value"), part.details.field("Tolerance")),
        (Some("47R"), Some("0.5%"))
    );
    // If, before you save on top, they also save your tolerance, saving on
    // top is not a second conflict either.
    let part = team.part(IPN).unwrap().unwrap();
    let mut theirs = opened(&part, "Thick film");
    set(&mut theirs, "Description", "Thick film 47R");
    assert!(matches!(
        team.edit(theirs).unwrap(),
        PublishResult::Saved { .. }
    ));
    let mut late = opened(&part, "Thick film too");
    set(&mut late, "Description", "Thick film 47R");
    set(&mut late, "Tolerance", "0.1%");
    let PublishResult::Conflict(conflict) = team.edit(late).unwrap() else {
        panic!("no conflict");
    };
    let mut same = opened(&team.part(IPN).unwrap().unwrap(), "Tighter");
    set(&mut same, "Tolerance", "0.1%");
    assert!(matches!(
        team.edit(same).unwrap(),
        PublishResult::Saved { .. }
    ));
    assert!(matches!(
        team.edit(conflict.merged.clone()).unwrap(),
        PublishResult::AlreadySaved { .. }
    ));
    desktop.server.stop();
}

#[test]
fn a_save_whose_answer_is_lost_is_retried_without_saving_twice() {
    let mut fault = None;
    let desktop = start_through(Some(&mut fault));
    let fault = fault.unwrap();
    let set_fault = |now: Fault| *fault.lock().unwrap() = now;
    let team = &desktop.team;
    let part = team.part(IPN).unwrap().unwrap();
    let saved_details = || {
        let readback = read_back(&desktop.admin).unwrap();
        readback.part(IPN).unwrap().current_details_number
    };
    let unreachable = PublishResult::Refused {
        message: "Could not reach the team server. Check that it is running, then save again."
            .into(),
    };

    // The save reaches the server, which saves it, but the answer is lost.
    set_fault(Fault::LoseSaveAnswer);
    let mut request = opened(&part, "Exact value");
    set(&mut request, "Value", "47R");
    request.correlation_id = Some("desktop-lost-answer-1".into());
    assert_eq!(team.edit(request.clone()), Ok(unreachable.clone()));
    assert_eq!(saved_details(), 2, "the server saved it");
    // Saving again, as the app does with the same correlation ID, replays
    // the answer: saved once, not twice and not a conflict with yourself.
    set_fault(Fault::None);
    let PublishResult::Saved { part, .. } = team.edit(request).unwrap() else {
        panic!("the retry was not taken as saved");
    };
    assert_eq!((part.details.number, part.history.len()), (2, 2));

    // The save works, but the server stops before the app can show it.
    set_fault(Fault::DownAfterSave);
    let mut request = opened(&part, "Tighter tolerance");
    set(&mut request, "Tolerance", "0.5%");
    assert_eq!(
        team.edit(request),
        Ok(PublishResult::SavedNotShown {
            message: "Saved. The app could not reach the team server to show the latest version; it will update when the server is back.".into()
        })
    );
    assert_eq!(saved_details(), 3);

    // Someone else saved first, and the server stops before the app can
    // show their change: nothing was saved, and the app says so plainly.
    set_fault(Fault::None);
    let readback = read_back(&desktop.admin).unwrap();
    let stored = readback.part(IPN).unwrap();
    let details: PartDetails = serde_json::from_str(
        &readback
            .part_details(&stored.current_details)
            .unwrap()
            .snapshot,
    )
    .unwrap();
    let mut theirs = EditCommand::new(IPN, "Better description")
        .editing_details(&details, stored.current_details_number);
    theirs.set_field("Description", "Thick film resistor 47R");
    assert_eq!(submit_edit(&desktop.admin, &theirs).unwrap().status, 200);
    set_fault(Fault::DownAfterSave);
    let mut stale = opened(&part, "Datasheet");
    stale.expected_details = 3;
    set(&mut stale, "Datasheet", "https://example.test/rc0603.pdf");
    assert_eq!(
        team.edit(stale),
        Ok(PublishResult::Refused {
            message: "Someone else changed this part first, so nothing was saved. The app could not reach the team server to show their change. Check that it is running, then save again.".into()
        })
    );
    // A stopped server while the app reads the part before saving: plain
    // too.
    set_fault(Fault::Down);
    let mut request = opened(&part, "Value");
    set(&mut request, "Value", "47 Ohm");
    assert_eq!(team.edit(request), Ok(unreachable));
    desktop.server.stop();
}
