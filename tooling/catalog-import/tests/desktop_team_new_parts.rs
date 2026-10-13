//! The desktop's Add part (seed kicad-parts-9693), through catalog-browser
//! against a disposable PocketBase with temp copies of the resistors and
//! capacitors imported. Downloads are small ZIP files built here, in the
//! layouts SamacSys, Ultra Librarian and SnapEDA use, from the
//! repository's own CAD files (only read); no vendor download is committed.
//!
//! Reading a download fills in what the symbol says, guesses the category
//! with its reason, offers the next free IPN, and checks the MPN, the
//! footprint and the pin and pad counts. Saving adds the part, which the
//! search finds and whose history says where it came from.

use base64::Engine;
use catalog::CsvCatalog;
use catalog::pocketbase::{
    ImportCommand, ImportOutcome, read_back, read_import_state, submit_import,
};
use catalog_browser::team::TeamCatalog;
use catalog_browser::{BrowserConfig, CatalogBrowser};
use catalog_import::services;
use collaboration::PocketBaseClient;
use desktop_contract::{
    DownloadedFile, Field, LiveUpdates, ManufacturerPart, PublishResult, SignInRequest,
    TeamDownload, TeamDownloadRequest, TeamNewPartRequest, TeamSession,
};
use kicad_integration::SearchLocations;
use std::fs;
use std::io::Write;
use std::path::Path;
use std::time::{Duration, Instant};

const MPN: &str = "CL10B104KB8NNNC";

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

fn import(admin: &PocketBaseClient, copy: &Path, csv_file: &str) {
    let import = CsvCatalog::new(copy).import(csv_file, None).unwrap();
    let state = read_import_state(admin).unwrap();
    let command = ImportCommand::new(&import, "Import").expecting(&state);
    let outcome = submit_import(admin, &command).unwrap().outcome;
    assert!(
        matches!(outcome, ImportOutcome::Applied { .. }),
        "{outcome:?}"
    );
}

fn repository_text(path: &str) -> String {
    String::from_utf8(fs::read(services::repository_root().join(path)).unwrap()).unwrap()
}

/// The capacitor symbol renamed C_New, with the fields a SamacSys download
/// carries (its spellings), and `mpn`.
fn symbol_library(mpn: &str) -> String {
    repository_text("symbols/g-cap.kicad_sym")
        .replace("(symbol \"C_", "(symbol \"C_New_")
        .replace("(symbol \"C\"", "(symbol \"C_New\"")
        .replace("\"Value\" \".Value\"", "\"Value\" \"100n\"")
        .replace(
            "\"Datasheet\" \".Datasheet\"",
            "\"Datasheet\" \"https://example.test/cl10b104.pdf\"",
        )
        .replace(
            "\"Description\" \"Unpolarized capacitor\"",
            "\"Description\" \"CAP CER 0.1UF 50V X7R 0603\"",
        )
        .replacen(
            "\t\t(property \"Reference\"",
            &format!(
                "\t\t(property \"Manufacturer_Name\" \"Samsung Electro-Mechanics\" (at 0 0 0) (effects (hide yes)))\n\t\t(property \"Manufacturer_Part_Number\" \"{mpn}\" (at 0 0 0) (effects (hide yes)))\n\t\t(property \"Mouser Part Number\" \"187-{mpn}\" (at 0 0 0) (effects (hide yes)))\n\t\t(property \"Reference\""
            ),
            1,
        )
}

/// The 0603 capacitor footprint renamed C_0603_New: the same pads as the
/// Catalog's C_0603_1608Metric, naming its model where the vendor put it.
fn footprint_file() -> String {
    repository_text("footprints/g-Capacitor_SMD.pretty/C_0603_1608Metric.kicad_mod")
        .replace(
            "(footprint \"C_0603_1608Metric\"",
            "(footprint \"C_0603_New\"",
        )
        .replace(
            "${KICAD9_3DMODEL_DIR}/Capacitor_SMD.3dshapes/C_0603_1608Metric.step",
            "${KIPRJMOD}/3D/C_0603_New.stp",
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

/// A download of the new capacitor in one vendor's layout: `folder` is
/// where the KiCad files go, `extra` any other entries.
fn download(mpn: &str, folder: &str, extra: &[(&str, &[u8])]) -> Vec<u8> {
    let symbol = symbol_library(mpn);
    let footprint = footprint_file();
    let symbol_entry = format!("{folder}C_New.kicad_sym");
    let footprint_entry = format!("{folder}C_0603_New.kicad_mod");
    let mut entries: Vec<(&str, &[u8])> = vec![
        (&symbol_entry, symbol.as_bytes()),
        (&footprint_entry, footprint.as_bytes()),
        (
            "CL10B104/3D/C_0603_New.stp",
            b"ISO-10303-21;\nEND-ISO-10303-21;\n",
        ),
    ];
    entries.extend_from_slice(extra);
    zip_of(&entries)
}

fn request(zip: &[u8], name: &str) -> TeamDownloadRequest {
    TeamDownloadRequest {
        files: vec![DownloadedFile {
            file_name: name.into(),
            base64: base64::engine::general_purpose::STANDARD.encode(zip),
        }],
        symbol: None,
        footprint: None,
        without_models: false,
        library: None,
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
    files.push(reading.model_files[reading.model.unwrap()].clone());
    TeamNewPartRequest {
        correlation_id: None,
        ipn: ipn.into(),
        reason: "A new 100n capacitor".into(),
        category: category.into(),
        fields: chosen.fields.clone(),
        manufacturer_parts: vec![ManufacturerPart {
            manufacturer: reading.manufacturer.clone(),
            mpn: reading.mpn.clone(),
        }],
        symbol: format!("g-{category}:{}", reading.symbol_name),
        footprint: format!("{}:{}", reading.footprint_library, reading.footprint_name),
        files,
        from: reading.from.clone(),
        downloads: vec!["CL10B104.zip".into()],
        library: None,
    }
}

#[test]
fn the_desktop_adds_a_part_from_a_download() {
    let root = services::repository_root();
    let state = tempfile::tempdir().unwrap();
    let (server, admin, _superuser) = services::start(&root, state.path()).unwrap();
    let copy = tempfile::tempdir().unwrap();
    for folder in ["database", "symbols", "footprints", "3d-models"] {
        copy_folder(&root.join(folder), &copy.path().join(folder));
    }
    import(&admin, copy.path(), "g-res");
    import(&admin, copy.path(), "g-cap");
    let editor = services::add_member(&admin, "Ada Editor", "ada@example.test", true).unwrap();
    let url = server.url().to_owned();
    let team = TeamCatalog::open(state.path().join("team.json"), Some(url.clone()), None);
    let status = team.sign_in(SignInRequest {
        server_url: url,
        email: editor.email.clone(),
        password: editor.password.clone(),
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
    let data = tempfile::tempdir().unwrap();
    let browser = CatalogBrowser::open(BrowserConfig {
        catalog_root: copy.path().to_path_buf(),
        cache_dir: data.path().join("cache"),
        settings_file: data.path().join("settings.json"),
        kicad_locations: SearchLocations::Standard,
    });

    // Each vendor's layout says where the files came from; files for other
    // CAD tools are left out.
    for (folder, extra, from) in [
        (
            "CL10B104/KiCad/",
            &[("CL10B104/Altium/C_New.SchLib", &b"x"[..])][..],
            "SamacSys (Component Search Engine)",
        ),
        ("KiCAD/CL10B104/", &[][..], "Ultra Librarian"),
        ("", &[("how-to-import.htm", &b"<html>"[..])][..], "SnapEDA"),
    ] {
        let reading = browser
            .read_download(
                &request(&download(MPN, folder, extra), "CL10B104.zip"),
                &team,
            )
            .unwrap();
        assert_eq!(reading.from, from, "{folder}");
        assert_eq!(reading.ignored, extra.len(), "{folder}");
        assert_eq!(
            reading.symbols,
            [format!("C_New in {folder}C_New.kicad_sym in CL10B104.zip")]
        );
    }

    let zip = download(MPN, "CL10B104/KiCad/", &[]);
    let reading = browser
        .read_download(&request(&zip, "CL10B104.zip"), &team)
        .unwrap();
    // What the symbol says, in the vendor's spellings.
    assert_eq!(reading.mpn, MPN);
    assert_eq!(reading.manufacturer, "Samsung Electro-Mechanics");
    let capacitors = reading.categories.iter().find(|c| c.code == "cap").unwrap();
    let value_of = |name: &str| {
        capacitors
            .fields
            .iter()
            .find(|field| field.name == name)
            .map(|field| field.value.clone())
            .unwrap()
    };
    assert_eq!(value_of("Description"), "CAP CER 0.1UF 50V X7R 0603");
    assert_eq!(value_of("Value"), "100n");
    assert_eq!(value_of("Datasheet"), "https://example.test/cl10b104.pdf");
    assert_eq!(value_of("Voltage"), "");
    // Nothing is silently lost.
    assert!(
        capacitors.unused.contains(&Field {
            name: "Mouser Part Number".into(),
            value: format!("187-{MPN}"),
        }),
        "{:?}",
        capacitors.unused
    );
    // The guess, with its reason, and the next free IPN.
    assert_eq!(reading.guesses[0].code, "cap", "{:?}", reading.guesses);
    assert!(
        reading.guesses[0]
            .reasons
            .iter()
            .any(|reason| reason.contains("C_0603_New")),
        "{:?}",
        reading.guesses[0].reasons
    );
    assert!(reading.guesses.len() <= 3);
    assert_eq!(capacitors.next_ipn, "CAP-0004-0001");
    // The checks: no part has the MPN; the Catalog has a footprint with the
    // same pads; two pins and two pads; the footprint is linked to the
    // download's 3D model.
    assert!(reading.same_mpn.is_empty());
    assert!(
        reading.known_footprints.iter().any(|known| known.reference
            == "g-Capacitor_SMD:C_0603_1608Metric"
            && known.why.contains("the same pads")),
        "{:?}",
        reading.known_footprints
    );
    assert_eq!((reading.pins, reading.pads), (Some(2), Some(2)));
    assert_eq!(reading.footprint_library, "g-Capacitor_SMD");
    assert_eq!(reading.model, Some(0));
    assert!(
        reading
            .notes
            .iter()
            .any(|note| note.contains("will use the 3D model C_0603_New.stp")),
        "{:?}",
        reading.notes
    );

    // An MPN the Catalog has: the part that has it is shown.
    let existing = browser
        .read_download(
            &request(&download("CL10C020CB8NNNC", "KiCad/", &[]), "known.zip"),
            &team,
        )
        .unwrap();
    assert_eq!(existing.same_mpn.len(), 1, "{:?}", existing.same_mpn);
    assert_eq!(existing.same_mpn[0].ipn, "CAP-0000-0020");
    assert!(existing.same_mpn[0].listed);

    // Saving adds the part; the search finds it.
    let ipn = capacitors.next_ipn.clone();
    let saved = browser
        .add_part(new_part(&reading, &ipn, "cap"), &team)
        .unwrap();
    let PublishResult::Added { part } = saved else {
        panic!("not added: {saved:?}");
    };
    assert_eq!(part.ipn, ipn);
    assert_eq!(part.details.category, "cap");
    assert_eq!(part.revision.number, 1);
    assert_eq!(part.history.len(), 1);
    let entry = &part.history[0];
    assert_eq!(entry.action, "Added");
    let rows: Vec<(String, String)> = entry
        .changes
        .iter()
        .map(|row| (row.label.clone(), row.after.clone()))
        .collect();
    assert_eq!(
        rows[0],
        (
            "Downloaded from".to_owned(),
            "SamacSys (Component Search Engine): CL10B104.zip".to_owned()
        )
    );
    assert!(
        rows.iter().any(|(label, after)| label == "Footprint file"
            && after.starts_with("CL10B104/KiCad/C_0603_New.kicad_mod in CL10B104.zip (3D model path set to the Catalog's)")),
        "{rows:?}"
    );
    let found = browser.search_with(MPN, 10, &team.current_values());
    assert_eq!(found.total, 1, "{found:?}");
    assert_eq!(found.hits[0].ipn, ipn);
    assert_eq!(found.hits[0].csv_file, "Team catalog");
    let detail = browser.part_with(&ipn, &team.current_values()).unwrap();
    // The MPN an engineer types is checked too, without regard to case.
    let typed = team.same_mpn("cl10c020cb8nnnc", "Someone Else");
    assert_eq!(typed.len(), 1);
    assert!(!typed[0].listed);
    assert_eq!(detail.summary.mpn, MPN);

    // The footprint names its 3D model in the Catalog's folder.
    let stored = read_back(&admin).unwrap();
    let footprint = stored
        .cad_items
        .iter()
        .find(|item| item.key == "g-Capacitor_SMD:C_0603_New")
        .unwrap();
    let revision = stored
        .cad_item_revisions
        .iter()
        .find(|revision| revision.cad_item == footprint.id)
        .unwrap();
    assert!(
        revision
            .snapshot
            .contains("${GITPLM_PARTS}/3d-models/g-Capacitor_SMD.3dshapes/C_0603_New.stp"),
        "{}",
        revision.snapshot
    );
    // The confirmed category's clues are remembered.
    assert_eq!(stored.category_matches.len(), 3);

    // An IPN someone else has (here an imported part's): nothing saved,
    // the next free one offered.
    let again = browser
        .add_part(new_part(&reading, "CAP-0000-0020", "cap"), &team)
        .unwrap();
    let PublishResult::IpnTaken {
        next_free, message, ..
    } = again
    else {
        panic!("not refused: {again:?}");
    };
    assert_eq!(next_free, "CAP-0005-0001");
    assert!(message.contains("first"), "{message}");
    // A malformed IPN is refused before sending.
    let check = browser.check_ipn("cap-1", "CAP", &team);
    assert!(check.problem.unwrap().contains("such as RES-0001-0001"));
    assert!(browser.check_ipn(&ipn, "CAP", &team).problem.is_some());
    assert!(
        browser
            .check_ipn("CAP-0005-0001", "CAP", &team)
            .problem
            .is_none()
    );
    // An IPN must start with the category's prefix.
    let check = browser.check_ipn("RES-0005-0001", "CAP", &team);
    assert!(check.problem.unwrap().contains("starting with CAP-"));

    // A save whose answer was lost, sent again with changes: you added it.
    let mut changed = new_part(&reading, &ipn, "cap");
    changed.reason = "A new 100n capacitor, X7R".into();
    let again = browser.add_part(changed, &team).unwrap();
    let PublishResult::IpnTaken {
        yours,
        ipn: taken,
        message,
        next_free,
    } = again
    else {
        panic!("not refused: {again:?}");
    };
    assert!(yours, "{message}");
    assert_eq!(taken, ipn);
    assert!(
        message.starts_with(&format!("You already added {ipn}")),
        "{message}"
    );
    // The next free IPN is offered too, for another part.
    assert_eq!(next_free, "CAP-0005-0001");
    assert!(message.contains("CAP-0005-0001"), "{message}");
    drop(server);
}
