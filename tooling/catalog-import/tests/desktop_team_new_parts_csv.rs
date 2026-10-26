//! Add part while the CSV catalog gets new rows (kicad-parts-3b72): it
//! never gives out an IPN a CSV row took since the app opened. Split from
//! `desktop_team_new_parts.rs` (kicad-parts-fbc2): this tests the CSV
//! catalog's IPNs, which kicad-parts-e257 removes with the CSV catalog.

use base64::Engine;
use catalog::CsvCatalog;
use catalog::pocketbase::{ImportCommand, ImportOutcome, read_import_state, submit_import};
use catalog_browser::team::TeamCatalog;
use catalog_browser::{BrowserConfig, CatalogBrowser};
use catalog_import::services;
use collaboration::PocketBaseClient;
use desktop_contract::{
    DownloadedFile, ManufacturerPart, PublishResult, SignInRequest, TeamDownload,
    TeamDownloadRequest, TeamNewPartRequest,
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
        picks: Vec::new(),
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
    }
}

/// Adds a capacitor row with `ipn` to the CSV catalog under `root`, as
/// someone working in the CSV catalog would, or as `git pull` would.
fn add_capacitor_row(root: &Path, ipn: &str) {
    let file = root.join("database/g-cap.csv");
    let mut text = fs::read_to_string(&file).unwrap();
    assert!(text.ends_with('\n'));
    text.push_str(&format!(
        "\"{ipn}\",\"GRM188R71H104KA93D\",\"Murata\",\"CAP CER 0.1UF 50V X7R 0603\"\n"
    ));
    fs::write(&file, text).unwrap();
}

/// The CSV catalog may get new rows while Add part is open (kicad-parts-
/// 3b72). Add part reads the folder again when it offers an IPN, while
/// someone types one, and just before saving, so it never gives a CSV
/// row's IPN to another part.
#[test]
fn add_part_never_gives_out_an_ipn_a_csv_row_took_since_the_app_opened() {
    let root = services::repository_root();
    let state = tempfile::tempdir().unwrap();
    let (server, admin, _superuser) = services::start(&root, state.path()).unwrap();
    let copy = tempfile::Builder::new()
        .prefix("add-part-csv-ipns-")
        .tempdir_in(root.join("target"))
        .unwrap();
    for folder in ["database", "symbols", "footprints", "3d-models"] {
        copy_folder(&root.join(folder), &copy.path().join(folder));
    }
    import(&admin, copy.path(), "g-cap");
    let editor = services::add_member(&admin, "Ada Editor", "ada@example.test", true).unwrap();
    let url = server.url().to_owned();
    let team = TeamCatalog::new();
    let status = team.sign_in(SignInRequest {
        server_url: url,
        email: editor.email.clone(),
        password: editor.password.clone(),
    });
    assert!(status.session.is_signed_in(), "{status:?}");
    wait_until("every imported part", || team.current_values().len() >= 17);
    let data = tempfile::tempdir().unwrap();
    let browser = CatalogBrowser::open(BrowserConfig {
        catalog_root: copy.path().to_path_buf(),
        cache_dir: data.path().join("cache"),
        settings_file: data.path().join("settings.json"),
        kicad_locations: SearchLocations::Standard,
        kicad_config_dir: None,
        kicad_documents_dir: None,
    });
    let zip = download(MPN, "CL10B104/KiCad/", &[]);
    let read = || {
        browser
            .read_download(&request(&zip, "CL10B104.zip"), &team)
            .unwrap()
    };
    let next_ipn = |reading: &TeamDownload| {
        reading
            .categories
            .iter()
            .find(|category| category.code == "cap")
            .unwrap()
            .next_ipn
            .clone()
    };
    let reading = read();
    assert_eq!(next_ipn(&reading), "CAP-0004-0001");

    // A row takes the offered IPN while the form is open: typing it is
    // refused, and saving with it saves nothing and offers the next one.
    add_capacitor_row(copy.path(), "CAP-0004-0001");
    let check = browser.check_ipn("CAP-0004-0001", "CAP", &team);
    assert_eq!(
        check.problem.as_deref(),
        Some("CAP-0004-0001 is already in database/g-cap.csv. An IPN is never reused.")
    );
    let saved = browser
        .add_part(new_part(&reading, "CAP-0004-0001", "cap"), &team)
        .unwrap();
    let PublishResult::IpnTaken {
        message,
        next_free,
        yours,
        ..
    } = saved
    else {
        panic!("saved with a CSV row's IPN: {saved:?}");
    };
    assert_eq!(
        message,
        "Someone gave the IPN CAP-0004-0001 to a row of database/g-cap.csv, so nothing was saved. The next free IPN is CAP-0005-0001."
    );
    assert_eq!(next_free, "CAP-0005-0001");
    assert!(!yours);
    assert!(!team.current_values().contains_key("CAP-0004-0001"));

    // Add part opened again offers the IPN after the row's.
    assert_eq!(next_ipn(&read()), "CAP-0005-0001");

    // The next free IPN saves.
    let saved = browser
        .add_part(new_part(&reading, &next_free, "cap"), &team)
        .unwrap();
    assert!(
        matches!(&saved, PublishResult::Added { part } if part.ipn == "CAP-0005-0001"),
        "{saved:?}"
    );
    drop(server);
}
