//! Excluded from BOM follows the CAD of each new part revision
//! (kicad-parts-ccc0): the server works it out from the stored bytes of the
//! CAD item revisions a new revision binds, and an existing revision never
//! changes. Each test starts its own disposable PocketBase on 127.0.0.1;
//! the repository is only read.

use catalog::PartRevision;
use catalog::pocketbase::{CatalogReadback, EditCommand, read_back, submit_edit};
use catalog_import::services;
use collaboration::PocketBaseClient;
use native_services::PocketBaseServer;
use std::fs;

/// A resistor the tests edit.
const EDITED: &str = "RES-0000-0470";
/// The test point, whose symbol g-art:TestPoint says (in_bom no).
const TEST_POINT: &str = "ART-0000-0001";
/// A footprint whose (attr ...) has exclude_from_bom.
const NO_BOM_FOOTPRINT: &str = "g-TestPoint:TestPoint_Pad_D1.5mm";

struct Team {
    server: PocketBaseServer,
    admin: PocketBaseClient,
    _state: tempfile::TempDir,
}

impl Team {
    /// A team server with the sample test point and resistors.
    fn start() -> Self {
        let team = Self::empty(None);
        sample_library::seed(&team.admin, &["res", "art"]).unwrap();
        team
    }

    /// A team server with no parts. With `extra_hook`, the server also
    /// loads that hook file.
    fn empty(extra_hook: Option<&str>) -> Self {
        let root = services::repository_root();
        let state = tempfile::tempdir().unwrap();
        let mut options = services::options(&root, state.path());
        if let Some(hook) = extra_hook {
            let hooks = state.path().join("hooks");
            fs::create_dir_all(&hooks).unwrap();
            for entry in fs::read_dir(root.join("infra/pocketbase/pb_hooks")).unwrap() {
                let entry = entry.unwrap();
                fs::copy(entry.path(), hooks.join(entry.file_name())).unwrap();
            }
            fs::write(hooks.join("test_bom.pb.js"), hook).unwrap();
            options.hooks_dir = hooks;
        }
        let superuser = native_services::Superuser::generate();
        let binary = native_services::pocketbase_binary(&root).unwrap();
        let server = PocketBaseServer::start(&binary, &options, &superuser).unwrap();
        let admin = PocketBaseClient::new(server.url())
            .authenticate("_superusers", &superuser.email, &superuser.password)
            .unwrap();
        Self {
            server,
            admin,
            _state: state,
        }
    }

    fn read(&self) -> CatalogReadback {
        read_back(&self.admin).unwrap()
    }
}

/// The part's current revision: its number and document.
fn current(stored: &CatalogReadback, ipn: &str) -> (u32, PartRevision) {
    let part = stored.part(ipn).unwrap();
    let revision = stored.part_revision(&part.current_revision).unwrap();
    (
        revision.number,
        serde_json::from_str(&revision.snapshot).unwrap(),
    )
}

/// The stored snapshot of revision `number` of `ipn`.
fn snapshot(stored: &CatalogReadback, ipn: &str, number: u32) -> String {
    let part = stored.part(ipn).unwrap();
    stored
        .part_revisions
        .iter()
        .find(|revision| revision.part == part.id && revision.number == number)
        .unwrap()
        .snapshot
        .clone()
}

/// Sets the Symbol or Footprint of `ipn` with a CAD edit and returns the
/// new current revision.
fn edit_cad(team: &Team, ipn: &str, field: &str, value: &str) -> (u32, PartRevision) {
    let stored = team.read();
    let (number, revision) = current(&stored, ipn);
    let mut command = EditCommand::new(ipn, &format!("Use {value}")).editing_cad(&revision, number);
    assert!(command.set_field(field, value));
    let reply = submit_edit(&team.admin, &command).unwrap();
    assert_eq!(reply.status, 200, "{}", reply.body);
    current(&team.read(), ipn)
}

#[test]
fn an_edit_recalculates_excluded_from_bom_from_the_new_cad() {
    let team = Team::start();
    let stored = team.read();
    // Adding the test point set it, from its symbol's (in_bom no).
    assert!(current(&stored, TEST_POINT).1.excluded_from_bom);
    assert!(!current(&stored, EDITED).1.excluded_from_bom);
    let first = snapshot(&stored, EDITED, 1);

    // Switching to the test point symbol sets it; revision 1 is unchanged.
    let (number, revision) = edit_cad(&team, EDITED, "Symbol", "g-art:TestPoint");
    assert_eq!(number, 2);
    assert!(revision.excluded_from_bom);
    assert!(snapshot(&team.read(), EDITED, 2).ends_with(r#","excluded_from_bom":true}"#));
    assert_eq!(
        snapshot(&team.read(), EDITED, 1),
        first,
        "revision 1 never changes"
    );

    // Switching back clears it; revision 2 keeps it.
    let (number, revision) = edit_cad(&team, EDITED, "Symbol", "g-res:R_US");
    assert_eq!(number, 3);
    assert!(!revision.excluded_from_bom);
    let stored = team.read();
    assert!(!snapshot(&stored, EDITED, 3).contains("excluded_from_bom"));
    let second: PartRevision = serde_json::from_str(&snapshot(&stored, EDITED, 2)).unwrap();
    assert!(second.excluded_from_bom, "revision 2 never changes");

    // A symbol from KiCad's own library is not read, so it gives no
    // evidence either way.
    let (_, revision) = edit_cad(&team, EDITED, "Symbol", "Device:R");
    assert!(!revision.excluded_from_bom);

    // Footprint choices: an edit to only the marked footprint sets it,
    // even with KiCad's own symbol...
    let (_, revision) = edit_cad(&team, EDITED, "Footprint", NO_BOM_FOOTPRINT);
    assert!(revision.excluded_from_bom);
    // ...and an edit to disagreeing choices leaves it unset.
    let (_, revision) = edit_cad(
        &team,
        EDITED,
        "Footprint",
        &format!("g-Resistor_SMD:R_0603_1608Metric;{NO_BOM_FOOTPRINT}"),
    );
    assert!(!revision.excluded_from_bom);
    team.server.stop();
}

/// A route that answers what the server's reader says about some bytes.
const BOM_MARK_HOOK: &str = r#"
routerAdd("POST", "/api/test/bom-mark", (e) => {
  const lib = require(`${__hooks}/catalog_lib.js`);
  const body = e.requestInfo().body;
  return e.json(200, { mark: lib.bomMark(body.role, lib.base64Decode(body.base64)) });
}, $apis.requireSuperuserAuth());
"#;

#[test]
fn the_server_reads_bom_flags_exactly_as_the_desktop_does() {
    use kicad_integration::library_file::{
        footprint_says_excluded_from_bom, symbol_entries_say_not_in_bom,
    };
    let team = Team::empty(Some(BOM_MARK_HOOK));
    let footprint = |attr: &[u8]| -> Vec<u8> {
        let mut bytes = b"(footprint \"F\"\n\t(version 20241229)\n\t(descr \"plain\")\n\t".to_vec();
        bytes.extend_from_slice(attr);
        bytes.extend_from_slice(b"\n)\n");
        bytes
    };
    let symbol = |inner: &[u8]| -> Vec<u8> {
        let mut bytes = b"(symbol \"TP\"\n\t\t".to_vec();
        bytes.extend_from_slice(inner);
        bytes.extend_from_slice(b"\n\t)");
        bytes
    };
    let latin1 = b"(footprint \"F\" (descr \"caf\xe9\") (attr smd exclude_from_bom))".to_vec();
    let cases: Vec<(&str, &str, Vec<u8>)> = vec![
        (
            "footprint",
            "flagged",
            footprint(b"(attr smd exclude_from_bom)"),
        ),
        ("footprint", "not flagged", footprint(b"(attr smd)")),
        ("footprint", "a Latin-1 byte", latin1),
        (
            "footprint",
            "NBSP",
            footprint(b"(attr smd\xc2\xa0exclude_from_bom)"),
        ),
        (
            "footprint",
            "VT",
            footprint(b"(attr smd\x0bexclude_from_bom)"),
        ),
        (
            "footprint",
            "FF",
            footprint(b"(attr smd\x0cexclude_from_bom)"),
        ),
        (
            "footprint",
            "CR",
            footprint(b"(attr smd\rexclude_from_bom)"),
        ),
        (
            "footprint",
            "U+FEFF",
            footprint(b"(attr smd\xef\xbb\xbfexclude_from_bom)"),
        ),
        (
            "footprint",
            "U+2028",
            footprint(b"(attr smd\xe2\x80\xa8exclude_from_bom)"),
        ),
        (
            "footprint",
            "a string",
            footprint(b"(attr smd \"exclude_from_bom\")"),
        ),
        (
            "footprint",
            "nested",
            footprint(b"(attr (smd exclude_from_bom))"),
        ),
        (
            "footprint",
            "elsewhere",
            footprint(b"(fp_text value \"x\" exclude_from_bom)"),
        ),
        (
            "footprint",
            "module",
            b"(module F (attr exclude_from_bom))".to_vec(),
        ),
        (
            "footprint",
            "unbalanced",
            b"(footprint F (attr exclude_from_bom)".to_vec(),
        ),
        (
            "footprint",
            "a byte order mark",
            b"\xef\xbb\xbf(footprint F (attr exclude_from_bom))".to_vec(),
        ),
        (
            "footprint",
            "a trailing backslash",
            b"(footprint F (attr exclude_from_bom) \"\\".to_vec(),
        ),
        ("symbol", "in_bom no", symbol(b"(in_bom no)")),
        ("symbol", "in_bom yes", symbol(b"(in_bom yes)")),
        ("symbol", "VT", symbol(b"(in_bom\x0bno)")),
        ("symbol", "FF", symbol(b"(in_bom\x0cno)")),
        ("symbol", "NBSP", symbol(b"(in_bom\xc2\xa0no)")),
        ("symbol", "a quoted head", symbol(b"(\"in_bom\" \"no\")")),
        (
            "symbol",
            "a Latin-1 byte",
            symbol(b"(property \"x\" \"caf\xe9\") (in_bom no)"),
        ),
        (
            "symbol",
            "only a unit",
            symbol(b"(symbol \"TP_0_1\" (in_bom no))"),
        ),
        ("symbol", "unnamed", b"(symbol (in_bom no))".to_vec()),
        (
            "symbol",
            "an unnamed entry beside a marked one",
            b"(symbol \"TP\" (in_bom no)) (symbol (in_bom yes))".to_vec(),
        ),
        (
            "symbol",
            "a parenthesis in a string",
            symbol(b"(property \"x\" \"a ) b\") (in_bom no)"),
        ),
    ];
    for (role, what, bytes) in cases {
        let rust = if role == "symbol" {
            symbol_entries_say_not_in_bom(&bytes)
        } else {
            footprint_says_excluded_from_bom(&bytes)
        };
        let reply = team
            .admin
            .post_json(
                "/api/test/bom-mark",
                &serde_json::json!({ "role": role, "base64": catalog::pocketbase::base64(&bytes) }),
            )
            .unwrap();
        assert_eq!(reply.status, 200, "{role} {what}: {}", reply.body);
        let server: serde_json::Value = serde_json::from_str(&reply.body).unwrap();
        assert_eq!(server["mark"], rust, "{role} with {what}");
    }
    team.server.stop();
}
