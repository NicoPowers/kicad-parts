//! Choosing symbols and footprints in the Libraries browser for Edit part
//! and Add part (kicad-parts-bc6f), through catalog-browser against a
//! disposable PocketBase. What choosing does depends on where the item
//! lives: the Catalog's own is bound, ours that the Catalog lacks is added
//! from the committed folder, KiCad's own is referred to as KiCad's, and
//! another installed library's is copied into one of our libraries with its
//! 3D model, the Source naming the library and file.
//!
//! Our libraries are the sample library, committed in a throwaway git
//! repository of its own (`sample_library::SampleRepo`); KiCad is a fake
//! install (its data folder only) and the person's
//! KiCad settings are fixture tables, never the developer's own.

use catalog::pocketbase::read_back;
use catalog_browser::team::TeamCatalog;
use catalog_browser::{BrowserConfig, CatalogBrowser};
use catalog_import::services;
use collaboration::PocketBaseClient;
use desktop_contract::{
    Field, LibraryGroup, LibraryKey, LiveUpdates, ManufacturerPart, PickUse, PreviewKind,
    PublishResult, SignInRequest, TeamDownload, TeamDownloadRequest, TeamEditRequest, TeamFile,
    TeamNewPartRequest, TeamPart, TeamPick, TeamPickRequest, TeamSession,
};
use kicad_integration::SearchLocations;
use native_services::PocketBaseServer;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const IPN: &str = "CAP-0000-0100";
const HAND: &str = "g-Capacitor_SMD:C_0603_Hand";
const HAND_FILE: &str = "footprints/g-Capacitor_SMD.pretty/C_0603_Hand.kicad_mod";
const HAND_MODEL: &str = "3d-models/g-Capacitor_SMD.3dshapes/C_0603_Hand.step";
const MODEL: &[u8] = b"ISO-10303-21;\nEND-ISO-10303-21;\n";

fn wait_until(what: &str, mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !done() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn write(path: &Path, bytes: impl AsRef<[u8]>) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, bytes).unwrap();
}

fn text(root: &Path, path: &str) -> String {
    String::from_utf8(fs::read(root.join(path)).unwrap()).unwrap()
}

/// The 0603 capacitor footprint of the library, named `name` and linking
/// the 3D model `link`.
fn footprint_like(root: &Path, name: &str, link: &str) -> String {
    text(
        root,
        "footprints/g-Capacitor_SMD.pretty/C_0603_1608Metric.kicad_mod",
    )
    .replace(
        "(footprint \"C_0603_1608Metric\"",
        &format!("(footprint \"{name}\""),
    )
    .replace(
        "${GITPLM_PARTS}/3d-models/g-Capacitor_SMD.3dshapes/C_0603_1608Metric.step",
        link,
    )
}

/// The library's capacitor symbol `C` as a library of its own, named
/// `name`.
fn symbol_like(root: &Path, name: &str) -> String {
    let library = fs::read(root.join("symbols/g-cap.kicad_sym")).unwrap();
    let parsed = kicad_integration::library_file::SymbolLibrary::parse(&library).unwrap();
    let kicad_integration::library_file::SymbolLookup::Found(entry) = parsed.find("C") else {
        panic!("no C symbol");
    };
    let entry = String::from_utf8(library[entry.span.clone()].to_vec())
        .unwrap()
        .replace("(symbol \"C_", &format!("(symbol \"{name}_"))
        .replacen("(symbol \"C\"", &format!("(symbol \"{name}\""), 1);
    format!(
        "(kicad_symbol_lib\n\t(version {})\n\t(generator \"kicad_symbol_editor\")\n\t{entry}\n)\n",
        parsed.version.unwrap()
    )
}

/// A fake KiCad 10 install whose data folder holds KiCad's own capacitor
/// symbol and 0603 footprint, and the person's KiCad settings with one
/// installed symbol library and one footprint library: a footprint whose 3D
/// model can be read and one whose model is gone.
struct Kicad {
    install: PathBuf,
    config: PathBuf,
    mine: PathBuf,
}

fn kicad(root: &Path, catalog: &Path) -> Kicad {
    let install = root.join("install");
    let kicad = install.join("KiCad").join("10.0");
    let cli = if cfg!(windows) {
        "kicad-cli.exe"
    } else {
        "kicad-cli"
    };
    write(&kicad.join("bin").join(cli), "");
    let share = kicad.join("share").join("kicad");
    write(
        &share.join("symbols").join("Device.kicad_sym"),
        symbol_like(catalog, "C"),
    );
    write(
        &share
            .join("footprints")
            .join("Capacitor_SMD.pretty")
            .join("C_0603_1608Metric.kicad_mod"),
        footprint_like(
            catalog,
            "C_0603_1608Metric",
            "${KICAD10_3DMODEL_DIR}/Capacitor_SMD.3dshapes/C_0603_1608Metric.step",
        ),
    );
    let mine = root.join("mine");
    write(&mine.join("Mine.kicad_sym"), symbol_like(catalog, "C_Mine"));
    write(
        &mine.join("Mine.pretty").join("C_0603_Mine.kicad_mod"),
        footprint_like(catalog, "C_0603_Mine", "${MY_LIBS}/models/C 0603 Mine.step"),
    );
    write(&mine.join("models").join("C 0603 Mine.step"), MODEL);
    write(
        &mine.join("Mine.pretty").join("C_0603_Lost.kicad_mod"),
        footprint_like(catalog, "C_0603_Lost", "${MY_LIBS}/models/gone.step"),
    );
    // A link to a file that is no 3D model, and one with no folder.
    write(
        &mine.join("secret.env"),
        "TOKEN=not for the team server
",
    );
    write(
        &mine.join("Mine.pretty").join("C_0603_Key.kicad_mod"),
        footprint_like(catalog, "C_0603_Key", "${MY_LIBS}/secret.env"),
    );
    write(
        &mine.join("Mine.pretty").join("C_0603_Bare.kicad_mod"),
        footprint_like(catalog, "C_0603_Bare", "models/C 0603 Mine.step"),
    );
    // Footprints named like KiCad's, and like another installed library's.
    let kicad_model = "${KICAD10_3DMODEL_DIR}/Capacitor_SMD.3dshapes/C_0603_1608Metric.step";
    write(
        &mine.join("Mine.pretty").join("C_0603_1608Metric.kicad_mod"),
        footprint_like(catalog, "C_0603_1608Metric", kicad_model).replacen(
            "(attr",
            "(descr \"mine\")
	(attr",
            1,
        ),
    );
    for (folder, words) in [("Mine.pretty", "mine"), ("Other.pretty", "other")] {
        write(
            &mine.join(folder).join("C_0603_Twin.kicad_mod"),
            footprint_like(catalog, "C_0603_Twin", kicad_model).replacen(
                "(attr",
                &format!(
                    "(descr \"{words}\")
	(attr"
                ),
                1,
            ),
        );
    }
    let config = root.join("kicad-config");
    write(
        &config.join("kicad_common.json"),
        format!(
            "{{\"environment\": {{\"vars\": {{\"MY_LIBS\": {}}}}}}}",
            serde_json::to_string(&mine.to_string_lossy()).unwrap()
        ),
    );
    write(
        &config.join("sym-lib-table"),
        "(sym_lib_table (version 7) (lib (name \"MySymbols\")(type \"KiCad\")(uri \"${MY_LIBS}/Mine.kicad_sym\")(options \"\")(descr \"\")))",
    );
    write(
        &config.join("fp-lib-table"),
        "(fp_lib_table (version 7) (lib (name \"MyFootprints\")(type \"KiCad\")(uri \"${MY_LIBS}/Mine.pretty\")(options \"\")(descr \"\")) (lib (name \"OtherFootprints\")(type \"KiCad\")(uri \"${MY_LIBS}/Other.pretty\")(options \"\")(descr \"\")))",
    );
    Kicad {
        install,
        config,
        mine,
    }
}

/// A team server with the sample capacitors, our libraries in a committed
/// copy of the sample library (in which someone drew the footprint
/// C_0603_Hand with its own 3D model and committed it), an editor signed
/// in, and the desktop's browser over the copy with the libraries read.
struct Team {
    _server: PocketBaseServer,
    admin: PocketBaseClient,
    copy: sample_library::SampleRepo,
    head: String,
    _state: tempfile::TempDir,
    _data: tempfile::TempDir,
    kicad: Kicad,
    team: TeamCatalog,
    browser: CatalogBrowser,
}

impl Team {
    fn start() -> Self {
        let root = services::repository_root();
        let state = tempfile::tempdir().unwrap();
        let (server, admin, _superuser) = services::start(&root, state.path()).unwrap();
        let copy = sample_library::SampleRepo::new();
        sample_library::seed(&admin, &["cap"]).unwrap();
        write(
            &copy.path().join(HAND_FILE),
            footprint_like(
                copy.path(),
                "C_0603_Hand",
                &format!("${{GITPLM_PARTS}}/{HAND_MODEL}"),
            ),
        );
        write(&copy.path().join(HAND_MODEL), MODEL);
        let head = copy.commit_all("Drawn by hand in KiCad");
        let member = services::add_member(&admin, "Ada Editor", "ada@example.test", true).unwrap();
        let url = server.url().to_owned();
        let team = TeamCatalog::new();
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
        wait_until("every sample capacitor", || {
            team.current_values().len() >= 3
        });
        let data = tempfile::tempdir().unwrap();
        let kicad = kicad(data.path(), copy.path());
        let browser = CatalogBrowser::open(BrowserConfig {
            catalog_root: Some(copy.path().to_path_buf()),
            cache_dir: data.path().join("cache"),
            settings_file: data.path().join("settings.json"),
            kicad_locations: SearchLocations::Only(vec![kicad.install.clone()]),
            kicad_config_dir: Some(kicad.config.clone()),
            kicad_documents_dir: None,
        });
        browser.libraries(true);
        assert!(browser.wait_for_libraries(Duration::from_secs(120)));
        Self {
            _server: server,
            admin,
            copy,
            head,
            _state: state,
            _data: data,
            kicad,
            team,
            browser,
        }
    }

    fn root(&self) -> &Path {
        self.copy.path()
    }

    fn pick(&self, kind: PreviewKind, group: LibraryGroup, library: &str, name: &str) -> TeamPick {
        self.browser
            .pick(&request(kind, group, library, name), &self.team)
            .unwrap()
    }

    fn part(&self, ipn: &str) -> TeamPart {
        self.team.part(ipn).unwrap().unwrap()
    }

    /// Saves Edit part for `ipn` with the Symbol or Footprint `field` set to
    /// `value` and `files` sent.
    fn edit(&self, ipn: &str, field: &str, value: &str, files: Vec<TeamFile>) -> PublishResult {
        let part = self.part(ipn);
        let mut cad_fields = part.revision.cad_fields.clone();
        cad_fields
            .iter_mut()
            .find(|cad| cad.name == field)
            .unwrap()
            .value = value.to_owned();
        self.browser
            .edit(
                TeamEditRequest {
                    correlation_id: None,
                    ipn: ipn.into(),
                    expected_details: part.details.number,
                    expected_revision: part.revision.number,
                    reason: "Chosen in the Libraries browser".into(),
                    category: String::new(),
                    fields: part.details.fields.clone(),
                    manufacturer_parts: part.details.manufacturer_parts.clone(),
                    cad_fields,
                    files,
                },
                &self.team,
            )
            .unwrap()
    }

    /// The Source of revision 1 of the CAD item `key` of `kind`.
    fn source(&self, kind: catalog::CadItemKind, key: &str) -> serde_json::Value {
        let stored = read_back(&self.admin).unwrap();
        let item = catalog::CadItemId::new(kind, key);
        let first = stored
            .cad_item_revision(catalog::CadItemRevisionId::new(&item, 1).as_str())
            .unwrap_or_else(|| panic!("no {key}"));
        serde_json::from_str(&first.source).unwrap()
    }

    /// The stored bytes of revision 1 of the CAD item `key` of `kind`.
    fn stored(&self, kind: catalog::CadItemKind, key: &str) -> Vec<u8> {
        let stored = read_back(&self.admin).unwrap();
        let item = catalog::CadItemId::new(kind, key);
        let first = stored
            .cad_item_revision(catalog::CadItemRevisionId::new(&item, 1).as_str())
            .unwrap();
        stored.blob(&first.blob).unwrap().bytes.clone()
    }

    fn read(&self, picks: Vec<TeamPickRequest>) -> TeamDownload {
        self.browser
            .read_download(
                &TeamDownloadRequest {
                    files: Vec::new(),
                    symbol: None,
                    footprint: None,
                    without_models: false,
                    picks,
                },
                &self.team,
            )
            .unwrap()
    }
}

fn request(kind: PreviewKind, group: LibraryGroup, library: &str, name: &str) -> TeamPickRequest {
    TeamPickRequest {
        kind,
        library: LibraryKey {
            group,
            nickname: library.into(),
            team: String::new(),
        },
        name: name.into(),
    }
}

fn saved(result: PublishResult) -> TeamPart {
    match result {
        PublishResult::Saved { part, .. } => part,
        other => panic!("not saved: {other:?}"),
    }
}

fn refused(result: PublishResult) -> String {
    match result {
        PublishResult::Refused { message } => message,
        other => panic!("not refused: {other:?}"),
    }
}

fn added(result: PublishResult) -> TeamPart {
    match result {
        PublishResult::Added { part } => part,
        other => panic!("not added: {other:?}"),
    }
}

/// Edit part chooses a footprint and a symbol from each place: the
/// Catalog's own is bound, ours the Catalog lacks is added from the commit,
/// KiCad's own is referred to, and an installed library's is copied into
/// the library chosen for it, with its 3D model when it can be read.
#[test]
fn edit_part_uses_a_choice_by_where_it_lives() {
    let team = Team::start();
    let footprint = || {
        team.part(IPN)
            .revision
            .field("Footprint")
            .unwrap()
            .to_owned()
    };

    // The Catalog's own footprint: bound to its current revision.
    let pick = team.pick(
        PreviewKind::Footprint,
        LibraryGroup::Ours,
        "g-Capacitor_SMD",
        "C_0805_2012Metric",
    );
    assert_eq!(pick.use_as, PickUse::Catalog);
    assert_eq!(pick.reference, "g-Capacitor_SMD:C_0805_2012Metric");
    assert!(pick.file.is_none() && pick.problems.is_empty(), "{pick:?}");
    saved(team.edit(IPN, "Footprint", &pick.reference, Vec::new()));
    assert_eq!(footprint(), "g-Capacitor_SMD:C_0805_2012Metric");

    // Ours, which the Catalog lacks: added from the commit that holds it.
    let pick = team.pick(
        PreviewKind::Footprint,
        LibraryGroup::Ours,
        "g-Capacitor_SMD",
        "C_0603_Hand",
    );
    assert_eq!(pick.use_as, PickUse::Ours);
    assert!(pick.problems.is_empty(), "{pick:?}");
    saved(team.edit(IPN, "Footprint", HAND, Vec::new()));
    assert_eq!(footprint(), HAND);
    for (kind, key, path) in [
        (catalog::CadItemKind::Footprint, HAND, HAND_FILE),
        (catalog::CadItemKind::Model3d, HAND_MODEL, HAND_MODEL),
    ] {
        let source = team.source(kind, key);
        assert_eq!(source["source"], "library", "{source}");
        assert_eq!(source["path"], path);
        assert_eq!(source["source_commit"], team.head.as_str());
    }

    // KiCad's own symbol: referred to as KiCad's, with nothing stored.
    let pick = team.pick(PreviewKind::Symbol, LibraryGroup::Kicad, "Device", "C");
    assert_eq!(pick.use_as, PickUse::Kicad);
    assert!(pick.file.is_none(), "{pick:?}");
    let part = saved(team.edit(IPN, "Symbol", "Device:C", Vec::new()));
    assert_eq!(part.revision.field("Symbol"), Some("Device:C"));

    // An installed library's footprint: copied into the library chosen
    // for it, its 3D model with it, each Source naming the library and file.
    let pick = team.pick(
        PreviewKind::Footprint,
        LibraryGroup::Installed,
        "MyFootprints",
        "C_0603_Mine",
    );
    assert_eq!(pick.use_as, PickUse::Copy);
    assert!(
        pick.problems.is_empty() && pick.model_problems.is_empty(),
        "{pick:?}"
    );
    let file = pick.file.clone().unwrap();
    let installed = file.installed.clone().unwrap();
    assert_eq!(installed.library, "MyFootprints");
    // The file inside its library, never the person's home folder.
    let footprint_path = "C_0603_Mine.kicad_mod";
    assert_eq!(installed.path, footprint_path);
    assert_eq!(pick.models.len(), 1, "{pick:?}");
    let mut files = vec![file];
    files.extend(pick.models.clone());
    saved(team.edit(IPN, "Footprint", "g-Capacitor_SMD:C_0603_Mine", files));
    let copied = "g-Capacitor_SMD:C_0603_Mine";
    let model_key = "3d-models/g-Capacitor_SMD.3dshapes/C 0603 Mine.step";
    let source = team.source(catalog::CadItemKind::Footprint, copied);
    assert_eq!(source["source"], "installed", "{source}");
    assert_eq!(source["library"], "MyFootprints");
    assert_eq!(source["path"], footprint_path);
    let stored = String::from_utf8(team.stored(catalog::CadItemKind::Footprint, copied)).unwrap();
    assert!(
        stored.contains(&format!("\"${{GITPLM_PARTS}}/{model_key}\"")),
        "the link points at the copy: {stored}"
    );
    assert!(!stored.contains("MY_LIBS"), "{stored}");
    let source = team.source(catalog::CadItemKind::Model3d, model_key);
    assert_eq!(source["source"], "installed", "{source}");
    // The link as the footprint wrote it.
    assert_eq!(source["path"], "${MY_LIBS}/models/C 0603 Mine.step");
    assert_eq!(team.stored(catalog::CadItemKind::Model3d, model_key), MODEL);

    // A 3D model that cannot be read stops the save plainly, unless No 3D
    // model is chosen: then the footprint is copied without its link.
    let pick = team.pick(
        PreviewKind::Footprint,
        LibraryGroup::Installed,
        "MyFootprints",
        "C_0603_Lost",
    );
    assert!(pick.models.is_empty(), "{pick:?}");
    assert_eq!(pick.model_problems.len(), 1, "{pick:?}");
    assert!(
        pick.model_problems[0]
            .starts_with("Its 3D model ${MY_LIBS}/models/gone.step could not be read"),
        "{pick:?}"
    );
    let lost = "g-Capacitor_SMD:C_0603_Lost";
    let message = refused(team.edit(IPN, "Footprint", lost, vec![pick.file.clone().unwrap()]));
    assert!(
        message.contains("${MY_LIBS}/models/gone.step could not be read")
            && message.contains("Choose No 3D model"),
        "{message}"
    );
    let mut without = pick.file.clone().unwrap();
    without.no_models = true;
    saved(team.edit(IPN, "Footprint", lost, vec![without]));
    let stored = String::from_utf8(team.stored(catalog::CadItemKind::Footprint, lost)).unwrap();
    assert!(!stored.contains("(model"), "{stored}");

    // An installed library's symbol: copied into the library chosen for it.
    let pick = team.pick(
        PreviewKind::Symbol,
        LibraryGroup::Installed,
        "MySymbols",
        "C_Mine",
    );
    assert_eq!(pick.use_as, PickUse::Copy);
    assert!(pick.problems.is_empty(), "{pick:?}");
    let part = saved(team.edit(
        IPN,
        "Symbol",
        "g-cap:C_Mine",
        vec![pick.file.clone().unwrap()],
    ));
    assert_eq!(part.revision.field("Symbol"), Some("g-cap:C_Mine"));
    let source = team.source(catalog::CadItemKind::Symbol, "g-cap:C_Mine");
    assert_eq!(source["source"], "installed", "{source}");
    assert_eq!(source["library"], "MySymbols");

    // Ours with a change that is not committed cannot be added: said when
    // chosen and when saving, and nothing is saved.
    let drawn = "footprints/g-Capacitor_SMD.pretty/C_0603_Late.kicad_mod";
    write(
        &team.root().join(drawn),
        footprint_like(team.root(), "C_0603_Late", "${KICAD10_3DMODEL_DIR}/x.step"),
    );
    let browser_again = &team.browser;
    browser_again.libraries(true);
    assert!(browser_again.wait_for_libraries(Duration::from_secs(120)));
    let pick = team.pick(
        PreviewKind::Footprint,
        LibraryGroup::Ours,
        "g-Capacitor_SMD",
        "C_0603_Late",
    );
    assert_eq!(pick.use_as, PickUse::Ours);
    assert!(
        pick.problems
            .iter()
            .any(|problem| problem.contains("is not committed")),
        "{pick:?}"
    );
    let message = refused(team.edit(IPN, "Footprint", "g-Capacitor_SMD:C_0603_Late", Vec::new()));
    assert!(message.contains("is not committed"), "{message}");

    // Only KiCad's 3D model files are copied, and a link must say where.
    for (name, says) in [
        ("C_0603_Key", "is not a 3D model file"),
        ("C_0603_Bare", "has no folder or path variable"),
    ] {
        let pick = team.pick(
            PreviewKind::Footprint,
            LibraryGroup::Installed,
            "MyFootprints",
            name,
        );
        assert!(pick.models.is_empty(), "{pick:?}");
        assert!(
            pick.model_problems.len() == 1 && pick.model_problems[0].contains(says),
            "{says}: {pick:?}"
        );
    }

    // A copy named like KiCad's first footprint goes to the second choice,
    // the one it was chosen for.
    let pick = team.pick(
        PreviewKind::Footprint,
        LibraryGroup::Installed,
        "MyFootprints",
        "C_0603_1608Metric",
    );
    let mut file = pick.file.clone().unwrap();
    file.choice = Some(1);
    let both = "Capacitor_SMD:C_0603_1608Metric;g-Inductor_SMD:C_0603_1608Metric";
    let part = saved(team.edit(IPN, "Footprint", both, vec![file]));
    assert_eq!(part.revision.field("Footprint"), Some(both));
    let source = team.source(
        catalog::CadItemKind::Footprint,
        "g-Inductor_SMD:C_0603_1608Metric",
    );
    assert_eq!(source["source"], "installed", "{source}");

    // Two copies of one name from two installed libraries each go to their
    // own choice.
    let twin = |library: &str, choice: usize| {
        let pick = team.pick(
            PreviewKind::Footprint,
            LibraryGroup::Installed,
            library,
            "C_0603_Twin",
        );
        let mut file = pick.file.unwrap();
        file.choice = Some(choice);
        file
    };
    let twins = "g-Capacitor_SMD:C_0603_Twin;g-Inductor_SMD:C_0603_Twin";
    saved(team.edit(
        IPN,
        "Footprint",
        twins,
        vec![twin("OtherFootprints", 1), twin("MyFootprints", 0)],
    ));
    for (key, library, words) in [
        ("g-Capacitor_SMD:C_0603_Twin", "MyFootprints", "mine"),
        ("g-Inductor_SMD:C_0603_Twin", "OtherFootprints", "other"),
    ] {
        let source = team.source(catalog::CadItemKind::Footprint, key);
        assert_eq!(source["library"], library, "{source}");
        let stored = String::from_utf8(team.stored(catalog::CadItemKind::Footprint, key)).unwrap();
        assert!(
            stored.contains(&format!("(descr \"{words}\")")),
            "{key}: {stored}"
        );
    }
}

/// Add part From a library: KiCad's generic capacitor becomes a new part
/// that refers to KiCad's own symbol and footprint, and an installed
/// library's symbol and footprint are copied into our libraries with the 3D
/// model, each recording where it came from.
#[test]
fn add_part_from_kicads_and_an_installed_library() {
    let team = Team::start();

    // KiCad's generic capacitor and footprint: external references, a new
    // IPN in the category guessed.
    let reading = team.read(vec![
        request(PreviewKind::Symbol, LibraryGroup::Kicad, "Device", "C"),
        request(
            PreviewKind::Footprint,
            LibraryGroup::Kicad,
            "Capacitor_SMD",
            "C_0603_1608Metric",
        ),
    ]);
    let uses: Vec<PickUse> = reading.picks.iter().map(|pick| pick.use_as).collect();
    assert_eq!(uses, [PickUse::Kicad, PickUse::Kicad]);
    assert!(reading.symbol_file.is_none() && reading.footprint_file.is_none());
    assert_eq!(reading.from, "KiCad's libraries");
    assert_eq!((reading.pins, reading.pads), (Some(2), Some(2)));
    let cap = reading
        .categories
        .iter()
        .find(|category| category.code == "cap")
        .unwrap();
    assert_eq!(
        reading.guesses.first().map(|guess| guess.code.as_str()),
        Some("cap"),
        "{:?}",
        reading.guesses
    );
    let request_for =
        |reading: &TeamDownload, ipn: &str, symbol: &str, footprint: &str, files: Vec<TeamFile>| {
            let cap = reading
                .categories
                .iter()
                .find(|category| category.code == "cap")
                .unwrap();
            TeamNewPartRequest {
                correlation_id: None,
                ipn: ipn.into(),
                reason: "A generic capacitor for the prototype".into(),
                category: "cap".into(),
                fields: cap
                    .fields
                    .iter()
                    .map(|field| Field {
                        name: field.name.clone(),
                        value: if field.name == "Value" {
                            "100n".into()
                        } else {
                            field.value.clone()
                        },
                    })
                    .collect(),
                manufacturer_parts: vec![ManufacturerPart {
                    manufacturer: String::new(),
                    mpn: String::new(),
                }],
                symbol: symbol.into(),
                footprint: footprint.into(),
                files,
                from: reading.from.clone(),
                downloads: Vec::new(),
            }
        };
    let generic = added(
        team.browser
            .add_part(
                request_for(
                    &reading,
                    &cap.next_ipn,
                    "Device:C",
                    "Capacitor_SMD:C_0603_1608Metric",
                    Vec::new(),
                ),
                &team.team,
            )
            .unwrap(),
    );
    assert_eq!(generic.revision.field("Symbol"), Some("Device:C"));
    let first = generic.history.first().unwrap();
    let from = first
        .changes
        .iter()
        .find(|row| row.label == "Added from")
        .unwrap_or_else(|| panic!("{:?}", first.changes));
    assert_eq!(from.after, "KiCad's libraries");

    // An installed library's symbol and footprint: copied, with the 3D
    // model, each Source naming the library and file.
    let reading = team.read(vec![
        request(
            PreviewKind::Symbol,
            LibraryGroup::Installed,
            "MySymbols",
            "C_Mine",
        ),
        request(
            PreviewKind::Footprint,
            LibraryGroup::Installed,
            "MyFootprints",
            "C_0603_Mine",
        ),
    ]);
    let uses: Vec<PickUse> = reading.picks.iter().map(|pick| pick.use_as).collect();
    assert_eq!(uses, [PickUse::Copy, PickUse::Copy]);
    assert_eq!(reading.from, "Installed library MySymbols");
    assert_eq!(reading.footprint_library, "g-Capacitor_SMD");
    let mut files: Vec<TeamFile> = reading.symbol_file.iter().cloned().collect();
    files.extend(reading.footprint_file.iter().cloned());
    files.extend(reading.model_files.iter().cloned());
    assert_eq!(files.len(), 3, "{files:?}");
    let ipn = format!("{}2", &generic.ipn[..generic.ipn.len() - 1]);
    let part = added(
        team.browser
            .add_part(
                request_for(
                    &reading,
                    &ipn,
                    "g-cap:C_Mine",
                    "g-Capacitor_SMD:C_0603_Mine",
                    files,
                ),
                &team.team,
            )
            .unwrap(),
    );
    assert_eq!(
        part.revision.field("Footprint"),
        Some("g-Capacitor_SMD:C_0603_Mine")
    );
    for (kind, key, library) in [
        (catalog::CadItemKind::Symbol, "g-cap:C_Mine", "MySymbols"),
        (
            catalog::CadItemKind::Footprint,
            "g-Capacitor_SMD:C_0603_Mine",
            "MyFootprints",
        ),
        (
            catalog::CadItemKind::Model3d,
            "3d-models/g-Capacitor_SMD.3dshapes/C 0603 Mine.step",
            "MyFootprints",
        ),
    ] {
        let source = team.source(kind, key);
        assert_eq!(source["source"], "installed", "{source}");
        assert_eq!(source["library"], library, "{source}");
    }

    // A symbol chosen alone brings the footprint it names when a library
    // has it.
    let named = symbol_like(team.root(), "C_Named").replacen(
        "(property \"Footprint\" \"\"",
        "(property \"Footprint\" \"Capacitor_SMD:C_0603_1608Metric\"",
        1,
    );
    assert!(named.contains("Capacitor_SMD:C_0603_1608Metric"));
    write(&team.kicad.mine.join("Mine.kicad_sym"), named);
    team.browser.libraries(true);
    assert!(team.browser.wait_for_libraries(Duration::from_secs(120)));
    let reading = team.read(vec![request(
        PreviewKind::Symbol,
        LibraryGroup::Installed,
        "MySymbols",
        "C_Named",
    )]);
    let picked: Vec<(PreviewKind, PickUse, &str)> = reading
        .picks
        .iter()
        .map(|pick| (pick.kind, pick.use_as, pick.reference.as_str()))
        .collect();
    assert_eq!(
        picked,
        [
            (PreviewKind::Symbol, PickUse::Copy, "MySymbols:C_Named"),
            (
                PreviewKind::Footprint,
                PickUse::Kicad,
                "Capacitor_SMD:C_0603_1608Metric"
            ),
        ]
    );
    assert!(
        reading.notes.iter().any(|note| note
            == "The footprint is the one the symbol names, Capacitor_SMD:C_0603_1608Metric."),
        "{:?}",
        reading.notes
    );
}
