/// <reference path="../pb_data/types.d.ts" />

// Editing one part (seeds kicad-parts-2254 and kicad-parts-7556). The route
// runs catalog_lib.js publishEdit in one transaction; see there for the
// steps. Team members marked can_edit_catalog and superusers may call it;
// other signed-in members get 403, anyone else 401.
//
// POST /api/catalog/edits: a change to a part's details, its Symbol and
// Footprint, or both. Each section is sent only when something in it
// changed, with the number it started from:
//
//   { correlationId, reason, ipn,
//     details: { expectedDetails,
//                fields: [{ name, value }],      every field of the current
//                                                details, same names and order
//                manufacturerParts: [{ manufacturer, mpn }] },  first is main
//     cad: { expectedRevision,
//            fields: [{ name, value }],      the Symbol and Footprint
//                                            fields of the current revision
//            uploads: [{ role, reference, fileName, formatVersion?,
//                        text | base64 }] } }   files, optional
//
// New part details add no revision. A new Symbol or Footprint adds a
// revision whose CAD references are bound to exact CAD item revisions: ones
// the Catalog already has, or ones made from files sent with the edit
// (kicad-parts-3b8b). Each upload has a role (symbol, footprint or
// 3d-model), the reference it is for (LIB:NAME in a g- library, or the
// 3d-models/... path a footprint sent with it names), the name of the file
// it came from, the KiCad (version ...) of the file a symbol came from, and
// its bytes, as text (symbols and footprints always) or base64. The server
// reads each file itself (see
// catalog_lib.js preparedUploads): a symbol is exactly one (symbol "NAME"
// ...) whose NAME is the reference's name, a footprint a whole .kicad_mod
// named like the reference. A file whose content is the CAD item's current
// revision's adds nothing; other content is the CAD item's next revision,
// or revision 1 of a new CAD item. Every file must be used.
//
// Answers:
//   200 { outcome: "applied", correlationId, ipn, changeId, seq,
//         auditEntryId, details: { id, number, snapshotSha256 } | null,
//         revision: { id, number, snapshotSha256 } | null,
//         cadItemRevisions,                  IDs the current revision binds
//         newCadItemRevisions: [{ kind, key, number, id, otherParts }] }
//                                            otherParts: how many other
//                                            parts use an older revision of
//                                            that symbol or footprint
//   422 { outcome: "noop", correlationId, ipn, message }
//         nothing would change; nothing is written
//   409 { outcome: "conflict", correlationId, ipn, message,
//         currentDetails?, currentRevision? }   each { id, number, snapshot,
//         snapshotSha256, changeKind, reason, actor, actorName, created }
//         someone else saved the details or revision a section started
//         from; only the ones that moved are given; nothing is written
//   409 { outcome: "correlation_reused", correlationId, message }
//   400 bad input (including a symbol or footprint the Catalog does not
//       have and no file was sent for, more than one symbol, a footprint
//       choice listed twice, or a file that is wrong or not used),
//       401 not signed in, 403 not allowed to edit, 404 no such part.
// Applied and no-op answers are replayed byte for byte for a repeated
// correlation ID (with X-Collab-Replayed: true).
//
// There is no restore route: an earlier revision is never copied back
// (kicad-parts-bd76; a bad revision will be archived instead).
//
// A request may be at most 32 MiB (EDIT_BODY_LIMIT in
// crates/catalog/src/pocketbase.rs refuses a bigger one before sending).
const EDIT_BODY_LIMIT = 32 * 1024 * 1024;
routerAdd(
  "POST",
  "/api/catalog/edits",
  (e) => {
    const lib = require(`${__hooks}/catalog_lib.js`);
    return lib.publishEdit(e);
  },
  $apis.bodyLimit(EDIT_BODY_LIMIT),
  $apis.requireAuth("users", "_superusers"),
);
