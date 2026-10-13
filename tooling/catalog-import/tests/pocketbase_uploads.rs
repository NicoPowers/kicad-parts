//! Edits that use files (seed kicad-parts-3b8b): a symbol, footprint or 3D
//! model sent with an edit becomes a new CAD item, or a new revision of one,
//! saved in the import's shape, numbering and IDs; a file with the content
//! of the CAD item's current revision adds nothing; other parts that use the
//! CAD item keep their exact binding. The server reads every file itself and
//! refuses, plainly, one that is wrong or not used. CSV imports afterwards
//! never replace a CAD item revision made in the app.
//!
//! Each test starts its own disposable PocketBase on 127.0.0.1 and imports a
//! temp copy of the resistor CSV file; the repository is only read.

use catalog::cad_file::{footprint_upload, model_upload, symbol_upload};
use catalog::pocketbase::{
    CatalogReadback, EDIT_ROUTE, EditCommand, ImportCommand, ImportOutcome, PublishOutcome,
    PublishReply, read_back, read_import_state, submit_edit, submit_import,
};
use catalog::{CadItemDetails, CadReferenceTarget, CsvCatalog, ModelTarget, PartRevision};
use catalog_import::services;
use collaboration::{CollabError, PocketBaseClient};
use native_services::PocketBaseServer;
use serde_json::{Value, json};
use std::fs;

const IPN: &str = "RES-0000-0470";
const OTHER: &str = "RES-0000-1000";
const FOOTPRINT: &str = "g-Resistor_SMD:R_0603_1608Metric";
const FOOTPRINT_FILE: &str = "footprints/g-Resistor_SMD.pretty/R_0603_1608Metric.kicad_mod";

/// Copies the CSV catalog trees the import and the zero-loss oracle read
/// into `target`.
fn copy_catalog(target: &std::path::Path) {
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
    for tree in ["database", "symbols", "footprints", "3d-models"] {
        copy_tree(&services::repository_root().join(tree), &target.join(tree));
    }
}

struct Team {
    server: PocketBaseServer,
    admin: PocketBaseClient,
    /// A copy of the CSV catalog, which the tests change.
    copy: tempfile::TempDir,
    _state: tempfile::TempDir,
}

impl Team {
    fn start() -> Self {
        let root = services::repository_root();
        let state = tempfile::tempdir().unwrap();
        let (server, admin, _superuser) = services::start(&root, state.path()).unwrap();
        let copy = tempfile::tempdir().unwrap();
        copy_catalog(copy.path());
        let team = Self {
            server,
            admin,
            copy,
            _state: state,
        };
        assert_eq!(team.import(), "applied");
        team
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

    /// Imports the copy as an importer does (reading the team catalog
    /// first) and returns the outcome; the zero-loss report must hold.
    fn import(&self) -> &'static str {
        let import = CsvCatalog::new(self.copy.path())
            .import("g-res", None)
            .unwrap();
        let state = read_import_state(&self.admin).unwrap();
        let command = ImportCommand::new(&import, "Import the resistors").expecting(&state);
        let reply = submit_import(&self.admin, &command).unwrap();
        let outcome = match reply.outcome {
            ImportOutcome::Applied { .. } => "applied",
            ImportOutcome::NothingChanged { .. } => "nothing_changed",
            ImportOutcome::Noop { .. } => "noop",
            _ => panic!("the import did not go through: {}", reply.body),
        };
        let oracle = csv_catalog_fixture::build_manifest(self.copy.path()).unwrap();
        let report = catalog_import::report(&oracle, "g-res", &self.read(), self.copy.path());
        assert!(
            report.is_lossless(),
            "{}",
            serde_json::to_string_pretty(&report.summary()).unwrap()
        );
        outcome
    }

    fn file(&self, path: &str) -> Vec<u8> {
        fs::read(self.copy.path().join(path)).unwrap()
    }
}

fn current_revision(stored: &CatalogReadback, ipn: &str) -> (u32, PartRevision) {
    let part = stored.part(ipn).unwrap();
    let revision = stored.part_revision(&part.current_revision).unwrap();
    (
        revision.number,
        serde_json::from_str(&revision.snapshot).unwrap(),
    )
}

/// A CAD edit of `ipn` as it is now, setting `values` and sending `uploads`.
fn cad_edit(
    stored: &CatalogReadback,
    ipn: &str,
    values: &[(&str, &str)],
    uploads: Vec<catalog::pocketbase::CadUpload>,
) -> EditCommand {
    let (number, revision) = current_revision(stored, ipn);
    let mut command =
        EditCommand::new(ipn, "Use the corrected file").editing_cad(&revision, number);
    for (field, value) in values {
        assert!(command.set_field(field, value), "{field}");
    }
    command.cad.as_mut().unwrap().uploads = uploads;
    command
}

/// A new CAD item revision an edit reported: kind, key, number, and how
/// many other parts use an older revision.
type NewRevision = (String, String, u32, Option<u32>);

/// The revision number an applied edit added and the new CAD item revisions
/// it reported.
fn applied(reply: &PublishReply) -> (Option<u32>, Vec<NewRevision>) {
    match &reply.outcome {
        PublishOutcome::Applied {
            revision,
            new_cad_item_revisions,
            ..
        } => (
            revision.as_ref().map(|revision| revision.number),
            new_cad_item_revisions
                .iter()
                .map(|new| {
                    (
                        new.kind.clone(),
                        new.key.clone(),
                        new.number,
                        new.other_parts,
                    )
                })
                .collect(),
        ),
        other => panic!("expected applied, got {other:?}: {}", reply.body),
    }
}

/// The CAD item revision a part's current revision binds for `reference`.
fn bound(stored: &CatalogReadback, ipn: &str, reference: &str) -> CadReferenceTarget {
    let (_, revision) = current_revision(stored, ipn);
    revision
        .cad_references
        .iter()
        .find(|item| item.reference == reference)
        .map(|item| item.target.clone())
        .unwrap_or_else(|| panic!("{ipn} does not use {reference}"))
}

fn number_of(target: &CadReferenceTarget) -> u32 {
    match target {
        CadReferenceTarget::CadItemRevision { number, .. } => *number,
        other => panic!("not bound to a CAD item revision: {other:?}"),
    }
}

/// The parts whose current revision uses `reference`.
fn users(stored: &CatalogReadback, reference: &str) -> Vec<String> {
    stored
        .parts
        .iter()
        .filter(|part| {
            current_revision(stored, &part.ipn)
                .1
                .cad_references
                .iter()
                .any(|item| item.reference == reference)
        })
        .map(|part| part.ipn.clone())
        .collect()
}

/// The footprint file with a circle added to its silkscreen.
fn redrawn(footprint: &[u8]) -> Vec<u8> {
    let text = String::from_utf8(footprint.to_vec()).unwrap();
    let changed = text.replacen(
        "\t(fp_line",
        "\t(fp_circle (center 0 0) (end 2 0) (stroke (width 0.3) (type solid)) (fill no) (layer \"F.SilkS\"))\n\t(fp_line",
        1,
    );
    assert_ne!(changed, text);
    changed.into_bytes()
}

fn refused(result: Result<PublishReply, CollabError>, status: u16, says: &str) {
    match result {
        Err(CollabError::Http {
            status: got,
            message,
        }) if got == status => assert!(message.contains(says), "{says}: {message}"),
        Err(CollabError::Forbidden(message)) if status == 403 => {
            assert!(message.contains(says), "{says}: {message}")
        }
        Err(CollabError::Unauthorized(message)) if status == 401 => {
            assert!(message.contains(says), "{says}: {message}")
        }
        other => panic!("expected {status} saying {says:?}, got {other:?}"),
    }
}

#[test]
fn a_footprint_file_adds_a_revision_and_other_parts_keep_theirs() {
    let team = Team::start();
    let editor = team.member("Ada Editor", true);
    let before = team.read();
    let original = team.file(FOOTPRINT_FILE);
    let users_before = users(&before, FOOTPRINT);
    assert!(users_before.len() > 2, "{users_before:?}");

    // The same file the import read has the content of footprint revision 1,
    // so it adds nothing: the server builds exactly the import's snapshot.
    let same = footprint_upload(&original, FOOTPRINT, "R_0603_1608Metric.kicad_mod").unwrap();
    let reply = submit_edit(&editor, &cad_edit(&before, IPN, &[], vec![same])).unwrap();
    assert_eq!(reply.status, 422, "{}", reply.body);
    assert_eq!(team.read().counts(), before.counts());

    // A corrected file is footprint revision 2; only this part moves to it.
    let corrected = redrawn(&original);
    let upload = footprint_upload(&corrected, FOOTPRINT, "R_0603 corrected.kicad_mod").unwrap();
    let reply = submit_edit(&editor, &cad_edit(&before, IPN, &[], vec![upload.clone()])).unwrap();
    let (revision, new) = applied(&reply);
    assert_eq!(revision, Some(2));
    assert_eq!(
        new,
        [(
            "footprint".to_owned(),
            FOOTPRINT.to_owned(),
            2,
            Some(users_before.len() as u32 - 1)
        )]
    );
    let after = team.read();
    assert_eq!(number_of(&bound(&after, IPN, FOOTPRINT)), 2);
    for ipn in users_before.iter().filter(|ipn| *ipn != IPN) {
        assert_eq!(number_of(&bound(&after, ipn, FOOTPRINT)), 1, "{ipn}");
        assert_eq!(current_revision(&after, ipn).0, 1, "{ipn}");
    }
    // Stored like an import's: numbered, hashed, licence to review, the
    // exact bytes, and the file's name as its source.
    let cad_item = catalog::CadItemId::new(catalog::CadItemKind::Footprint, FOOTPRINT);
    let stored = after
        .cad_item_revision(catalog::CadItemRevisionId::new(&cad_item, 2).as_str())
        .unwrap();
    assert_eq!(stored.license, "unknown-needs-review");
    assert_eq!(
        catalog::sha256_hex(stored.snapshot.as_bytes()),
        stored.snapshot_sha256
    );
    let source: Value = serde_json::from_str(&stored.source).unwrap();
    assert_eq!(source["source"], "edit");
    assert_eq!(source["file_name"], "R_0603 corrected.kicad_mod");
    let snapshot: CadItemDetails = serde_json::from_str(&stored.snapshot).unwrap();
    assert_eq!(snapshot.blob_sha256, catalog::sha256_hex(&corrected));
    assert_eq!(snapshot.format_version.as_deref(), Some("20241229"));
    // KiCad's own 3D model stays external, as in the import.
    assert!(matches!(
        &snapshot.model_references[0].target,
        ModelTarget::Unresolved { status } if status == "unresolved"
    ));
    assert_eq!(after.blob(&stored.blob).unwrap().bytes, corrected);
    // The Change holds the CAD item revision and the part revision.
    let change = after
        .changes
        .iter()
        .find(|change| change.id == stored.change)
        .unwrap();
    assert_eq!(change.kind, "edit");

    // The same corrected file for another part reuses revision 2.
    let reply = submit_edit(&editor, &cad_edit(&after, OTHER, &[], vec![upload])).unwrap();
    let (revision, new) = applied(&reply);
    assert_eq!(revision, Some(2));
    assert!(new.is_empty(), "{new:?}");
    let last = team.read();
    assert_eq!(number_of(&bound(&last, OTHER, FOOTPRINT)), 2);
    assert_eq!(
        last.cad_item_revisions.len(),
        after.cad_item_revisions.len()
    );
    assert_eq!(last.blobs.len(), after.blobs.len());

    // Going back to the first file is new content for the CAD item: like the
    // import, it is revision 3, sharing revision 1's stored bytes.
    let back = footprint_upload(&original, FOOTPRINT, "R_0603_1608Metric.kicad_mod").unwrap();
    let reply = submit_edit(&editor, &cad_edit(&last, IPN, &[], vec![back])).unwrap();
    let (_, new) = applied(&reply);
    assert_eq!(new.len(), 1);
    assert_eq!(new[0].2, 3);
    assert_eq!(team.read().blobs.len(), last.blobs.len());
}

#[test]
fn a_new_symbol_from_a_file_with_two_symbols() {
    let team = Team::start();
    let editor = team.member("Ada Editor", true);
    let before = team.read();
    let library = "(kicad_symbol_lib\n\t(version 20241209)\n\t(generator \"kicad_symbol_editor\")\n\t(symbol \"R_Alt\"\n\t\t(in_bom yes)\n\t\t(property \"Reference\" \"R\" (at 0 0 0) (effects (font (size 1.27 1.27))))\n\t)\n\t(symbol \"R_TestPoint\"\n\t\t(in_bom no)\n\t\t(property \"Reference\" \"TP\" (at 0 0 0) (effects (font (size 1.27 1.27))))\n\t\t(symbol \"R_TestPoint_0_1\")\n\t)\n)\n";
    let upload = symbol_upload(
        library.as_bytes(),
        "R_TestPoint",
        "g-res:R_TestPoint",
        "two symbols.kicad_sym",
    )
    .unwrap();
    let reply = submit_edit(
        &editor,
        &cad_edit(
            &before,
            IPN,
            &[("Symbol", "g-res:R_TestPoint")],
            vec![upload],
        ),
    )
    .unwrap();
    let (revision, new) = applied(&reply);
    assert_eq!(revision, Some(2));
    assert_eq!(
        new,
        [(
            "symbol".to_owned(),
            "g-res:R_TestPoint".to_owned(),
            1,
            Some(0)
        )]
    );
    let after = team.read();
    let (_, part) = current_revision(&after, IPN);
    assert_eq!(number_of(&bound(&after, IPN, "g-res:R_TestPoint")), 1);
    // Excluded from BOM comes from the new symbol's own (in_bom no).
    assert!(part.excluded_from_bom);
    let cad_item = catalog::CadItemId::new(catalog::CadItemKind::Symbol, "g-res:R_TestPoint");
    let stored = after
        .cad_item_revision(catalog::CadItemRevisionId::new(&cad_item, 1).as_str())
        .unwrap();
    let snapshot: CadItemDetails = serde_json::from_str(&stored.snapshot).unwrap();
    assert_eq!(snapshot.format, "kicad-symbol-entry");
    assert_eq!(snapshot.format_version.as_deref(), Some("20241209"));
    let bytes = &after.blob(&stored.blob).unwrap().bytes;
    assert!(bytes.starts_with(b"(symbol \"R_TestPoint\""));
    assert!(bytes.ends_with(b")"));
    assert!(after.cad_item(cad_item.as_str()).is_some());

    // The symbol the import read, sent again, has its current content.
    let symbols = team.file("symbols/g-res.kicad_sym");
    let same = symbol_upload(&symbols, "R_US", "g-res:R_US", "g-res.kicad_sym").unwrap();
    let reply = submit_edit(&editor, &cad_edit(&after, OTHER, &[], vec![same])).unwrap();
    assert_eq!(reply.status, 422, "{}", reply.body);
}

#[test]
fn a_footprint_names_its_3d_model_which_is_sent_or_the_edit_is_refused() {
    let team = Team::start();
    let editor = team.member("Ada Editor", true);
    let before = team.read();
    let footprint = String::from_utf8(team.file(FOOTPRINT_FILE))
        .unwrap()
        .replacen(
            "(footprint \"R_0603_1608Metric\"",
            "(footprint \"R_New\"",
            1,
        )
        .replace(
            "${KICAD9_3DMODEL_DIR}/Resistor_SMD.3dshapes/R_0603_1608Metric.step",
            "${GITPLM_PARTS}/3d-models/g-Resistor_SMD.3dshapes/R_New.step",
        );
    let key = "3d-models/g-Resistor_SMD.3dshapes/R_New.step";
    let reference = "g-Resistor_SMD:R_New";
    let upload = footprint_upload(footprint.as_bytes(), reference, "R_New.kicad_mod").unwrap();

    // Without the 3D model, which the Catalog lacks, the edit is refused
    // (the library check's rule, kicad-parts-da65) and nothing is written.
    refused(
        submit_edit(
            &editor,
            &cad_edit(
                &before,
                IPN,
                &[("Footprint", reference)],
                vec![upload.clone()],
            ),
        ),
        400,
        "he footprint links the 3D model 3d-models/g-Resistor_SMD.3dshapes/R_New.step, which the Catalog does not have. Choose that 3D model file too, or fix the link in KiCad's Footprint Editor",
    );
    assert!(team.read() == before, "nothing was written");

    // With it (any bytes, here not text), the 3D model is a new CAD item and
    // the footprint's first revision binds it.
    let model = vec![0x00, 0xff, 0x10, 0x80];
    let reply = submit_edit(
        &editor,
        &cad_edit(
            &before,
            OTHER,
            &[("Footprint", reference)],
            vec![upload, model_upload(&model, key, "R_New.stp").unwrap()],
        ),
    )
    .unwrap();
    let (_, new) = applied(&reply);
    assert_eq!(
        new,
        [
            ("model-3d".to_owned(), key.to_owned(), 1, None),
            ("footprint".to_owned(), reference.to_owned(), 1, Some(0)),
        ]
    );
    let last = team.read();
    let footprint_item = catalog::CadItemId::new(catalog::CadItemKind::Footprint, reference);
    let first = last
        .cad_item_revision(catalog::CadItemRevisionId::new(&footprint_item, 1).as_str())
        .unwrap();
    let snapshot: CadItemDetails = serde_json::from_str(&first.snapshot).unwrap();
    let model_item = catalog::CadItemId::new(catalog::CadItemKind::Model3d, key);
    assert!(matches!(
        &snapshot.model_references[0].target,
        ModelTarget::CadItemRevision { cad_item, number: 1, .. } if cad_item == model_item.as_str()
    ));
    let stored_model = last
        .cad_item_revision(catalog::CadItemRevisionId::new(&model_item, 1).as_str())
        .unwrap();
    assert_eq!(last.blob(&stored_model.blob).unwrap().bytes, model);
}

/// The team server applies the library check's rule for 3D model links to
/// every footprint file an edit sends, so an older app or a direct request
/// cannot store a link that only works on one computer: KiCad's own and
/// embedded models are fine; any place outside the Catalog's 3D model
/// folder, or a Catalog 3D model path outside a footprint library's folder,
/// is refused plainly, and nothing is written.
#[test]
fn a_footprint_file_linking_a_3d_model_outside_the_library_is_refused() {
    let team = Team::start();
    let editor = team.member("Ada Editor", true);
    let before = team.read();
    let original = String::from_utf8(team.file(FOOTPRINT_FILE)).unwrap();
    let kicad = "${KICAD9_3DMODEL_DIR}/Resistor_SMD.3dshapes/R_0603_1608Metric.step";
    let linking = |link: &str| {
        let text = redrawn(original.replace(kicad, link).as_bytes());
        footprint_upload(&text, FOOTPRINT, "R.kicad_mod").unwrap()
    };
    let own = "${GITPLM_PARTS}/3d-models/g-Resistor_SMD.3dshapes/R_0603_1608Metric.step";
    for (link, says) in [
        (
            "${KIPRJMOD}/3D/R_0603.stp".to_owned(),
            format!("he footprint's 3D model link ${{KIPRJMOD}}/3D/R_0603.stp is outside the Catalog's library, so it only works on one computer. Link the model as {own} in KiCad's Footprint Editor, or remove the link, then choose the file again"),
        ),
        (
            "/home/me/models/R_0603.step".to_owned(),
            "he footprint's 3D model link /home/me/models/R_0603.step is outside the Catalog's library".to_owned(),
        ),
        (
            "${GITPLM_3DMODELS}/R_0603.step".to_owned(),
            "he footprint's 3D model link ${GITPLM_3DMODELS}/R_0603.step is outside the Catalog's library".to_owned(),
        ),
        (
            "${KICAD9_3DMODEL_DIR}/../../me/R_0603.step".to_owned(),
            "he footprint's 3D model link ${KICAD9_3DMODEL_DIR}/../../me/R_0603.step is outside the Catalog's library".to_owned(),
        ),
        (
            "${GITPLM_PARTS}/3d-models/R_0603.step".to_owned(),
            format!("he footprint links the 3D model 3d-models/R_0603.step, a place where the Catalog keeps no 3D models. Link the model as {own} in KiCad's Footprint Editor, then choose the file again"),
        ),
    ] {
        refused(
            submit_edit(&editor, &cad_edit(&before, IPN, &[], vec![linking(&link)])),
            400,
            &says,
        );
    }
    // A KiCad 5 path written without quotes is judged the same way.
    let unquoted = redrawn(
        original
            .replace(&format!("\"{kicad}\""), "${KIPRJMOD}/3D/R_0603.wrl")
            .as_bytes(),
    );
    assert!(String::from_utf8_lossy(&unquoted).contains("(model ${KIPRJMOD}/3D/R_0603.wrl"));
    refused(
        submit_edit(
            &editor,
            &cad_edit(
                &before,
                IPN,
                &[],
                vec![footprint_upload(&unquoted, FOOTPRINT, "R.kicad_mod").unwrap()],
            ),
        ),
        400,
        "he footprint's 3D model link ${KIPRJMOD}/3D/R_0603.wrl is outside the Catalog's library",
    );
    assert!(team.read() == before, "nothing was written");
    // KiCad's own and embedded models are fine.
    let reply = submit_edit(
        &editor,
        &cad_edit(
            &before,
            IPN,
            &[],
            vec![linking("kicad-embed://R_0603.step")],
        ),
    )
    .unwrap();
    applied(&reply);
    let after = team.read();
    let reply = submit_edit(
        &editor,
        &cad_edit(
            &after,
            IPN,
            &[],
            vec![linking(
                "${KICAD10_3DMODEL_DIR}/Resistor_SMD.3dshapes/R_0603_1608Metric.step",
            )],
        ),
    )
    .unwrap();
    applied(&reply);
}

#[test]
fn wrong_or_unused_files_are_refused_plainly_and_nothing_is_written() {
    let team = Team::start();
    let editor = team.member("Ada Editor", true);
    let viewer = team.member("Vic Viewer", false);
    let before = team.read();
    let footprint = team.file(FOOTPRINT_FILE);
    let corrected = redrawn(&footprint);
    let (number, revision) = current_revision(&before, IPN);
    let fields: Vec<Value> = revision
        .cad_fields
        .iter()
        .map(|field| json!({ "name": field.name, "value": field.value }))
        .collect();
    let send = |client: &PocketBaseClient, uploads: Value, fields: &[Value]| {
        let body = json!({
            "correlationId": format!("uploads-{}", catalog::sha256_hex(uploads.to_string())),
            "reason": "Use a file",
            "ipn": IPN,
            "cad": { "expectedRevision": number, "fields": fields, "uploads": uploads },
        });
        client
            .post_json(EDIT_ROUTE, &body)
            .and_then(|reply| -> Result<(), CollabError> {
                reply.success()?;
                panic!("expected a refusal, got {}", reply.body)
            })
    };
    let text = |bytes: &[u8]| String::from_utf8(bytes.to_vec()).unwrap();
    let with_symbol = |value: &str| -> Vec<Value> {
        revision
            .cad_fields
            .iter()
            .map(|field| {
                let value = if field.name == "Symbol" {
                    value
                } else {
                    &field.value
                };
                json!({ "name": field.name, "value": value })
            })
            .collect()
    };
    let symbol = |bytes: &str| json!({ "role": "symbol", "reference": "g-res:R_X", "fileName": "x.kicad_sym", "formatVersion": "20241209", "text": bytes });
    let cases = [
        // The name in the file is not the reference's.
        (
            json!([{ "role": "footprint", "reference": "g-Resistor_SMD:R_0805_2012Metric", "fileName": "f.kicad_mod", "text": text(&corrected) }]),
            fields.clone(),
            "not named R_0805_2012Metric",
        ),
        // Two files for one reference.
        (
            json!([
                { "role": "footprint", "reference": FOOTPRINT, "fileName": "a.kicad_mod", "text": text(&corrected) },
                { "role": "footprint", "reference": FOOTPRINT, "fileName": "b.kicad_mod", "text": text(&footprint) },
            ]),
            fields.clone(),
            "files were sent for the footprint",
        ),
        // Two symbols in one file.
        (
            json!([symbol(
                "(symbol \"R_X\" (in_bom yes))(symbol \"R_Y\" (in_bom yes))"
            )]),
            with_symbol("g-res:R_X"),
            "exactly one symbol",
        ),
        // Not an s-expression.
        (
            json!([symbol("(symbol \"R_X\" (in_bom yes)")]),
            with_symbol("g-res:R_X"),
            "exactly one symbol",
        ),
        (
            json!([{ "role": "footprint", "reference": FOOTPRINT, "fileName": "f.kicad_mod", "text": "(footprint \"R_0603_1608Metric\" (layer" }]),
            fields.clone(),
            "not a KiCad footprint",
        ),
        // A symbol that is drawn from another.
        (
            json!([symbol("(symbol \"R_X\" (extends \"R_US\"))")]),
            with_symbol("g-res:R_X"),
            "extends another symbol",
        ),
        // An unknown role.
        (
            json!([{ "role": "drawing", "reference": FOOTPRINT, "fileName": "f.pdf", "text": "x" }]),
            fields.clone(),
            "the role must be symbol, footprint or 3d-model",
        ),
        // A file nobody uses: no field names it.
        (
            json!([{ "role": "footprint", "reference": "g-Resistor_SMD:R_0603_1608Metric_Pad", "fileName": "f.kicad_mod", "text": text(&corrected).replacen("R_0603_1608Metric", "R_0603_1608Metric_Pad", 1) }]),
            fields.clone(),
            "is not used",
        ),
        (
            json!([{ "role": "3d-model", "reference": "3d-models/g-Resistor_SMD.3dshapes/X.step", "fileName": "x.step", "base64": "AAEC" }]),
            fields.clone(),
            "footprint uploaded with the 3D model",
        ),
        // Only the Catalog's own libraries take files.
        (
            json!([{ "role": "symbol", "reference": "Device:R_X", "fileName": "x.kicad_sym", "formatVersion": "20241209", "text": "(symbol \"R_X\")" }]),
            with_symbol("Device:R_X"),
            "start with g-",
        ),
        // A symbol needs the version of the file it came from, and must be
        // text; a file cannot be empty.
        (
            json!([{ "role": "symbol", "reference": "g-res:R_X", "fileName": "x.kicad_sym", "text": "(symbol \"R_X\")" }]),
            with_symbol("g-res:R_X"),
            "needs the (version ...)",
        ),
        (
            json!([{ "role": "footprint", "reference": FOOTPRINT, "fileName": "f.kicad_mod", "base64": "AAEC" }]),
            fields.clone(),
            "must be text",
        ),
        (
            json!([{ "role": "3d-model", "reference": "3d-models/x.step", "fileName": "x.step", "text": "" }]),
            fields.clone(),
            "is empty",
        ),
    ];
    // References and 3D model paths follow the rule KiCad Integration's
    // LibRef::parse applies, as both halves become file names.
    let mut cases = cases.to_vec();
    cases.push((
        json!([{ "role": "symbol", "reference": "g-:R_X", "fileName": "x.kicad_sym", "formatVersion": "20241209", "text": "(symbol \"x\")" }]),
        with_symbol("g-:R_X"),
        "start with g- followed by a name",
    ));
    for reference in [
        "g-res:-R",
        "g-res:R\u{1}",
        "g-res:R\u{0}",
        "g-res:..",
        "g-res:a:b",
        "..:R",
        "g-res\\x:R",
        "g-res/x:R",
    ] {
        cases.push((
            json!([{ "role": "symbol", "reference": reference, "fileName": "x.kicad_sym", "formatVersion": "20241209", "text": "(symbol \"x\")" }]),
            with_symbol(reference),
            "cannot be used as a reference",
        ));
    }
    // The version a symbol came from is a KiCad file version: 20 and six
    // more digits.
    for version in ["1", "123456789", "19991231", "2024120"] {
        cases.push((
            json!([{ "role": "symbol", "reference": "g-res:R_X", "fileName": "x.kicad_sym", "formatVersion": version, "text": "(symbol \"R_X\")" }]),
            with_symbol("g-res:R_X"),
            "needs the (version ...)",
        ));
    }
    for key in [
        "3d-models/../x.step",
        "3d-models/x.step",
        "3d-models/g-a.3dshapes/../x.step",
        "3d-models/g-a.3dshapes/sub/x.step",
        "3d-models/g-.3dshapes/x.step",
        "3d-models/g-a.3dshapes/-x.step",
    ] {
        cases.push((
            json!([{ "role": "3d-model", "reference": key, "fileName": "x.step", "base64": "AAEC" }]),
            fields.clone(),
            "is not a Catalog 3D model path",
        ));
    }
    for (uploads, fields, says) in cases {
        match send(&editor, uploads, &fields) {
            Err(CollabError::Http {
                status: 400,
                message,
            }) => {
                assert!(message.contains(says), "{says}: {message}")
            }
            other => panic!("expected 400 saying {says:?}, got {other:?}"),
        }
    }
    // Uploading needs the right to edit.
    let upload = footprint_upload(&corrected, FOOTPRINT, "f.kicad_mod").unwrap();
    refused(
        submit_edit(&viewer, &cad_edit(&before, IPN, &[], vec![upload.clone()])),
        403,
        "edit access",
    );
    refused(
        submit_edit(
            &PocketBaseClient::new(team.server.url()),
            &cad_edit(&before, IPN, &[], vec![upload]),
        ),
        401,
        "",
    );
    assert_eq!(team.read().counts(), before.counts());

    // An edit over the size limit is refused before anything is sent.
    let big = catalog::pocketbase::CadUpload {
        role: catalog::pocketbase::UploadRole::Model3d,
        reference: "3d-models/big.step".into(),
        file_name: "big.step".into(),
        format_version: None,
        bytes: vec![b'x'; catalog::pocketbase::EDIT_BODY_LIMIT],
        library: None,
    };
    let error = submit_edit(&editor, &cad_edit(&before, IPN, &[], vec![big])).unwrap_err();
    assert!(error.to_string().contains("nothing was sent"), "{error}");
}

/// The import problems with `code` the latest import run found, by subject.
fn found(stored: &CatalogReadback, code: &str) -> Vec<catalog::pocketbase::StoredImportProblem> {
    let run = stored
        .audit_entries
        .iter()
        .filter(|entry| entry.csv_import_number > 0)
        .max_by_key(|entry| entry.csv_import_number)
        .and_then(|entry| entry.import_run())
        .unwrap();
    stored
        .problems
        .iter()
        .filter(|problem| problem.code == code && run.problems.contains(&problem.id))
        .cloned()
        .collect()
}

/// The newest revision number of `reference`'s CAD item.
fn newest(stored: &CatalogReadback, kind: catalog::CadItemKind, reference: &str) -> u32 {
    let cad_item = catalog::CadItemId::new(kind, reference);
    stored
        .cad_item_revisions
        .iter()
        .filter(|revision| revision.cad_item == cad_item.as_str())
        .map(|revision| revision.number)
        .max()
        .unwrap_or(0)
}

#[test]
fn imports_never_replace_a_cad_item_revision_made_in_the_app() {
    use catalog::pocketbase::{
        CSV_CAD_DIFFERS, CSV_CAD_ITEM_KEPT, CsvReviewCommand, CsvReviewOutcome,
        recorded_csv_document, submit_csv_review,
    };
    let team = Team::start();
    let editor = team.member("Ada Editor", true);
    let before = team.read();
    let footprint_users = users(&before, FOOTPRINT);
    let original = team.file(FOOTPRINT_FILE);
    let upload = footprint_upload(&redrawn(&original), FOOTPRINT, "R_0603.kicad_mod").unwrap();
    applied(&submit_edit(&editor, &cad_edit(&before, IPN, &[], vec![upload])).unwrap());
    let edited = team.read();
    let footprint = catalog::CadItemKind::Footprint;
    assert_eq!(newest(&edited, footprint, FOOTPRINT), 2);

    // The CSV's file has not changed since the import last applied it, so
    // nothing happens: no new footprint revision, every part as it was.
    assert_eq!(team.import(), "nothing_changed");
    let after = team.read();
    assert_eq!(newest(&after, footprint, FOOTPRINT), 2);
    assert_eq!(
        after.cad_item_revisions.len(),
        edited.cad_item_revisions.len()
    );
    assert_eq!(after.part_revisions.len(), edited.part_revisions.len());
    assert_eq!(number_of(&bound(&after, IPN, FOOTPRINT)), 2);
    for ipn in footprint_users.iter().filter(|ipn| *ipn != IPN) {
        assert_eq!(number_of(&bound(&after, ipn, FOOTPRINT)), 1, "{ipn}");
    }
    assert!(found(&after, CSV_CAD_ITEM_KEPT).is_empty());
    // The edited part's row is kept, as for any edited CAD; what it records
    // is exactly what the last import saved, so the app asks nothing.
    let kept = found(&after, CSV_CAD_DIFFERS);
    assert_eq!(kept.len(), 1);
    assert_eq!(kept[0].subject, IPN);
    let first = after
        .part_revisions
        .iter()
        .find(|revision| revision.part == after.part(IPN).unwrap().id && revision.number == 1)
        .unwrap();
    assert_eq!(recorded_csv_document(&kept[0]).unwrap(), first.snapshot);
    assert_eq!(team.import(), "noop");

    // An importer that does not know the footprint was changed in the app
    // would send a new revision of it: the server refuses.
    let import = CsvCatalog::new(team.copy.path())
        .import("g-res", None)
        .unwrap();
    let mut state = read_import_state(&team.admin).unwrap();
    assert!(!state.current_cad.edited_in_app.is_empty());
    state.current_cad.edited_in_app.clear();
    let command = ImportCommand::new(&import, "Import the resistors").expecting(&state);
    match submit_import(&team.admin, &command) {
        Err(CollabError::Http {
            status: 400,
            message,
        }) => {
            assert!(message.contains("never replaces"), "{message}")
        }
        other => panic!("expected 400, got {other:?}"),
    }
    assert_eq!(team.read().counts(), after.counts());

    // The CSV's file changes: still no new footprint revision, but every
    // part whose row uses it keeps the row with a problem the app shows.
    let changed = String::from_utf8(original).unwrap().replacen(
        "(tags \"resistor\")",
        "(tags \"resistor 0603\")",
        1,
    );
    fs::write(team.copy.path().join(FOOTPRINT_FILE), changed).unwrap();
    assert_eq!(team.import(), "nothing_changed");
    let changed = team.read();
    assert_eq!(newest(&changed, footprint, FOOTPRINT), 2);
    assert_eq!(changed.part_revisions.len(), edited.part_revisions.len());
    let problems = found(&changed, CSV_CAD_ITEM_KEPT);
    let mut subjects: Vec<String> = problems.iter().map(|p| p.subject.clone()).collect();
    subjects.sort();
    let mut expected = footprint_users.clone();
    expected.sort();
    assert_eq!(subjects, expected);
    let problem = problems.iter().find(|p| p.subject == OTHER).unwrap();
    assert_eq!(problem.location, format!("rows[IPN={OTHER}].Footprint"));

    // The CSV's file is not stored, so it can only be kept.
    let (number, _) = current_revision(&changed, OTHER);
    let take = CsvReviewCommand::new(
        OTHER,
        &problem.id,
        vec!["Footprint".into()],
        number,
        "Take it",
    );
    match submit_csv_review(&editor, &take) {
        Err(CollabError::Http {
            status: 400,
            message,
        }) => {
            assert!(
                message.contains("choose the file in Edit part"),
                "{message}"
            )
        }
        other => panic!("expected 400, got {other:?}"),
    }
    let keep = CsvReviewCommand::new(OTHER, &problem.id, vec![], number, "Keep ours");
    let reply = submit_csv_review(&editor, &keep).unwrap();
    assert!(
        matches!(
            reply.outcome,
            CsvReviewOutcome::Applied { revision: None, .. }
        ),
        "{}",
        reply.body
    );
    // Importing the same CSV again finds the same problem, which the review
    // covers.
    assert_eq!(team.import(), "noop");
}

#[test]
fn a_csv_that_caught_up_with_the_app_is_not_a_change() {
    use catalog::pocketbase::CSV_CAD_ITEM_KEPT;
    let team = Team::start();
    let editor = team.member("Ada Editor", true);
    // The CSV catalog's symbol library with a symbol the app adds first.
    let library_path = team.copy.path().join("symbols/g-res.kicad_sym");
    let library = fs::read_to_string(&library_path).unwrap();
    let entry = "\t(symbol \"R_Added\"\n\t\t(in_bom yes)\n\t\t(property \"Reference\" \"R\" (at 0 0 0) (effects (font (size 1.27 1.27))))\n\t)\n";
    let end = library.trim_end().len() - 1;
    let caught_up = format!("{}{entry}{}", &library[..end], &library[end..]);
    let symbol = symbol_upload(
        caught_up.as_bytes(),
        "R_Added",
        "g-res:R_Added",
        "added.kicad_sym",
    )
    .unwrap();
    let before = team.read();
    applied(
        &submit_edit(
            &editor,
            &cad_edit(&before, IPN, &[("Symbol", "g-res:R_Added")], vec![symbol]),
        )
        .unwrap(),
    );
    // And a corrected footprint for the same part.
    let corrected = redrawn(&team.file(FOOTPRINT_FILE));
    let edited = team.read();
    let footprint = footprint_upload(&corrected, FOOTPRINT, "R.kicad_mod").unwrap();
    applied(&submit_edit(&editor, &cad_edit(&edited, IPN, &[], vec![footprint])).unwrap());

    // The CSV catalog catches up with both, byte for byte, and another row
    // uses the new symbol.
    fs::write(&library_path, caught_up).unwrap();
    fs::write(team.copy.path().join(FOOTPRINT_FILE), &corrected).unwrap();
    let csv = team.copy.path().join("database/g-res.csv");
    let text = fs::read_to_string(&csv).unwrap();
    let row = text
        .split_inclusive('\n')
        .find(|line| line.starts_with(&format!("\"{OTHER}\"")))
        .unwrap()
        .to_owned();
    fs::write(
        &csv,
        text.replacen(
            &row,
            &row.replacen("\"g-res:R_US\"", "\"g-res:R_Added\"", 1),
            1,
        ),
    )
    .unwrap();
    let made = team.read();
    team.import();
    let after = team.read();
    // Nothing to show, and no CAD item gets a revision.
    assert!(
        found(&after, CSV_CAD_ITEM_KEPT).is_empty(),
        "{:?}",
        found(&after, CSV_CAD_ITEM_KEPT)
    );
    assert_eq!(
        after.cad_item_revisions.len(),
        made.cad_item_revisions.len()
    );
    // The other row's part follows the CSV to the revisions the app made.
    assert_eq!(number_of(&bound(&after, OTHER, "g-res:R_Added")), 1);
    assert_eq!(number_of(&bound(&after, OTHER, FOOTPRINT)), 2);
}

#[test]
fn an_edit_with_files_is_replayed_and_its_correlation_id_is_never_reused() {
    let team = Team::start();
    let editor = team.member("Ada Editor", true);
    let before = team.read();
    let original = team.file(FOOTPRINT_FILE);
    let corrected = redrawn(&original);
    let upload = footprint_upload(&corrected, FOOTPRINT, "R.kicad_mod").unwrap();
    let command = cad_edit(&before, IPN, &[], vec![upload]);
    let first = submit_edit(&editor, &command).unwrap();
    applied(&first);
    let saved = team.read();
    // Sent again (as after a lost answer): the same answer, byte for byte,
    // and nothing more written.
    let again = submit_edit(&editor, &command).unwrap();
    assert!(again.replayed);
    assert_eq!((again.status, &again.body), (first.status, &first.body));
    assert_eq!(team.read().counts(), saved.counts());
    // The same correlation ID with other bytes is another request.
    let mut other = command.clone();
    other.cad.as_mut().unwrap().uploads[0].bytes =
        corrected.iter().copied().chain(*b"\n").collect();
    let reused = submit_edit(&editor, &other).unwrap();
    assert_eq!(reused.status, 409, "{}", reused.body);
    assert!(
        matches!(reused.outcome, PublishOutcome::CorrelationReused { .. }),
        "{}",
        reused.body
    );
    assert_eq!(team.read().counts(), saved.counts());
}
