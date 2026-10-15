//! Add part from symbols and footprints already in our libraries
//! (kicad-parts-37a7), through catalog-browser against a disposable
//! PocketBase: a temp copy of the CSV catalog, committed in a git
//! repository of its own, in which an engineer drew a capacitor symbol and
//! a footprint with its 3D model by hand in KiCad. The resistors and
//! capacitors are imported first; the repository is only read.

use catalog::pocketbase::{ImportOutcome, read_back, submit_import};
use catalog_browser::team::TeamCatalog;
use catalog_browser::{BrowserConfig, CatalogBrowser};
use catalog_import::services::{self, Allow};
use collaboration::PocketBaseClient;
use desktop_contract::{
    LibraryCheckView, LiveUpdates, ManufacturerPart, PublishResult, SignInRequest,
    TeamCsvReviewRequest, TeamDownload, TeamDownloadRequest, TeamLibraryChoice, TeamNewPartRequest,
    TeamPart, TeamSession,
};
use kicad_integration::SearchLocations;
use kicad_integration::library_file::{SymbolLibrary, SymbolLookup};
use native_services::PocketBaseServer;
use std::fs;
use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

const MPN: &str = "CL10B104KB8NNNC";
const SYMBOL: &str = "g-cap:C_Hand";
const FOOTPRINT: &str = "g-Capacitor_SMD:C_0603_Hand";
const SYMBOL_FILE: &str = "symbols/g-cap.kicad_sym";
const FOOTPRINT_FILE: &str = "footprints/g-Capacitor_SMD.pretty/C_0603_Hand.kicad_mod";
const MODEL_FILE: &str = "3d-models/g-Capacitor_SMD.3dshapes/C_0603_Hand.step";
const MODEL: &[u8] = b"ISO-10303-21;\nEND-ISO-10303-21;\n";
const FOLDERS: [&str; 4] = ["database", "symbols", "footprints", "3d-models"];

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

/// Runs git in `root` with a fixture identity and none of this computer's
/// hooks, line-ending changes or signing.
fn git(root: &Path, args: &[&str]) -> String {
    let hooks = format!("core.hooksPath={}", root.join(".git/no-hooks").display());
    let mut command = Command::new("git");
    command.arg("-C").arg(root).args([
        "-c",
        "user.name=fixture",
        "-c",
        "user.email=fixture@example.test",
        "-c",
        "commit.gpgsign=false",
        "-c",
        "core.autocrlf=false",
        "-c",
        hooks.as_str(),
    ]);
    command.args(args);
    let output = native_services::run_hidden("git", &mut command).unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

fn commit_all(root: &Path, message: &str) -> String {
    let mut add = vec!["add", "--"];
    add.extend_from_slice(&FOLDERS);
    git(root, &add);
    git(root, &["commit", "-q", "-m", message]);
    git(root, &["rev-parse", "HEAD"])
}

fn text(root: &Path, path: &str) -> String {
    String::from_utf8(fs::read(root.join(path)).unwrap()).unwrap()
}

/// A footprint drawn by hand from the 0603 capacitor footprint, named
/// `name` and linking the 3D model `link`.
fn hand_footprint(root: &Path, name: &str, link: &str) -> String {
    text(
        root,
        "footprints/g-Capacitor_SMD.pretty/C_0603_1608Metric.kicad_mod",
    )
    .replace(
        "(footprint \"C_0603_1608Metric\"",
        &format!("(footprint \"{name}\""),
    )
    .replace(
        "${KICAD9_3DMODEL_DIR}/Capacitor_SMD.3dshapes/C_0603_1608Metric.step",
        link,
    )
}

/// Draws, in the copy under `root`, the capacitor symbol C_Hand (in
/// g-cap.kicad_sym, naming its footprint and MPN) and the footprint
/// C_0603_Hand linking its own 3D model, as an engineer would in KiCad.
fn draw_by_hand(root: &Path) {
    let library = fs::read(root.join(SYMBOL_FILE)).unwrap();
    let parsed = SymbolLibrary::parse(&library).unwrap();
    let SymbolLookup::Found(entry) = parsed.find("C") else {
        panic!("no C symbol");
    };
    let text = String::from_utf8(library.clone()).unwrap();
    let hand = text[entry.span.clone()]
        .replace("(symbol \"C_", "(symbol \"C_Hand_")
        .replacen("(symbol \"C\"", "(symbol \"C_Hand\"", 1)
        .replacen(
            "(property \"Footprint\" \"\"",
            &format!("(property \"Footprint\" \"{FOOTPRINT}\""),
            1,
        )
        .replacen(
            "\"Description\" \"Unpolarized capacitor\"",
            "\"Description\" \"CAP CER 0.1UF 50V X7R 0603\"",
            1,
        )
        .replacen(
            "\t\t(property \"Reference\"",
            &format!(
                "\t\t(property \"MPN\" \"{MPN}\" (at 0 0 0) (effects (hide yes)))\n\t\t(property \"Manufacturer\" \"Samsung Electro-Mechanics\" (at 0 0 0) (effects (hide yes)))\n\t\t(property \"Reference\""
            ),
            1,
        );
    assert!(hand.contains(FOOTPRINT), "{hand}");
    let close = text.rfind(')').unwrap();
    fs::write(
        root.join(SYMBOL_FILE),
        format!("{}\t{hand}\n{}", &text[..close], &text[close..]),
    )
    .unwrap();
    fs::write(
        root.join(FOOTPRINT_FILE),
        hand_footprint(
            root,
            "C_0603_Hand",
            &format!("${{GITPLM_PARTS}}/{MODEL_FILE}"),
        ),
    )
    .unwrap();
    fs::create_dir_all(root.join("3d-models/g-Capacitor_SMD.3dshapes")).unwrap();
    fs::write(root.join(MODEL_FILE), MODEL).unwrap();
}

/// Imports `csv_file` from the copy as the CSV import does it: from its
/// commit, refused for uncommitted changes and the library check's errors.
fn import(admin: &PocketBaseClient, root: &Path, csv_file: &str) -> ImportOutcome {
    let import = services::plan(root, csv_file).unwrap();
    let command =
        services::prepare_import(admin, root, &import, "Import", Allow::default()).unwrap();
    submit_import(admin, &command).unwrap().outcome
}

fn choice(symbol: Option<&str>, footprint: Option<&str>) -> TeamLibraryChoice {
    TeamLibraryChoice {
        symbol: symbol.map(str::to_owned),
        footprint: footprint.map(str::to_owned),
    }
}

/// A team server with the resistors and capacitors imported from the
/// committed copy, an editor signed in, and the desktop's browser over the
/// copy.
struct Team {
    _server: PocketBaseServer,
    admin: PocketBaseClient,
    copy: tempfile::TempDir,
    head: String,
    _state: tempfile::TempDir,
    _data: tempfile::TempDir,
    team: TeamCatalog,
    browser: CatalogBrowser,
}

impl Team {
    /// `draw` changes the copy before it is committed.
    fn start(draw: impl FnOnce(&Path)) -> Self {
        let root = services::repository_root();
        let state = tempfile::tempdir().unwrap();
        let (server, admin, _superuser) = services::start(&root, state.path()).unwrap();
        let copy = tempfile::tempdir().unwrap();
        for folder in FOLDERS {
            copy_folder(&root.join(folder), &copy.path().join(folder));
        }
        git(copy.path(), &["init", "-q"]);
        commit_all(copy.path(), "Today's CSV catalog");
        for csv in ["g-res", "g-cap"] {
            assert!(matches!(
                import(&admin, copy.path(), csv),
                ImportOutcome::Applied { .. }
            ));
        }
        draw(copy.path());
        let head = commit_all(copy.path(), "Drawn by hand in KiCad");
        let member = services::add_member(&admin, "Ada Editor", "ada@example.test", true).unwrap();
        let url = server.url().to_owned();
        let team = TeamCatalog::open(state.path().join("ada.json"), Some(url.clone()), None);
        let status = team.sign_in(SignInRequest {
            server_url: url,
            email: member.email.clone(),
            password: member.password.clone(),
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
            kicad_config_dir: None,
            kicad_documents_dir: None,
        });
        Self {
            _server: server,
            admin,
            copy,
            head,
            _state: state,
            _data: data,
            team,
            browser,
        }
    }

    fn root(&self) -> &Path {
        self.copy.path()
    }

    fn read(&self, choice: TeamLibraryChoice) -> TeamDownload {
        self.browser
            .read_download(
                &TeamDownloadRequest {
                    files: Vec::new(),
                    symbol: None,
                    footprint: None,
                    without_models: false,
                    library: Some(choice),
                    picks: Vec::new(),
                },
                &self.team,
            )
            .unwrap()
    }

    fn check(&self) -> LibraryCheckView {
        self.browser.library_check(&self.team).unwrap().unwrap()
    }

    fn add(&self, request: TeamNewPartRequest) -> PublishResult {
        self.browser.add_part(request, &self.team).unwrap()
    }

    fn stored_revisions(&self) -> usize {
        read_back(&self.admin).unwrap().cad_item_revisions.len()
    }
}

/// The items a group of the library check lists, as `LIBRARY:NAME` or a
/// path.
fn group(view: &LibraryCheckView, title: &str) -> Vec<String> {
    view.groups
        .iter()
        .find(|group| group.title == title)
        .map(|group| {
            group
                .items
                .iter()
                .map(|item| {
                    if item.library.is_empty() {
                        item.name.clone()
                    } else {
                        format!("{}:{}", item.library, item.name)
                    }
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The request Add part sends for `reading` from our libraries, accepting
/// what it offers, in `category`.
fn new_part(reading: &TeamDownload, ipn: &str, category: &str) -> TeamNewPartRequest {
    let library = reading.library.clone().unwrap();
    let chosen = reading
        .categories
        .iter()
        .find(|offered| offered.code == category)
        .unwrap();
    TeamNewPartRequest {
        correlation_id: None,
        ipn: ipn.into(),
        reason: "Drawn in KiCad for the new board".into(),
        category: category.into(),
        fields: chosen.fields.clone(),
        manufacturer_parts: vec![ManufacturerPart {
            manufacturer: reading.manufacturer.clone(),
            mpn: reading.mpn.clone(),
        }],
        symbol: library.symbol.clone(),
        footprint: library.footprint.clone(),
        files: Vec::new(),
        from: String::new(),
        downloads: Vec::new(),
        library: Some(choice(
            Some(&library.symbol)
                .filter(|symbol| !symbol.is_empty())
                .map(String::as_str),
            Some(&library.footprint)
                .filter(|footprint| !footprint.is_empty())
                .map(String::as_str),
        )),
    }
}

/// Appends `part`'s CSV row to the copy's g-cap.csv, with the part's own
/// values and its Symbol and Footprint, as someone would by hand.
fn add_csv_row(root: &Path, part: &TeamPart) {
    let field = |name: &str| {
        part.details
            .fields
            .iter()
            .find(|field| field.name == name)
            .map(|field| field.value.clone())
            .unwrap_or_default()
    };
    let mut csv = text(root, "database/g-cap.csv");
    csv.push_str(&format!(
        "\"{}\",\"{MPN}\",\"Samsung Electro-Mechanics\",\"{}\",\"{}\",\"{}\",\"{}\",\"{}\",\"{}\",\"{}\",\"{}\",\"\",\"\",\"\",\"\",\"\",\"\",\"\"
",
        part.ipn,
        field("Description"),
        part.revision.field("Symbol").unwrap_or_default(),
        part.revision.field("Footprint").unwrap_or_default(),
        field("Value"),
        field("Voltage"),
        field("Material"),
        field("Tolerance"),
        field("Datasheet"),
    ));
    fs::write(root.join("database/g-cap.csv"), csv).unwrap();
}

fn added(result: PublishResult) -> TeamPart {
    match result {
        PublishResult::Added { part } => part,
        other => panic!("not added: {other:?}"),
    }
}

/// An engineer adds a part from a symbol drawn by hand: its footprint and
/// 3D model come along, the checks run, the files are saved exactly as in
/// the library with the commit they came from, the library check no longer
/// lists them, and a later CSV import naming them finds nothing to change.
#[test]
fn a_part_from_our_libraries_end_to_end() {
    let team = Team::start(draw_by_hand);
    let root = team.root().to_path_buf();

    // Add part's search lists what the library check has not in the
    // Catalog yet: the hand-drawn items, not the Catalog's own.
    let before = team.check();
    let new_items = group(&before, "Not in the Catalog yet");
    assert!(new_items.contains(&SYMBOL.to_owned()), "{new_items:?}");
    assert!(new_items.contains(&FOOTPRINT.to_owned()), "{new_items:?}");
    assert!(!new_items.contains(&"g-cap:C".to_owned()));
    assert!(
        !new_items.contains(&MODEL_FILE.to_owned()),
        "a 3D model a library footprint links is never listed on its own"
    );

    // Choosing the symbol brings the footprint it names, and that
    // footprint's own 3D model.
    let reading = team.read(choice(Some(SYMBOL), None));
    let library = reading.library.clone().unwrap();
    assert_eq!(library.symbol, SYMBOL);
    assert_eq!(library.footprint, FOOTPRINT);
    assert_eq!(library.model, MODEL_FILE);
    assert_eq!(library.commit, team.head);
    assert_eq!(library.problems, Vec::<String>::new());
    assert!(library.in_catalog.is_empty(), "{library:?}");
    assert_eq!(reading.from, "Our libraries");
    assert_eq!(reading.mpn, MPN);
    assert_eq!(reading.manufacturer, "Samsung Electro-Mechanics");
    assert_eq!(
        reading.guesses.first().map(|guess| guess.code.as_str()),
        Some("cap")
    );
    assert_eq!((reading.pins, reading.pads), (Some(2), Some(2)));
    assert_eq!(reading.footprint_library, "g-Capacitor_SMD");
    assert!(
        reading.notes.iter().any(|note| note
            == "The footprint is the one the symbol names, g-Capacitor_SMD:C_0603_Hand."),
        "{:?}",
        reading.notes
    );
    // The footprint is not a near-copy of itself, but of the Catalog's.
    let known: Vec<&str> = reading
        .known_footprints
        .iter()
        .map(|known| known.reference.as_str())
        .collect();
    assert!(!known.contains(&FOOTPRINT), "{known:?}");
    assert!(
        known.contains(&"g-Capacitor_SMD:C_0603_1608Metric"),
        "{known:?}"
    );
    let cap = reading
        .categories
        .iter()
        .find(|category| category.code == "cap")
        .unwrap();
    assert_eq!(cap.footprint_library, "g-Capacitor_SMD");

    let part = added(team.add(new_part(&reading, &cap.next_ipn, "cap")));
    assert_eq!(part.ipn, cap.next_ipn);
    // References stay exactly as in the library.
    assert_eq!(part.revision.field("Symbol"), Some(SYMBOL));
    assert_eq!(part.revision.field("Footprint"), Some(FOOTPRINT));
    // The history says where it came from, with the commit.
    let first = part.history.first().unwrap();
    assert_eq!(first.action, "Added");
    let short = &team.head[..12];
    let from = first
        .changes
        .iter()
        .find(|row| row.label == "Added from")
        .unwrap_or_else(|| panic!("{:?}", first.changes));
    assert_eq!(
        from.after,
        format!(
            "Our libraries: {SYMBOL_FILE} at {short}, {FOOTPRINT_FILE} at {short}, {MODEL_FILE} at {short}"
        )
    );

    // Stored as CAD items, byte for byte, naming the file and the commit.
    let stored = read_back(&team.admin).unwrap();
    let library_bytes = fs::read(root.join(SYMBOL_FILE)).unwrap();
    let parsed = SymbolLibrary::parse(&library_bytes).unwrap();
    let SymbolLookup::Found(entry) = parsed.find("C_Hand") else {
        panic!("no C_Hand");
    };
    for (key, path, bytes) in [
        (
            SYMBOL,
            SYMBOL_FILE,
            library_bytes[entry.span.clone()].to_vec(),
        ),
        (
            FOOTPRINT,
            FOOTPRINT_FILE,
            fs::read(root.join(FOOTPRINT_FILE)).unwrap(),
        ),
        (MODEL_FILE, MODEL_FILE, MODEL.to_vec()),
    ] {
        let item = stored
            .cad_items
            .iter()
            .find(|item| item.key == key)
            .unwrap();
        let revision = stored
            .cad_item_revisions
            .iter()
            .find(|revision| revision.cad_item == item.id)
            .unwrap();
        assert_eq!(revision.number, 1);
        let source: serde_json::Value = serde_json::from_str(&revision.source).unwrap();
        assert_eq!(source["source"], "library", "{source}");
        assert_eq!(source["path"], path);
        assert_eq!(source["source_commit"], team.head.as_str());
        let blob = stored.blob(&revision.blob).unwrap();
        assert_eq!(blob.bytes, bytes, "{key}");
    }

    // The library check no longer lists them as not in the Catalog yet.
    let after = team.check();
    let still_new = group(&after, "Not in the Catalog yet");
    assert!(!still_new.contains(&SYMBOL.to_owned()), "{still_new:?}");
    assert!(!still_new.contains(&FOOTPRINT.to_owned()), "{still_new:?}");
    assert_eq!(after.not_in_catalog, before.not_in_catalog - 2);
    assert_eq!(after.to_fix, 0, "{after:#?}");

    // Someone adds the part's CSV row by hand, naming the same symbol and
    // footprint with the same bytes, and imports: nothing to change.
    let revisions = team.stored_revisions();
    add_csv_row(&root, &part);
    commit_all(&root, "The new capacitor's CSV row");
    let outcome = import(&team.admin, &root, "g-cap");
    assert!(
        matches!(
            outcome,
            ImportOutcome::NothingChanged { .. } | ImportOutcome::Noop { .. }
        ),
        "{outcome:?}"
    );
    assert_eq!(team.stored_revisions(), revisions);
    let now = team.team.part(&part.ipn).unwrap().unwrap();
    assert!(now.csv_changes.is_empty(), "{:?}", now.csv_changes);
    assert_eq!(now.revision.number, 1);
}

/// What stops a part from our libraries being saved, each said plainly
/// before saving and refused when saving: a 3D model link outside our
/// library, a missing 3D model file, and files not committed as they are.
/// A symbol the Catalog already has is used, not sent.
#[test]
fn problems_stop_the_save_and_the_catalogs_own_items_are_used() {
    let team = Team::start(|root| {
        draw_by_hand(root);
        fs::write(
            root.join("footprints/g-Capacitor_SMD.pretty/C_0603_Far.kicad_mod"),
            hand_footprint(root, "C_0603_Far", "/home/me/models/C_0603_Far.step"),
        )
        .unwrap();
        fs::write(
            root.join("footprints/g-Capacitor_SMD.pretty/C_0603_Gone.kicad_mod"),
            hand_footprint(
                root,
                "C_0603_Gone",
                "${GITPLM_PARTS}/3d-models/g-Capacitor_SMD.3dshapes/C_0603_Gone.step",
            ),
        )
        .unwrap();
    });
    let revisions = team.stored_revisions();
    let refused = |symbol: &str, footprint: &str| -> (TeamDownload, String) {
        let reading = team.read(choice(Some(symbol), Some(footprint)));
        let request = new_part(&reading, "CAP-0099-0001", "cap");
        match team.add(request) {
            PublishResult::Refused { message } => (reading, message),
            other => panic!("not refused: {other:?}"),
        }
    };

    // A 3D model link that only works on one computer.
    let (reading, message) = refused(SYMBOL, "g-Capacitor_SMD:C_0603_Far");
    let problems = reading.library.unwrap().problems;
    assert_eq!(problems.len(), 1, "{problems:?}");
    assert!(
        problems[0].starts_with("g-Capacitor_SMD:C_0603_Far: The footprint links the 3D model /home/me/models/C_0603_Far.step, which is outside our library"),
        "{problems:?}"
    );
    assert_eq!(message, format!("{} Nothing was saved.", problems[0]));

    // A 3D model file missing from our library.
    let (reading, message) = refused(SYMBOL, "g-Capacitor_SMD:C_0603_Gone");
    let problems = reading.library.unwrap().problems;
    assert!(
        problems.iter().any(|problem| problem.contains(
            "links the 3D model 3d-models/g-Capacitor_SMD.3dshapes/C_0603_Gone.step, which is not in our library"
        )),
        "{problems:?}"
    );
    assert!(message.ends_with("Nothing was saved."), "{message}");

    // A footprint changed in KiCad and not committed yet.
    let footprint = team.root().join(FOOTPRINT_FILE);
    let committed = fs::read_to_string(&footprint).unwrap();
    fs::write(
        &footprint,
        committed.replacen("(attr", "(descr \"checked\")\n\t(attr", 1),
    )
    .unwrap();
    let (reading, message) = refused(SYMBOL, FOOTPRINT);
    let library = reading.library.unwrap();
    assert_eq!(
        library.problems,
        [format!(
            "{FOOTPRINT_FILE} is not committed as it is. Commit it first, then choose it again: the Catalog records the commit each file came from."
        )]
    );
    assert!(library.commit.is_empty());
    assert!(message.contains("is not committed as it is"), "{message}");
    assert_eq!(team.stored_revisions(), revisions, "nothing was saved");
    fs::write(&footprint, committed).unwrap();

    // The Catalog's own symbol with the hand-drawn footprint: the symbol is
    // used as the Catalog has it, and only the footprint and its 3D model
    // are sent.
    let reading = team.read(choice(Some("g-cap:C"), Some(FOOTPRINT)));
    let library = reading.library.clone().unwrap();
    assert_eq!(library.in_catalog, ["g-cap:C"]);
    assert!(library.problems.is_empty(), "{library:?}");
    assert!(
        reading.notes.iter().any(|note| note
            == "The Catalog already has g-cap:C, so the part uses it as the Catalog has it."),
        "{:?}",
        reading.notes
    );
    let part = added(team.add(new_part(&reading, "CAP-0099-0001", "cap")));
    assert_eq!(part.revision.field("Symbol"), Some("g-cap:C"));
    assert_eq!(team.stored_revisions(), revisions + 2);

    // Typing another symbol than the one chosen uses that one instead, so
    // the chosen symbol is not sent.
    let reading = team.read(choice(Some(SYMBOL), Some(FOOTPRINT)));
    let mut request = new_part(&reading, "CAP-0099-0002", "cap");
    request.symbol = "g-cap:C".into();
    let part = added(team.add(request));
    assert_eq!(part.revision.field("Symbol"), Some("g-cap:C"));
    assert_eq!(team.stored_revisions(), revisions + 2);
}

/// Items added from our libraries are imported CAD, not CAD made in the
/// app: someone redraws the footprint in KiCad and commits, and the next CSV
/// import that names it gives it a new revision, with no false "changed in
/// this app" notice. The part itself was added in the app, so it keeps its
/// revision and offers the new footprint as a CSV change to accept.
#[test]
fn a_later_csv_import_updates_items_added_from_our_libraries() {
    let team = Team::start(draw_by_hand);
    let root = team.root().to_path_buf();
    let reading = team.read(choice(Some(SYMBOL), None));
    let cap = reading
        .categories
        .iter()
        .find(|category| category.code == "cap")
        .unwrap();
    let part = added(team.add(new_part(&reading, &cap.next_ipn, "cap")));

    // Redrawn in KiCad with a link only one computer has: like any imported
    // footprint changed since its import, it is to fix.
    let footprint = root.join(FOOTPRINT_FILE);
    let saved = fs::read_to_string(&footprint).unwrap();
    let redrawn = saved.replacen(
        "(attr",
        "(descr \"redrawn\")
	(attr",
        1,
    );
    fs::write(
        &footprint,
        redrawn.replace(
            &format!("${{GITPLM_PARTS}}/{MODEL_FILE}"),
            "/home/me/C_0603_Hand.step",
        ),
    )
    .unwrap();
    let check = team.check();
    let to_fix = group(&check, "To fix");
    assert_eq!(to_fix, [FOOTPRINT], "{check:#?}");
    assert!(
        check.groups[0].items[0]
            .message
            .ends_with("The footprint changed since the last import."),
        "{check:#?}"
    );

    // Fixed, committed, and its CSV row added: the import gives the
    // footprint revision 2.
    fs::write(&footprint, &redrawn).unwrap();
    add_csv_row(&root, &part);
    commit_all(&root, "Redrawn in KiCad, with its CSV row");
    let outcome = import(&team.admin, &root, "g-cap");
    assert!(
        matches!(outcome, ImportOutcome::Applied { .. }),
        "{outcome:?}"
    );
    let stored = read_back(&team.admin).unwrap();
    let item = stored
        .cad_items
        .iter()
        .find(|item| item.key == FOOTPRINT)
        .unwrap();
    let newest = stored
        .cad_item_revisions
        .iter()
        .filter(|revision| revision.cad_item == item.id)
        .max_by_key(|revision| revision.number)
        .unwrap();
    assert_eq!(newest.number, 2);
    assert!(
        newest.source.contains("\"source\":\"csv-catalog\""),
        "{}",
        newest.source
    );
    assert!(
        !stored
            .problems
            .iter()
            .any(|problem| problem.code == catalog::CSV_CAD_DIFFERS_FROM_EDITED_CAD_ITEM),
        "{:?}",
        stored.problems
    );

    // The part keeps its revision and offers the new footprint, which can
    // be accepted.
    let now = team.team.part(&part.ipn).unwrap().unwrap();
    assert_eq!(now.revision.number, 1);
    let cad = now
        .csv_changes
        .iter()
        .find(|changes| changes.cad)
        .unwrap_or_else(|| panic!("{:?}", now.csv_changes));
    let row = cad
        .rows
        .iter()
        .find(|row| row.field == "Footprint")
        .unwrap_or_else(|| panic!("{cad:?}"));
    assert!(!row.keep_only, "{row:?}");
    let reply = team
        .team
        .csv_review(TeamCsvReviewRequest {
            correlation_id: None,
            ipn: part.ipn.clone(),
            problem: cad.problem.clone(),
            accept: vec!["Footprint".into()],
            expected: cad.expected,
            reason: "Take the redrawn footprint".into(),
        })
        .unwrap();
    assert!(
        matches!(
            reply,
            PublishResult::CsvReviewed {
                revision: Some(2),
                ..
            }
        ),
        "{reply:?}"
    );
    assert_eq!(team.check().to_fix, 0);
}
