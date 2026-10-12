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
//            fields: [{ name, value }] } }       the Symbol and Footprint
//                                                fields of the current revision
//
// New part details add no revision. A new Symbol or Footprint adds a
// revision whose CAD references are bound to exact CAD item revisions the
// Catalog already has.
//
// Answers:
//   200 { outcome: "applied", correlationId, ipn, changeId, seq,
//         auditEntryId, details: { id, number, snapshotSha256 } | null,
//         revision: { id, number, snapshotSha256 } | null,
//         cadItemRevisions }
//   422 { outcome: "noop", correlationId, ipn, message }
//         nothing would change; nothing is written
//   409 { outcome: "conflict", correlationId, ipn, message,
//         currentDetails?, currentRevision? }   each { id, number, snapshot,
//         snapshotSha256, changeKind, reason, actor, actorName, created }
//         someone else saved the details or revision a section started
//         from; only the ones that moved are given; nothing is written
//   409 { outcome: "correlation_reused", correlationId, message }
//   400 bad input (including a symbol or footprint the Catalog does not
//       have, more than one symbol, or a footprint choice listed twice),
//       401 not signed in, 403 not allowed to edit, 404 no such part.
// Applied and no-op answers are replayed byte for byte for a repeated
// correlation ID (with X-Collab-Replayed: true).
//
// There is no restore route: an earlier revision is never copied back
// (kicad-parts-bd76; a bad revision will be archived instead).
routerAdd(
  "POST",
  "/api/catalog/edits",
  (e) => {
    const lib = require(`${__hooks}/catalog_lib.js`);
    return lib.publishEdit(e);
  },
  $apis.requireAuth("users", "_superusers"),
);
