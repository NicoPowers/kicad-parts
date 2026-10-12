//! New parts added from downloaded CAD files (seed kicad-parts-9693): one
//! Change of kind create holds the part with part details 1 and revision 1,
//! and revision 1 of each new CAD item, read and checked by the server as
//! for an edit's files. The person confirms the category, so its clues are
//! remembered. The server refuses an IPN the Catalog has, a malformed IPN,
//! unsafe names, files nothing uses and people who cannot edit, and replays
//! a repeated save.
//!
//! Each test starts its own disposable PocketBase on 127.0.0.1 and imports a
//! temp copy of the resistor CSV file; the repository is only read.

use catalog::cad_file::{footprint_upload, model_upload, symbol_upload};
use catalog::pocketbase::{
    CadUpload, CatalogReadback, CreateCommand, CreateOutcome, CreateReply, ImportCommand,
    ImportOutcome, NEW_PART_ROUTE, read_back, read_import_state, submit_create, submit_import,
};
use catalog::{
    CadItemDetails, CadReferenceTarget, CsvCatalog, DetailField, ManufacturerPart, ModelTarget,
    PartDetails, PartRevision,
};
use catalog_import::services;
use collaboration::{CollabError, PocketBaseClient};
use native_services::PocketBaseServer;
use serde_json::{Value, json};
use std::fs;

const IPN: &str = "RES-0001-0001";
const SYMBOL: &str = "g-res:R_New";
const FOOTPRINT: &str = "g-Resistor_SMD:R_0603_New";
const MODEL: &str = "3d-models/g-Resistor_SMD.3dshapes/R_0603_New.step";

struct Team {
    server: PocketBaseServer,
    admin: PocketBaseClient,
    _copy: tempfile::TempDir,
    _state: tempfile::TempDir,
}

impl Team {
    fn start() -> Self {
        let root = services::repository_root();
        let state = tempfile::tempdir().unwrap();
        let (server, admin, _superuser) = services::start(&root, state.path()).unwrap();
        let copy = tempfile::tempdir().unwrap();
        for tree in ["database", "symbols", "footprints", "3d-models"] {
            copy_tree(&root.join(tree), &copy.path().join(tree));
        }
        let import = CsvCatalog::new(copy.path()).import("g-res", None).unwrap();
        let state_now = read_import_state(&admin).unwrap();
        let command = ImportCommand::new(&import, "Import the resistors").expecting(&state_now);
        let reply = submit_import(&admin, &command).unwrap();
        assert!(
            matches!(reply.outcome, ImportOutcome::Applied { .. }),
            "{}",
            reply.body
        );
        Self {
            server,
            admin,
            _copy: copy,
            _state: state,
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

fn copy_tree(from: &std::path::Path, to: &std::path::Path) {
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

fn repository_file(path: &str) -> String {
    String::from_utf8(fs::read(services::repository_root().join(path)).unwrap()).unwrap()
}

/// The resistor symbol renamed R_New, with an MPN, as a vendor's library.
fn symbol_library() -> String {
    repository_file("symbols/g-res.kicad_sym").replace("(symbol \"R_US", "(symbol \"R_New")
}

/// The 0603 resistor footprint renamed R_0603_New, naming its own 3D model
/// in the Catalog's folder.
fn footprint_file() -> String {
    repository_file("footprints/g-Resistor_SMD.pretty/R_0603_1608Metric.kicad_mod")
        .replace(
            "(footprint \"R_0603_1608Metric\"",
            "(footprint \"R_0603_New\"",
        )
        .replace(
            "${KICAD9_3DMODEL_DIR}/Resistor_SMD.3dshapes/R_0603_1608Metric.step",
            &format!("${{GITPLM_PARTS}}/{MODEL}"),
        )
}

fn uploads() -> Vec<CadUpload> {
    vec![
        symbol_upload(
            symbol_library().as_bytes(),
            "R_New",
            SYMBOL,
            "KiCad/R_New.kicad_sym in R_New.zip",
        )
        .unwrap(),
        footprint_upload(
            footprint_file().as_bytes(),
            FOOTPRINT,
            "KiCad/R_0603_New.kicad_mod in R_New.zip",
        )
        .unwrap(),
        model_upload(
            b"ISO-10303-21;\nEND-ISO-10303-21;\n",
            MODEL,
            "3D/R_0603_New.step in R_New.zip",
        )
        .unwrap(),
    ]
}

fn field(name: &str, value: &str) -> DetailField {
    DetailField {
        name: name.into(),
        value: value.into(),
    }
}

/// A new resistor from the three files.
fn new_part(ipn: &str) -> CreateCommand {
    let mut command = CreateCommand::new(ipn, "res", "A new 0603 resistor");
    command.fields = vec![
        field("Description", "RES 1K 1% 1/10W 0603"),
        field("Value", "1k"),
        field("Tolerance", "1%"),
        field("Datasheet", ""),
    ];
    command.manufacturer_parts = vec![ManufacturerPart {
        manufacturer: "Acme Resistors".into(),
        mpn: "AR0603-1K".into(),
    }];
    command.cad_fields = vec![field("Symbol", SYMBOL), field("Footprint", FOOTPRINT)];
    command.uploads = uploads();
    command.from = "SamacSys (Component Search Engine)".into();
    command.files = vec!["R_New.zip".into()];
    command
}

fn applied(reply: &CreateReply) -> (u64, Vec<(String, String)>) {
    match &reply.outcome {
        CreateOutcome::Applied {
            seq,
            remembered_matches,
            details,
            revision,
            ..
        } => {
            assert_eq!((details.number, revision.number), (1, 1));
            (
                *seq,
                remembered_matches
                    .iter()
                    .map(|clue| (clue.clue.clone(), clue.value.clone()))
                    .collect(),
            )
        }
        other => panic!("expected applied, got {other:?}: {}", reply.body),
    }
}

#[test]
fn a_new_part_is_one_change_with_its_first_details_revision_and_cad_items() {
    let team = Team::start();
    let editor = team.member("Ada Editor", true);
    let before = team.read();
    let command = new_part(IPN);
    let reply = submit_create(&editor, &command).unwrap();
    let (seq, remembered) = applied(&reply);
    let stored = team.read();

    // One Change of kind create.
    let change = stored
        .changes
        .iter()
        .find(|change| change.seq == seq)
        .unwrap();
    assert_eq!(change.kind, "create");
    assert_eq!(change.reason, "A new 0603 resistor");
    assert_eq!(change.actor_name, "Ada Editor");
    assert_eq!(stored.changes.len(), before.changes.len() + 1);

    // The part, its details 1 and revision 1.
    let part = stored.part(IPN).unwrap();
    assert_eq!(
        (part.current_details_number, part.current_revision_number),
        (1, 1)
    );
    let details: PartDetails =
        serde_json::from_str(&stored.part_details(&part.current_details).unwrap().snapshot)
            .unwrap();
    assert_eq!(details.category, "res");
    assert_eq!(details.field("Value"), Some("1k"));
    assert_eq!(details.manufacturer_parts[0].mpn, "AR0603-1K");
    let revision_record = stored.part_revision(&part.current_revision).unwrap();
    let revision: PartRevision = serde_json::from_str(&revision_record.snapshot).unwrap();
    // The Source records the site and the downloaded files.
    let source: Value = serde_json::from_str(&revision_record.source).unwrap();
    assert_eq!(source["source"], "create");
    assert_eq!(source["from"], "SamacSys (Component Search Engine)");
    assert_eq!(source["files"], json!(["R_New.zip"]));

    // Revision 1 of each new CAD item, bound exactly; the footprint names
    // the 3D model sent with it.
    for (reference, kind) in [(SYMBOL, "symbol"), (FOOTPRINT, "footprint")] {
        let target = revision
            .cad_references
            .iter()
            .find(|item| item.reference == reference)
            .map(|item| item.target.clone())
            .unwrap();
        let CadReferenceTarget::CadItemRevision {
            cad_item, number, ..
        } = target
        else {
            panic!("{reference} is not bound: {target:?}");
        };
        assert_eq!(number, 1, "{reference}");
        let made = stored
            .cad_item_revisions
            .iter()
            .find(|made| made.cad_item == cad_item && made.number == 1)
            .unwrap();
        assert_eq!(made.license, "unknown-needs-review");
        let snapshot: CadItemDetails = serde_json::from_str(&made.snapshot).unwrap();
        assert_eq!(snapshot.kind.as_str(), kind);
        let made_source: Value = serde_json::from_str(&made.source).unwrap();
        assert_eq!(made_source["source"], "create");
        assert!(
            made_source["file_name"]
                .as_str()
                .unwrap()
                .ends_with("in R_New.zip"),
            "{made_source}"
        );
        if kind == "footprint" {
            assert!(
                matches!(
                    &snapshot.model_references[0].target,
                    ModelTarget::CadItemRevision { number: 1, .. }
                ),
                "{:?}",
                snapshot.model_references
            );
        }
    }
    assert_eq!(
        stored.cad_item_revisions.len(),
        before.cad_item_revisions.len() + 3
    );

    // The confirmed category's clues are remembered: the footprint family,
    // the symbol's reference letter and the manufacturer.
    assert_eq!(
        remembered,
        [
            ("footprint-family".to_owned(), "R".to_owned()),
            ("reference-letter".to_owned(), "R".to_owned()),
            ("manufacturer".to_owned(), "Acme Resistors".to_owned()),
        ]
    );
    let matches: Vec<_> = stored
        .category_matches
        .iter()
        .filter(|stored| stored.seq == seq)
        .collect();
    assert_eq!(matches.len(), 3);
    assert!(matches.iter().all(|stored| stored.part == part.id));

    // Sent again (as after a lost answer): the same answer, nothing more.
    let again = submit_create(&editor, &command).unwrap();
    assert!(again.replayed, "{}", again.body);
    assert_eq!((again.status, &again.body), (reply.status, &reply.body));
    assert_eq!(team.read().counts(), stored.counts());
}

#[test]
fn a_new_part_may_use_what_the_catalog_and_kicad_already_have() {
    let team = Team::start();
    let editor = team.member("Ada Editor", true);
    let mut command = new_part("RES-0002-0001");
    // The Catalog's own symbol, and KiCad's own footprint: no files.
    command.cad_fields = vec![
        field("Symbol", "g-res:R_US"),
        field("Footprint", "Resistor_SMD:R_0603_1608Metric"),
    ];
    command.uploads = Vec::new();
    let reply = submit_create(&editor, &command).unwrap();
    applied(&reply);
    let stored = team.read();
    let part = stored.part("RES-0002-0001").unwrap();
    let revision: PartRevision = serde_json::from_str(
        &stored
            .part_revision(&part.current_revision)
            .unwrap()
            .snapshot,
    )
    .unwrap();
    let targets: Vec<&CadReferenceTarget> = revision
        .cad_references
        .iter()
        .map(|item| &item.target)
        .collect();
    assert!(matches!(
        targets[0],
        CadReferenceTarget::CadItemRevision { .. }
    ));
    assert_eq!(*targets[1], CadReferenceTarget::External);
}

/// A change to a request's body.
type Change = Box<dyn Fn(&mut Value)>;

#[test]
fn a_new_part_is_refused_plainly_and_nothing_is_written() {
    let team = Team::start();
    let editor = team.member("Ada Editor", true);
    let viewer = team.member("Vic Viewer", false);
    let before = team.read();

    // An IPN the Catalog already has: someone took it first.
    let taken = submit_create(&editor, &new_part("RES-0000-0470")).unwrap();
    assert_eq!(taken.status, 409, "{}", taken.body);
    assert!(
        matches!(&taken.outcome, CreateOutcome::IpnTaken { message, .. } if message.contains("never reused")),
        "{}",
        taken.body
    );

    // Not allowed to edit.
    match submit_create(&viewer, &new_part(IPN)) {
        Err(CollabError::Forbidden(message)) => assert!(message.contains("edit"), "{message}"),
        other => panic!("expected 403, got {other:?}"),
    }

    let base = serde_json::to_value(new_part(IPN).body()).unwrap();
    let send = |change: &dyn Fn(&mut Value)| -> String {
        let mut body = base.clone();
        change(&mut body);
        body["correlationId"] = json!(format!("refused-{}", catalog::sha256_hex(body.to_string())));
        match editor.post_json(NEW_PART_ROUTE, &body) {
            Ok(reply) => match reply.success() {
                Err(CollabError::Http {
                    status: 400,
                    message,
                }) => message,
                other => panic!("expected 400, got {other:?}"),
            },
            Err(CollabError::Http {
                status: 400,
                message,
            }) => message,
            Err(other) => panic!("expected 400, got {other:?}"),
        }
    };
    let cases: Vec<(&str, Change)> = vec![
        (
            "is not an IPN",
            Box::new(|body| body["ipn"] = json!("RES-1-1")),
        ),
        // A category with an IPN prefix numbers its parts with it.
        (
            "has an IPN starting with RES-, not CAP-0001-0001",
            Box::new(|body| body["ipn"] = json!("CAP-0001-0001")),
        ),
        (
            "is not in the Catalog",
            Box::new(|body| body["category"] = json!("nope")),
        ),
        (
            "must list the category's 4 fields",
            Box::new(|body| body["fields"] = json!([{ "name": "Value", "value": "1k" }])),
        ),
        (
            "field 1 must be Description",
            Box::new(|body| {
                body["fields"][0]["name"] = json!("Notes");
            }),
        ),
        // Unsafe names, in a file's reference and in a typed one.
        (
            "cannot be used as a reference",
            Box::new(|body| {
                body["cadFields"][1]["value"] = json!("../evil:R_0603_New");
                body["uploads"] = json!([]);
            }),
        ),
        (
            "cannot be used as a reference",
            Box::new(|body| {
                body["uploads"][1]["reference"] = json!("g-Resistor_SMD:../R_0603_New");
            }),
        ),
        // A file nothing uses.
        (
            "is not used",
            Box::new(|body| {
                body["cadFields"][1]["value"] = json!("Resistor_SMD:R_0603_1608Metric");
            }),
        ),
        // A 3D model no footprint names.
        (
            "names it",
            Box::new(|body| {
                body["uploads"][2]["reference"] =
                    json!("3d-models/g-Resistor_SMD.3dshapes/Other.step");
            }),
        ),
        // A file named like the Catalog's own footprint, with other
        // content, would change it for every part that uses it.
        (
            "already has the footprint g-Resistor_SMD:R_0603_1608Metric",
            Box::new(|body| {
                let text = body["uploads"][1]["text"].as_str().unwrap().replace(
                    "(footprint \"R_0603_New\"",
                    "(footprint \"R_0603_1608Metric\"",
                );
                body["uploads"][1]["text"] = json!(text);
                body["uploads"][1]["reference"] = json!("g-Resistor_SMD:R_0603_1608Metric");
                body["cadFields"][1]["value"] = json!("g-Resistor_SMD:R_0603_1608Metric");
            }),
        ),
        // A symbol the Catalog does not have and no file was sent for.
        (
            "is not in the Catalog; choose a file",
            Box::new(|body| {
                body["cadFields"][0]["value"] = json!("g-res:R_Missing");
            }),
        ),
    ];
    for (says, change) in &cases {
        // PocketBase starts the message with a capital letter.
        let message = send(change.as_ref()).to_lowercase();
        assert!(message.contains(&says.to_lowercase()), "{says}: {message}");
    }
    assert_eq!(team.read(), before, "nothing was written");
}

#[test]
fn a_correlation_id_is_never_reused_for_another_new_part() {
    let team = Team::start();
    let editor = team.member("Ada Editor", true);
    let command = new_part(IPN);
    applied(&submit_create(&editor, &command).unwrap());
    let saved = team.read();
    let mut other = command.clone();
    other.ipn = "RES-0003-0001".into();
    let reused = submit_create(&editor, &other).unwrap();
    assert_eq!(reused.status, 409, "{}", reused.body);
    assert!(
        matches!(reused.outcome, CreateOutcome::CorrelationReused { .. }),
        "{}",
        reused.body
    );
    assert_eq!(team.read().counts(), saved.counts());
}

#[test]
fn signing_in_is_needed() {
    let team = Team::start();
    let anonymous = PocketBaseClient::new(team.server.url());
    match submit_create(&anonymous, &new_part(IPN)) {
        Err(CollabError::Unauthorized(_)) => {}
        other => panic!("expected 401, got {other:?}"),
    }
}

#[test]
fn two_saves_of_one_ipn_at_once_add_one_part_and_the_other_can_retry() {
    let team = Team::start();
    let ada = team.member("Ada Editor", true);
    let bob = team.member("Bob Editor", true);
    let replies: Vec<CreateReply> = std::thread::scope(|scope| {
        let saves: Vec<_> = [&ada, &bob]
            .into_iter()
            .map(|client| scope.spawn(move || submit_create(client, &new_part(IPN)).unwrap()))
            .collect();
        saves.into_iter().map(|save| save.join().unwrap()).collect()
    });
    let applied_count = replies
        .iter()
        .filter(|reply| matches!(reply.outcome, CreateOutcome::Applied { .. }))
        .count();
    let taken: Vec<&CreateReply> = replies
        .iter()
        .filter(|reply| matches!(reply.outcome, CreateOutcome::IpnTaken { .. }))
        .collect();
    assert_eq!((applied_count, taken.len()), (1, 1), "{replies:?}");
    // The one who lost did not add it, so the app offers the next IPN.
    assert!(matches!(
        taken[0].outcome,
        CreateOutcome::IpnTaken {
            added_by_you: false,
            ..
        }
    ));
    // Retrying with the next free IPN works, using the files the first
    // save stored.
    let mut retry = new_part("RES-0002-0001");
    retry.uploads = Vec::new();
    applied(&submit_create(&ada, &retry).unwrap());
    assert!(team.read().part("RES-0002-0001").is_some());
}

#[test]
fn a_changed_resave_after_a_lost_answer_says_you_added_it() {
    let team = Team::start();
    let ada = team.member("Ada Editor", true);
    let bob = team.member("Bob Editor", true);
    applied(&submit_create(&ada, &new_part(IPN)).unwrap());
    // The answer was lost; Ada changes the reason and saves again.
    let mut again = new_part(IPN);
    again.reason = "A new 0603 resistor, 1k".into();
    let reply = submit_create(&ada, &again).unwrap();
    assert!(
        matches!(
            reply.outcome,
            CreateOutcome::IpnTaken {
                added_by_you: true,
                ..
            }
        ),
        "{}",
        reply.body
    );
    // Someone else taking the same IPN is told it is taken, not theirs.
    let reply = submit_create(&bob, &new_part(IPN)).unwrap();
    assert!(
        matches!(
            reply.outcome,
            CreateOutcome::IpnTaken {
                added_by_you: false,
                ..
            }
        ),
        "{}",
        reply.body
    );
    // An imported part is never "yours".
    let reply = submit_create(&ada, &new_part("RES-0000-0470")).unwrap();
    assert!(
        matches!(
            reply.outcome,
            CreateOutcome::IpnTaken {
                added_by_you: false,
                ..
            }
        ),
        "{}",
        reply.body
    );
}

/// The team server applies the library check's rule for 3D model links to
/// a new part's footprint file too (kicad-parts-da65): a link outside the
/// Catalog's 3D model folder, or to a Catalog 3D model neither stored nor
/// sent, is refused plainly and nothing is written.
#[test]
fn a_new_part_whose_footprint_links_a_3d_model_outside_the_library_is_refused() {
    let team = Team::start();
    let editor = team.member("Ada Editor", true);
    let before = team.read();
    let link = format!("${{GITPLM_PARTS}}/{MODEL}");
    let with_footprint = |text: String, model: bool| {
        let mut command = new_part(IPN);
        command.uploads = uploads();
        command.uploads[1] =
            footprint_upload(text.as_bytes(), FOOTPRINT, "R_0603_New.kicad_mod").unwrap();
        if !model {
            command.uploads.truncate(2);
        }
        command
    };
    for (text, model, says) in [
        (
            footprint_file().replace(&link, "${KIPRJMOD}/3D/R_0603_New.stp"),
            false,
            "he footprint's 3D model link ${KIPRJMOD}/3D/R_0603_New.stp is outside the Catalog's library, so it only works on one computer",
        ),
        (
            footprint_file(),
            false,
            "he footprint links the 3D model 3d-models/g-Resistor_SMD.3dshapes/R_0603_New.step, which the Catalog does not have",
        ),
    ] {
        match submit_create(&editor, &with_footprint(text, model)) {
            Err(CollabError::Http {
                status: 400,
                message,
            }) => assert!(message.contains(says), "{says}: {message}"),
            other => panic!("expected 400 saying {says:?}, got {other:?}"),
        }
    }
    assert!(team.read() == before, "nothing was written");
    // KiCad's own model is fine.
    let kicad = footprint_file().replace(
        &link,
        "${KICAD9_3DMODEL_DIR}/Resistor_SMD.3dshapes/R_0603_1608Metric.step",
    );
    applied(&submit_create(&editor, &with_footprint(kicad, false)).unwrap());
}
